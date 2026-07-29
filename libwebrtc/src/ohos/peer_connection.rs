// Copyright 2025 LiveKit, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
// OHOS implementation - pure Rust, backed by webrtc-rs/rtc.
//
// This file wires the public `crate::peer_connection::PeerConnection`
// wrapper to the [`RtcIoDriver`] tokio task. The driver owns the rtc
// crate's `RTCPeerConnection`; this module only owns the channels used
// to talk to it plus a mirror of the high-level state needed by the
// synchronous accessors (`signaling_state()`, `current_local_description()`,
// ...). All async methods round-trip through the driver via a oneshot
// reply channel.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot};

use super::data_channel::DataChannel as ImpDataChannel;
use super::rtc_io_driver::{
    AddTrackParams, ControlCommand, PcEvent, RtpPacketData, TransceiverInfo,
};
use super::rtp_send_pipeline::RtpSendPipeline;
use super::{
    media_stream::MediaStream as ImpMediaStream,
    media_stream_track::new_media_stream_track,
    rtp_receiver::RtpReceiver as ImpRtpReceiver, rtp_sender::RtpSender as ImpRtpSender,
    rtp_transceiver::RtpTransceiver as ImpRtpTransceiver,
};
use crate::{
    data_channel::{DataChannel, DataChannelInit, DataChannelState},
    ice_candidate::IceCandidate,
    media_stream::MediaStream,
    media_stream_track::MediaStreamTrack,
    peer_connection::{
        IceConnectionState, IceGatheringState, OnConnectionChange, OnDataChannel, OnIceCandidate,
        OnIceCandidateError, OnIceConnectionChange, OnIceGatheringChange, OnNegotiationNeeded,
        OnSignalingChange, OnTrack, PeerConnectionState, SignalingState, TrackEvent,
    },
    peer_connection_factory::RtcConfiguration,
    rtp_receiver::RtpReceiver,
    rtp_sender::RtpSender,
    rtp_transceiver::{RtpTransceiver, RtpTransceiverDirection, RtpTransceiverInit},
    session_description::{SdpType, SessionDescription},
    stats::RtcStats,
    MediaType, RtcError, RtcErrorType,
};

// `OfferOptions`/`AnswerOptions` are part of the public method contract.
use crate::peer_connection::{AnswerOptions, OfferOptions};

// ---------------------------------------------------------------------------
// Observer state
// ---------------------------------------------------------------------------

/// OHOS peer connection observer state.
///
/// The public layer registers `Box<dyn FnMut>` callbacks for every WebRTC
/// signal. They live here behind a mutex so the driver task can dispatch
/// them from any thread.
#[derive(Default)]
pub struct PeerObserver {
    pub connection_change_handler: Mutex<Option<OnConnectionChange>>,
    pub data_channel_handler: Mutex<Option<OnDataChannel>>,
    pub ice_candidate_handler: Mutex<Option<OnIceCandidate>>,
    pub ice_candidate_error_handler: Mutex<Option<OnIceCandidateError>>,
    pub ice_connection_change_handler: Mutex<Option<OnIceConnectionChange>>,
    pub ice_gathering_change_handler: Mutex<Option<OnIceGatheringChange>>,
    pub negotiation_needed_handler: Mutex<Option<OnNegotiationNeeded>>,
    pub signaling_change_handler: Mutex<Option<OnSignalingChange>>,
    pub track_handler: Mutex<Option<OnTrack>>,
// ---------------------------------------------------------------------------
}
// Inner state
// ---------------------------------------------------------------------------

/// Internal mutable state of the OHOS peer connection.
struct PeerConnectionInner {
    config: RtcConfiguration,
    cmd_tx: mpsc::UnboundedSender<ControlCommand>,

    connection_state: PeerConnectionState,
    ice_connection_state: IceConnectionState,
    ice_gathering_state: IceGatheringState,
    signaling_state: SignalingState,
    local_description: Option<SessionDescription>,
    remote_description: Option<SessionDescription>,
    senders: Vec<ImpRtpSender>,
    receivers: Vec<ImpRtpReceiver>,
    transceivers: Vec<ImpRtpTransceiver>,
    next_id: u64,
    closed: bool,
    /// Data channels indexed by the SCTP stream id assigned by the driver.
    /// Used to dispatch state-change events from the rtc state machine.
    data_channels: HashMap<u16, ImpDataChannel>,
    /// Counter used to mint temporary negative ids for data channels that
    /// haven't yet been opened on the rtc side.
    next_pending_dc_id: i32,
}

impl PeerConnectionInner {
    fn new(config: RtcConfiguration, cmd_tx: mpsc::UnboundedSender<ControlCommand>) -> Self {
        Self {
            config,
            cmd_tx,
            connection_state: PeerConnectionState::New,
            ice_connection_state: IceConnectionState::New,
            ice_gathering_state: IceGatheringState::New,
            signaling_state: SignalingState::Stable,
            local_description: None,
            remote_description: None,
            senders: Vec::new(),
            receivers: Vec::new(),
            transceivers: Vec::new(),
            next_id: 0,
            closed: false,
            data_channels: HashMap::new(),
            next_pending_dc_id: -1,
        }
    }

    fn alloc_id(&mut self, prefix: &str) -> String {
        let id = self.next_id;
        self.next_id += 1;
        format!("{prefix}-{id}")
    }
}

// ---------------------------------------------------------------------------
// PeerConnection
// ---------------------------------------------------------------------------

/// OHOS peer connection handle.
#[derive(Clone)]
pub struct PeerConnection {
    inner: Arc<Mutex<PeerConnectionInner>>,
    observer: Arc<PeerObserver>,
}

impl PeerConnection {
    /// Construct a new peer connection wrapper. The factory is expected
    /// to drive the [`RtcIoDriver`] task and feed events back via
    /// [`PeerConnection::spawn_event_consumer`].
    pub(crate) fn new(
        config: RtcConfiguration,
        cmd_tx: mpsc::UnboundedSender<ControlCommand>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(PeerConnectionInner::new(config, cmd_tx))),
            observer: Arc::new(PeerObserver::default()),
        }
    }

    /// Borrow the observer (used by the factory when wiring up native callbacks).
    #[allow(dead_code)]
    pub(crate) fn observer(&self) -> Arc<PeerObserver> {
        self.observer.clone()
    }

    /// Spawn the event-consumer task that translates [`PcEvent`]s coming
    /// from the driver into mirror-state updates and user callbacks.
    pub(crate) fn spawn_event_consumer(&self, mut event_rx: mpsc::UnboundedReceiver<PcEvent>) {
        let inner = self.inner.clone();
        let observer = self.observer.clone();
        livekit_runtime::spawn(async move {
            while let Some(evt) = event_rx.recv().await {
                Self::handle_pc_event(&inner, &observer, evt);
            }
        });
    }

    fn handle_pc_event(
        inner: &Arc<Mutex<PeerConnectionInner>>,
        observer: &Arc<PeerObserver>,
        evt: PcEvent,
    ) {
        match evt {
            PcEvent::ConnectionStateChange(state) => {
                log::info!("[OHOS PC] ConnectionStateChange -> {:?}", state);
                inner.lock().connection_state = state;
                if let Some(cb) = observer.connection_change_handler.lock().as_mut() {
                    cb(state);
                }
            }
            PcEvent::IceConnectionStateChange(state) => {
                inner.lock().ice_connection_state = state;
                if let Some(cb) = observer.ice_connection_change_handler.lock().as_mut() {
                    cb(state);
                }
            }
            PcEvent::IceGatheringStateChange(state) => {
                inner.lock().ice_gathering_state = state;
                if let Some(cb) = observer.ice_gathering_change_handler.lock().as_mut() {
                    cb(state);
                }
            }
            PcEvent::SignalingStateChange(state) => {
                inner.lock().signaling_state = state;
                if let Some(cb) = observer.signaling_change_handler.lock().as_mut() {
                    cb(state);
                }
            }
            PcEvent::IceCandidate(candidate) => {
                if let Some(cb) = observer.ice_candidate_handler.lock().as_mut() {
                    cb(crate::ice_candidate::IceCandidate { handle: candidate });
                }
            }
            PcEvent::IceCandidateError(err) => {
                if let Some(cb) = observer.ice_candidate_error_handler.lock().as_mut() {
                    cb(err);
                }
            }
            PcEvent::NegotiationNeeded => {
                if let Some(cb) = observer.negotiation_needed_handler.lock().as_mut() {
                    cb(0);
                }
            }
            PcEvent::DataChannelOpen(id) => {
                if let Some(dc) = inner.lock().data_channels.get(&id).cloned() {
                    dc.set_state(DataChannelState::Open);
                }
            }
            PcEvent::DataChannelClosing(id) => {
                if let Some(dc) = inner.lock().data_channels.get(&id).cloned() {
                    dc.set_state(DataChannelState::Closing);
                }
            }
            PcEvent::DataChannelClosed(id) => {
                let dc = inner.lock().data_channels.remove(&id);
                if let Some(dc) = dc {
                    dc.set_state(DataChannelState::Closed);
                }
            }
            PcEvent::DataChannelError(_id) => {
                // The public `OnDataChannel` callback signature only
                // covers new channels, not error notifications. The
                // OHOS data-channel imp doesn't expose an error sink
                // either, so we drop the event for now.
            }
            PcEvent::DataChannelBufferedAmountLow(id) => {
                if let Some(dc) = inner.lock().data_channels.get(&id).cloned() {
                    dc.emit_buffered_amount_change(dc.buffered_amount());
                }
            }
            PcEvent::DataChannelBufferedAmountHigh(_id) => {
                // No corresponding callback in the public API.
            }
            PcEvent::RemoteDataChannel(handle) => {
                let imp = ImpDataChannel::new(handle.label, handle.id);
                inner.lock().data_channels.insert(handle.id as u16, imp.clone());
                if let Some(cb) = observer.data_channel_handler.lock().as_mut() {
                    cb(DataChannel { handle: imp });
                }
            }
            PcEvent::RemoteTrack { receiver_id, track_id, stream_ids, ssrc: _, kind, codec_mime } => {
                // The rtc state machine only surfaces RTP-bearing tracks; if
                // we somehow get an unsupported kind there's no public
                // representation to build, so drop the event.
                if kind == MediaType::Audio || kind == MediaType::Video {
                    // Pre-allocate the per-track receive queue and hand the
                    // sender to the driver immediately, so packets that arrive
                    // before the user constructs a stream are buffered instead
                    // of dropped on the floor.
                    let (rtp_tx, rtp_rx) = mpsc::unbounded_channel();
                    {
                        let state = inner.lock();
                        let _ = state.cmd_tx.send(ControlCommand::RegisterReceiver {
                            track_id: track_id.clone(),
                            sender: rtp_tx,
                        });
                    }

                    let pub_track = new_media_stream_track(track_id.clone(), kind);

                    // Attach the pre-allocated RTP receiver to the track
                    // object so that a later `NativeAudioStream::new()` /
                    // `NativeVideoStream::new()` construction can pick it
                    // up instead of creating a dummy (empty) channel.
                    match &pub_track {
                        crate::media_stream_track::MediaStreamTrack::Audio(a) => {
                            log::info!(
                                "[PeerConnection] set_rtp_rx on AUDIO track={}",
                                a.handle.id()
                            );
                            a.handle.set_rtp_rx(rtp_rx);
                        }
                        crate::media_stream_track::MediaStreamTrack::Video(v) => {
                            log::info!(
                                "[PeerConnection] set_rtp_rx on VIDEO track={}, rtp_rx Arc strong_count={}",
                                v.handle.id(),
                                std::sync::Arc::strong_count(&v.handle.rtp_rx)
                            );
                            v.handle.set_rtp_rx(rtp_rx);
                            if let Some(ref mime) = codec_mime {
                                v.handle.set_codec_mime(mime.clone());
                            }
                        }
                    }

                    let imp_receiver =
                        ImpRtpReceiver::new(receiver_id, Some(pub_track.clone()));
                    let imp_sender =
                        ImpRtpSender::new(format!("remote-sender-{track_id}"), None);

                    let tinit = RtpTransceiverInit {
                        direction: RtpTransceiverDirection::RecvOnly,
                        stream_ids: stream_ids.clone(),
                        send_encodings: Vec::new(),
                    };
                    let imp_transceiver =
                        ImpRtpTransceiver::new(&tinit, imp_sender, imp_receiver.clone());

                    let streams: Vec<MediaStream> = stream_ids
                        .into_iter()
                        .map(|sid| MediaStream { handle: ImpMediaStream::new(sid) })
                        .collect();

                    {
                        let mut state = inner.lock();
                        state.receivers.push(imp_receiver.clone());
                        state.transceivers.push(imp_transceiver.clone());
                    }

                    if let Some(cb) = observer.track_handler.lock().as_mut() {
                        cb(TrackEvent {
                            receiver: RtpReceiver { handle: imp_receiver },
                            streams,
                            track: pub_track,
                            transceiver: RtpTransceiver { handle: imp_transceiver },
                        });
                    }
                }
            }
        }
    }

    // ---- Configuration -----------------------------------------------------

    pub fn set_configuration(&self, config: RtcConfiguration) -> Result<(), RtcError> {
        let mut inner = self.inner.lock();
        if inner.closed {
            return Err(closed_err());
        }
        inner.config = config;
        // TODO(ohos): Forward live config changes (ICE servers etc.) to the
        // driver. The rtc crate currently expects configuration at build
        // time, so a runtime update would require recreating the
        // peer connection.
        Ok(())
    }

    // ---- SDP / ICE async API ----------------------------------------------

    pub async fn create_offer(
        &self,
        options: OfferOptions,
    ) -> Result<SessionDescription, RtcError> {
        let cmd_tx = self.cmd_tx_or_closed()?;
        let (tx, rx) = oneshot::channel();
        cmd_tx
            .send(ControlCommand::CreateOffer { options, reply: tx })
            .map_err(|_| driver_gone_err())?;
        let imp = rx.await.map_err(|_| driver_gone_err())??;
        Ok(SessionDescription { handle: imp })
    }

    pub async fn create_answer(
        &self,
        options: AnswerOptions,
    ) -> Result<SessionDescription, RtcError> {
        let cmd_tx = self.cmd_tx_or_closed()?;
        let (tx, rx) = oneshot::channel();
        cmd_tx
            .send(ControlCommand::CreateAnswer { options, reply: tx })
            .map_err(|_| driver_gone_err())?;
        let imp = rx.await.map_err(|_| driver_gone_err())??;
        Ok(SessionDescription { handle: imp })
    }

    pub async fn set_local_description(&self, desc: SessionDescription) -> Result<(), RtcError> {
        let cmd_tx = self.cmd_tx_or_closed()?;
        let (tx, rx) = oneshot::channel();
        cmd_tx
            .send(ControlCommand::SetLocalDescription { desc: desc.handle.clone(), reply: tx })
            .map_err(|_| driver_gone_err())?;
        rx.await.map_err(|_| driver_gone_err())??;
        // Mirror the new local description AND signaling state synchronously.
        // The async rtc event stream also updates the state, but consumers
        // (e.g. PeerTransport::create_and_send_offer) read it immediately
        // after this call returns — a stale cached state causes deferred
        // renegotiations that never fire.
        {
            let mut inner = self.inner.lock();
            inner.signaling_state = match desc.sdp_type() {
                SdpType::Offer => SignalingState::HaveLocalOffer,
                SdpType::PrAnswer => SignalingState::HaveLocalPrAnswer,
                SdpType::Answer | SdpType::Rollback => SignalingState::Stable,
            };
            inner.local_description = Some(desc);
        }
        Ok(())
    }

    pub async fn set_remote_description(&self, desc: SessionDescription) -> Result<(), RtcError> {
        let cmd_tx = self.cmd_tx_or_closed()?;
        let (tx, rx) = oneshot::channel();
        cmd_tx
            .send(ControlCommand::SetRemoteDescription { desc: desc.handle.clone(), reply: tx })
            .map_err(|_| driver_gone_err())?;
        rx.await.map_err(|_| driver_gone_err())??;
        // Mirror synchronously (see set_local_description for rationale).
        {
            let mut inner = self.inner.lock();
            inner.signaling_state = match desc.sdp_type() {
                SdpType::Offer => SignalingState::HaveRemoteOffer,
                SdpType::PrAnswer => SignalingState::HaveRemotePrAnswer,
                SdpType::Answer | SdpType::Rollback => SignalingState::Stable,
            };
            inner.remote_description = Some(desc);
        }
        Ok(())
    }

    pub async fn add_ice_candidate(&self, candidate: IceCandidate) -> Result<(), RtcError> {
        let cmd_tx = self.cmd_tx_or_closed()?;
        let (tx, rx) = oneshot::channel();
        cmd_tx
            .send(ControlCommand::AddIceCandidate { candidate: candidate.handle, reply: tx })
            .map_err(|_| driver_gone_err())?;
        rx.await.map_err(|_| driver_gone_err())?
    }

    // ---- Data channel ------------------------------------------------------

    /// Create a data channel.
    ///
    /// This API is synchronous in the public layer but the underlying rtc
    /// state machine only assigns a real SCTP stream id once the SCTP
    /// association has been negotiated. We return a [`DataChannel`] in the
    /// `Connecting` state immediately and back-fill its id from the driver
    /// reply (typically within microseconds). Until the id is set, the
    /// channel cannot be looked up by id but it can already be used by
    /// callers - sends are queued internally.
    pub fn create_data_channel(
        &self,
        label: &str,
        init: DataChannelInit,
    ) -> Result<DataChannel, RtcError> {
        let cmd_tx = self.cmd_tx_or_closed()?;

        // Mint a temporary id so the imp DataChannel has *something* to
        // report until the driver responds. We use negative ids so
        // they're easy to distinguish from real SCTP stream ids.
        let pending_id = {
            let mut inner = self.inner.lock();
            if inner.closed {
                return Err(closed_err());
            }
            let id = inner.next_pending_dc_id;
            inner.next_pending_dc_id -= 1;
            id
        };

        let imp = ImpDataChannel::new(label.to_owned(), pending_id);
        let imp_for_driver = imp.clone();
        let inner = self.inner.clone();

        let (tx, rx) = oneshot::channel();
        cmd_tx
            .send(ControlCommand::CreateDataChannel {
                label: label.to_owned(),
                init,
                reply: tx,
            })
            .map_err(|_| driver_gone_err())?;

        // Fire-and-forget task that updates the imp once the driver replies.
        livekit_runtime::spawn(async move {
            match rx.await {
                Ok(Ok(handle)) => {
                    imp_for_driver.set_id(handle.id);
                    let mut state = inner.lock();
                    if let Ok(real_id) = u16::try_from(handle.id) {
                        state.data_channels.insert(real_id, imp_for_driver);
                    }
                }
                Ok(Err(err)) => {
                    log::warn!("create_data_channel failed: {}", err.message);
                    imp_for_driver.set_state(DataChannelState::Closed);
                }
                Err(_) => {
                    log::warn!("create_data_channel: driver dropped reply");
                    imp_for_driver.set_state(DataChannelState::Closed);
                }
            }
        });

        Ok(DataChannel { handle: imp })
    }

    // ---- Track / transceiver bookkeeping -----------------------------------

    /// Attach a local track and return a sender wired into the rtc state
    /// machine. The driver round-trip is synchronous (matching the public
    /// API contract) and reuses `block_in_place(|| blocking_recv())` so it is safe to call from
    /// [`create_data_channel`].
    pub fn add_track<T: AsRef<str>>(
        &self,
        track: MediaStreamTrack,
        stream_ids: &[T],
    ) -> Result<RtpSender, RtcError> {
        let cmd_tx = self.cmd_tx_or_closed()?;

        let track_id = track.id();
        let kind = media_kind_of(&track);
        let stream_ids_vec: Vec<String> =
            stream_ids.iter().map(|s| s.as_ref().to_owned()).collect();
        let stream_id_primary = stream_ids_vec.first().cloned().unwrap_or_default();

        let (tx, rx) = oneshot::channel();
        cmd_tx
            .send(ControlCommand::AddTrack {
                params: AddTrackParams {
                    stream_id: stream_id_primary,
                    track_id: track_id.clone(),
                    label: track_id.clone(),
                    kind,
                    ssrc: 0, // let the rtc crate allocate
                    codec_mime: default_codec_mime(kind).to_owned(),
                    clock_rate: default_clock_rate(kind),
                    channels: default_channels(kind),
                    stream_ids: stream_ids_vec,
                },
                reply: tx,
            })
            .map_err(|_| driver_gone_err())?;

        let (sender_id, actual_ssrc) = tokio::task::block_in_place(|| rx.blocking_recv())
            .map_err(|_| driver_gone_err())??;

        // Bind an RtpSendPipeline to the source so that encoded frames pushed
        // via `source.send_encoded_frame()` are forwarded as RTP packets.
        // Use the actual SSRC assigned by the rtc crate, not the one we requested.
        {
            let clock_rate = default_clock_rate(kind);
            let payload_type = default_payload_type(kind);
            let pipeline = RtpSendPipeline::new(
                track_id.clone(),
                actual_ssrc, // use the actual SSRC from rtc crate
                payload_type,
                96, // VP8 fallback PT (must match peer_connection_factory.rs)
                clock_rate,
                cmd_tx.clone(),
            );
            match &track {
                MediaStreamTrack::Video(video_track) => {
                    if let Some(source) = &video_track.handle.source {
                        source.bind_rtp_pipeline(pipeline);
                    }
                }
                MediaStreamTrack::Audio(audio_track) => {
                    if let Some(source) = &audio_track.handle.source {
                        source.bind_rtp_pipeline(pipeline);
                    }
                }
            }
        }

        let sender = ImpRtpSender::new(sender_id, Some(track));
        self.inner.lock().senders.push(sender.clone());
        Ok(RtpSender { handle: sender })
    }

    /// Detach a local track, asking the driver to stop the corresponding
    /// rtc-crate sender and renegotiate.
    pub fn remove_track(&self, sender: RtpSender) -> Result<(), RtcError> {
        let cmd_tx = self.cmd_tx_or_closed()?;

        let id = sender.handle.id.clone();
        let (tx, rx) = oneshot::channel();
        cmd_tx
            .send(ControlCommand::RemoveTrack { sender_id: id.clone(), reply: tx })
            .map_err(|_| driver_gone_err())?;
        tokio::task::block_in_place(|| rx.blocking_recv())
            .map_err(|_| driver_gone_err())??;

        let mut inner = self.inner.lock();
        inner.senders.retain(|s| s.id != id);
        // Detach the track so consumers observe the removal.
        let _ = sender.handle.set_track(None);
        Ok(())
    }

    pub async fn get_stats(&self) -> Result<Vec<RtcStats>, RtcError> {
        let cmd_tx = self.cmd_tx_or_closed()?;
        let (tx, rx) = oneshot::channel();
        cmd_tx
            .send(ControlCommand::GetStats { reply: tx })
            .map_err(|_| driver_gone_err())?;
        rx.await.map_err(|_| driver_gone_err())?
    }

    pub fn add_transceiver(
        &self,
        track: MediaStreamTrack,
        init: RtpTransceiverInit,
    ) -> Result<RtpTransceiver, RtcError> {
        let cmd_tx = self.cmd_tx_or_closed()?;

        let track_id = track.id();
        let kind = media_kind_of(&track);
        let stream_ids_vec = init.stream_ids.clone();
        let stream_id_primary = stream_ids_vec.first().cloned().unwrap_or_default();

        let (tx, rx) = oneshot::channel();
        cmd_tx
            .send(ControlCommand::AddTrack {
                params: AddTrackParams {
                    stream_id: stream_id_primary,
                    track_id: track_id.clone(),
                    label: track_id.clone(),
                    kind,
                    ssrc: 0, // let the rtc crate allocate
                    codec_mime: default_codec_mime(kind).to_owned(),
                    clock_rate: default_clock_rate(kind),
                    channels: default_channels(kind),
                    stream_ids: stream_ids_vec,
                },
                reply: tx,
            })
            .map_err(|_| driver_gone_err())?;

        let (sender_id, actual_ssrc) = tokio::task::block_in_place(|| rx.blocking_recv())
            .map_err(|_| driver_gone_err())??;

        // Bind an RtpSendPipeline to the source so that encoded frames pushed
        // via `source.send_encoded_frame()` are forwarded as RTP packets.
        {
            let clock_rate = default_clock_rate(kind);
            let payload_type = default_payload_type(kind);
            let pipeline = RtpSendPipeline::new(
                track_id.clone(),
                actual_ssrc,
                payload_type,
                96, // VP8 fallback PT (must match peer_connection_factory.rs)
                clock_rate,
                cmd_tx.clone(),
            );
            match &track {
                MediaStreamTrack::Video(video_track) => {
                    if let Some(source) = &video_track.handle.source {
                        source.bind_rtp_pipeline(pipeline);
                    }
                }
                MediaStreamTrack::Audio(audio_track) => {
                    if let Some(source) = &audio_track.handle.source {
                        source.bind_rtp_pipeline(pipeline);
                    }
                }
            }
        }

        let sender = ImpRtpSender::new(sender_id, Some(track));
        let receiver = ImpRtpReceiver::new(format!("recv-{track_id}"), None);
        let transceiver = ImpRtpTransceiver::new(&init, sender.clone(), receiver.clone());

        let mut inner = self.inner.lock();
        inner.senders.push(sender);
        inner.receivers.push(receiver);
        inner.transceivers.push(transceiver.clone());
        Ok(RtpTransceiver { handle: transceiver })
    }

    /// Create an `m=` section of the given media kind without attaching a
    /// local track. The driver runs the actual rtc-crate call; we then
    /// mirror the returned identifiers/mid in the local imp structs so
    /// the synchronous accessors (`senders`, `receivers`, `transceivers`)
    /// keep working as before.
    pub fn add_transceiver_for_media(
        &self,
        media_type: MediaType,
        init: RtpTransceiverInit,
    ) -> Result<RtpTransceiver, RtcError> {
        let cmd_tx = self.cmd_tx_or_closed()?;
        let (tx, rx) = oneshot::channel();
        cmd_tx
            .send(ControlCommand::AddTransceiverForMedia {
                kind: media_type,
                direction: init.direction,
                reply: tx,
            })
            .map_err(|_| driver_gone_err())?;
        let info: TransceiverInfo =
            tokio::task::block_in_place(|| rx.blocking_recv()).map_err(|_| driver_gone_err())??;

        let sender = ImpRtpSender::new(info.sender_id, None);
        let receiver = ImpRtpReceiver::new(info.receiver_id, None);
        let transceiver = ImpRtpTransceiver::new(&init, sender.clone(), receiver.clone());
        if let Some(mid) = info.mid {
            *transceiver.mid.lock() = Some(mid);
        }
        *transceiver.direction.lock() = info.direction;

        let mut inner = self.inner.lock();
        inner.senders.push(sender);
        inner.receivers.push(receiver);
        inner.transceivers.push(transceiver.clone());
        Ok(RtpTransceiver { handle: transceiver })
    }

    /// Fire-and-forget RTP packet write. Used by audio/video sources to
    /// inject encoded media into the established connection. Errors are
    /// returned synchronously only when the driver channel is closed -
    /// the packet itself is dispatched without waiting for confirmation,
    /// so high-frequency callers never block.
    pub(crate) fn write_rtp(&self, track_id: &str, packet: RtpPacketData) -> Result<(), RtcError> {
        let cmd_tx = self.cmd_tx_or_closed()?;
        cmd_tx
            .send(ControlCommand::WriteRtp { track_id: track_id.to_owned(), packet })
            .map_err(|_| driver_gone_err())?;
        Ok(())
    }

    pub fn restart_ice(&self) {
        // Forward the request to the driver. The rtc state machine simply
        // sets a flag that biases the next `create_offer` toward emitting
        // fresh ICE credentials; we don't propagate the boolean reply
        // upward because the public API is fire-and-forget.
        let cmd_tx = match self.cmd_tx_or_closed() {
            Ok(tx) => tx,
            Err(_) => return,
        };
        let (tx, rx) = oneshot::channel();
        if cmd_tx.send(ControlCommand::RestartIce { reply: tx }).is_err() {
            return;
        }
        // Block briefly on the driver's acknowledgement so the local
        // state mutation below cannot race ahead of the rtc-crate flag
        // update. The driver replies as soon as the flag is set, so this
        // is a sub-millisecond round-trip.
        let _ = tokio::task::block_in_place(|| rx.blocking_recv());

        let mut inner = self.inner.lock();
        inner.ice_gathering_state = IceGatheringState::New;
        inner.ice_connection_state = IceConnectionState::New;
    }

    pub fn close(&self) {
        let cmd_tx = {
            let mut inner = self.inner.lock();
            if inner.closed {
                log::info!("[PeerConnection] close() called but already closed");
                return;
            }
            log::info!(
                "[PeerConnection] close() called, state={:?}, senders={}, transceivers={}",
                inner.connection_state,
                inner.senders.len(),
                inner.transceivers.len()
            );
            inner.closed = true;
            inner.connection_state = PeerConnectionState::Closed;
            inner.ice_connection_state = IceConnectionState::Closed;
            inner.signaling_state = SignalingState::Closed;
            for tx in &inner.transceivers {
                *tx.direction.lock() = RtpTransceiverDirection::Stopped;
                *tx.current_direction.lock() = Some(RtpTransceiverDirection::Stopped);
            }
            inner.cmd_tx.clone()
        };

        // Tell the driver to shut down. If it's already gone we don't care.
        let _ = cmd_tx.send(ControlCommand::Close);

        // Surface the synthesised state changes immediately so callers
        // don't have to wait for the driver event loop to round-trip.
        let conn_state = self.inner.lock().connection_state;
        let ice_state = self.inner.lock().ice_connection_state;
        let sig_state = self.inner.lock().signaling_state;
        if let Some(cb) = self.observer.connection_change_handler.lock().as_mut() {
            cb(conn_state);
        }
        if let Some(cb) = self.observer.ice_connection_change_handler.lock().as_mut() {
            cb(ice_state);
        }
        if let Some(cb) = self.observer.signaling_change_handler.lock().as_mut() {
            cb(sig_state);
        }
    }

    // ---- Synchronous accessors --------------------------------------------

    pub fn connection_state(&self) -> PeerConnectionState {
        self.inner.lock().connection_state
    }

    pub fn ice_connection_state(&self) -> IceConnectionState {
        self.inner.lock().ice_connection_state
    }

    pub fn ice_gathering_state(&self) -> IceGatheringState {
        self.inner.lock().ice_gathering_state
    }

    pub fn signaling_state(&self) -> SignalingState {
        self.inner.lock().signaling_state
    }

    pub fn current_local_description(&self) -> Option<SessionDescription> {
        self.inner.lock().local_description.clone()
    }

    pub fn current_remote_description(&self) -> Option<SessionDescription> {
        self.inner.lock().remote_description.clone()
    }

    pub fn senders(&self) -> Vec<RtpSender> {
        self.inner
            .lock()
            .senders
            .iter()
            .cloned()
            .map(|handle| RtpSender { handle })
            .collect()
    }

    pub fn receivers(&self) -> Vec<RtpReceiver> {
        self.inner
            .lock()
            .receivers
            .iter()
            .cloned()
            .map(|handle| RtpReceiver { handle })
            .collect()
    }

    pub fn transceivers(&self) -> Vec<RtpTransceiver> {
        self.inner
            .lock()
            .transceivers
            .iter()
            .cloned()
            .map(|handle| RtpTransceiver { handle })
            .collect()
    }

    // ---- Callback registration --------------------------------------------

    pub fn on_connection_state_change(&self, f: Option<OnConnectionChange>) {
        *self.observer.connection_change_handler.lock() = f;
    }

    pub fn on_data_channel(&self, f: Option<OnDataChannel>) {
        *self.observer.data_channel_handler.lock() = f;
    }

    pub fn on_ice_candidate(&self, f: Option<OnIceCandidate>) {
        *self.observer.ice_candidate_handler.lock() = f;
    }

    pub fn on_ice_candidate_error(&self, f: Option<OnIceCandidateError>) {
        *self.observer.ice_candidate_error_handler.lock() = f;
    }

    pub fn on_ice_connection_state_change(&self, f: Option<OnIceConnectionChange>) {
        *self.observer.ice_connection_change_handler.lock() = f;
    }

    pub fn on_ice_gathering_state_change(&self, f: Option<OnIceGatheringChange>) {
        *self.observer.ice_gathering_change_handler.lock() = f;
    }

    pub fn on_negotiation_needed(&self, f: Option<OnNegotiationNeeded>) {
        *self.observer.negotiation_needed_handler.lock() = f;
    }

    pub fn on_signaling_state_change(&self, f: Option<OnSignalingChange>) {
        *self.observer.signaling_change_handler.lock() = f;
    }

    pub fn on_track(&self, f: Option<OnTrack>) {
        *self.observer.track_handler.lock() = f;
    }

    // ---- Internal helpers --------------------------------------------------

    fn cmd_tx_or_closed(&self) -> Result<mpsc::UnboundedSender<ControlCommand>, RtcError> {
        let inner = self.inner.lock();
        if inner.closed {
            return Err(closed_err());
        }
        Ok(inner.cmd_tx.clone())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn closed_err() -> RtcError {
    RtcError {
        error_type: RtcErrorType::InvalidState,
        message: "peer connection is closed".to_owned(),
    }
}

fn driver_gone_err() -> RtcError {
    RtcError {
        error_type: RtcErrorType::Internal,
        message: "rtc IO driver is no longer running".to_owned(),
    }
}

/// Map a public [`MediaStreamTrack`] enum variant to the kind enum the
/// driver and rtc state machine reason about. The OHOS backend only
/// supports audio/video on the RTP path.
fn media_kind_of(track: &MediaStreamTrack) -> MediaType {
    match track {
        MediaStreamTrack::Audio(_) => MediaType::Audio,
        MediaStreamTrack::Video(_) => MediaType::Video,
    }
}

/// Default codec MIME used when the public layer adds a track without
/// announcing a specific encoder. Uses VP8 for video (software encoder,
/// universally supported by browsers) and Opus for audio; the
/// negotiated codec may differ once SDP exchange completes.
fn default_codec_mime(kind: MediaType) -> &'static str {
    match kind {
        MediaType::Video => "video/H264",
        MediaType::Audio => "audio/opus",
        _ => "",
    }
}

/// Default RTP clock rate for the given media kind. 90 kHz for video and
/// 48 kHz for audio (Opus); zero for kinds that don't carry RTP.
fn default_clock_rate(kind: MediaType) -> u32 {
    match kind {
        MediaType::Video => 90000,
        MediaType::Audio => 48000,
        _ => 0,
    }
}

/// Default channel count. Stereo for audio; video tracks ignore the
/// field but the rtc-crate codec descriptor still requires a value.
fn default_channels(kind: MediaType) -> u16 {
    match kind {
        MediaType::Audio => 2,
        _ => 0,
    }
}

/// Default RTP payload type number for the given media kind.
/// Must match the payload types registered in peer_connection_factory.rs.
/// Video uses PT 96 (VP8, which is our primary codec); audio uses PT 111 (Opus).
fn default_payload_type(kind: MediaType) -> u8 {
    match kind {
        MediaType::Video => 125, // H264 — must match peer_connection_factory.rs
        MediaType::Audio => 111, // Opus
        _ => 0,
    }
}

/// Generate a random SSRC for outbound RTP streams.
///
/// Uses the sub-second nanos from the system clock as a simple random
/// source. This avoids pulling in an RNG crate for a non-cryptographic use.
fn rand_ssrc() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos()
}

