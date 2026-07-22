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

//! Sans-I/O ↔ async bridge for the [`rtc`] crate's `RTCPeerConnection`.
//!
//! The rtc crate exposes a polling, synchronous "sans-I/O" API: the
//! application is responsible for moving bytes between the network and the
//! state machine, and for driving timers. [`RtcIoDriver`] is a dedicated
//! tokio task that owns the `RTCPeerConnection`, performs the actual UDP
//! reads / writes through a [`tokio::net::UdpSocket`], and exposes an
//! async, channel-based command/event API that the public
//! [`super::peer_connection::PeerConnection`] wrapper uses to expose the
//! standard WebRTC surface.
//!
//! ```text
//!   PeerConnection (public, async/callback API)
//!         │ cmd_tx (ControlCommand)        ▲ event_rx (PcEvent)
//!         ▼                                │
//!   RtcIoDriver (tokio task, sans-I/O loop)
//!         │  poll_write/poll_event/poll_timeout/handle_read/handle_timeout
//!         ▼
//!   rtc::peer_connection::RTCPeerConnection<NoopInterceptor>
//!         │  send_to/recv_from
//!         ▼
//!   tokio::net::UdpSocket
//! ```

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};

// rtc crate (sans-I/O) types.
use rtc::data_channel::RTCDataChannelInit;
use rtc::interceptor::NoopInterceptor;
use rtc::media_stream::MediaStreamTrack as RtcMediaStreamTrack;
use rtc::peer_connection::configuration::{RTCAnswerOptions, RTCOfferOptions};
use rtc::peer_connection::event::{
    RTCDataChannelEvent, RTCPeerConnectionEvent, RTCPeerConnectionIceErrorEvent,
    RTCPeerConnectionIceEvent, RTCTrackEvent,
};
use rtc::peer_connection::message::RTCMessage;
use rtc::peer_connection::sdp::{RTCSdpType, RTCSessionDescription};
use rtc::peer_connection::state::{
    RTCIceConnectionState, RTCIceGatheringState, RTCPeerConnectionState, RTCSignalingState,
};
use rtc::peer_connection::transport::{
    CandidateConfig, CandidateHostConfig, RTCIceCandidate, RTCIceCandidateInit,
};
use rtc::peer_connection::RTCPeerConnection;
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind,
};
use rtc::rtp_transceiver::{
    RTCRtpSenderId, RTCRtpTransceiverDirection, RTCRtpTransceiverId, RTCRtpTransceiverInit,
};
use rtc::sansio::Protocol;
use rtc::shared::{TaggedBytesMut, TransportContext, TransportProtocol};

// Public-facing types from the parent crate.
use crate::data_channel::DataChannelInit;
#[cfg(any(target_env = "ohos", target_os = "android"))]
use super::h264_encoder::H264Encoder;
use crate::peer_connection::{
    AnswerOptions, IceCandidateError, IceConnectionState, IceGatheringState, OfferOptions,
    PeerConnectionState, SignalingState,
};
use crate::rtp_transceiver::RtpTransceiverDirection;
use crate::session_description::SdpType;
use crate::{MediaType, RtcError, RtcErrorType};

use super::ice_candidate::IceCandidate as ImpIceCandidate;
use super::session_description::SessionDescription as ImpSessionDescription;
use super::transport_manager::TransportManager;

/// Default fall-back interval used when the rtc crate has no pending timer.
///
/// The driver will still wake whenever a packet arrives or a command comes
/// in, so this only bounds the worst-case latency for waking up to drain
/// state-change events that might happen on internal book-keeping timers.
/// Default fall-back interval used when the rtc crate has no pending timer.
///
/// 20ms matches the recv_from timeout used in tdnis-ohos.  A shorter
/// tick ensures handle_timeout() drives the ICE state machine at least
/// every 20ms even when the socket is idle, preventing ICE connectivity
/// checks from stalling.
const IDLE_TICK: Duration = Duration::from_millis(20);

/// Maximum size of the UDP receive buffer.  DTLS handshake packets can be
/// quite large (up to ~60 KiB with certificate chains), so we use 64 KiB to
/// match the tdnis-ohos buffer size and avoid truncation.
const RECV_BUF_SIZE: usize = 65536;

// ---------------------------------------------------------------------------
// Channel message types
// ---------------------------------------------------------------------------

/// Parameters describing a media track to attach to the peer connection.
///
/// Mirrors the rtc-crate `MediaStreamTrack` plus the codec/SSRC hints that
/// the public wrapper supplies when it knows the encoded format up front
/// (which is the common case for OHOS where the application drives
/// encoding itself).
#[derive(Debug, Clone)]
pub(crate) struct AddTrackParams {
    pub stream_id: String,
    pub track_id: String,
    pub label: String,
    pub kind: MediaType,
    pub ssrc: u32,
    pub codec_mime: String,
    pub clock_rate: u32,
    pub channels: u16,
    pub stream_ids: Vec<String>,
}

/// Metadata returned to the public wrapper after a transceiver has been
/// created on the underlying rtc state machine.
#[derive(Debug, Clone)]
pub(crate) struct TransceiverInfo {
    pub mid: Option<String>,
    pub sender_id: String,
    pub receiver_id: String,
    pub direction: RtpTransceiverDirection,
}

/// Encoded RTP packet body submitted via [`ControlCommand::WriteRtp`].
///
/// The audio/video sources own packetisation; the driver merely wraps the
/// payload in an `rtp::Packet` and feeds it to the rtc state machine,
/// which performs SRTP encryption and ICE-aware transport.
#[derive(Debug, Clone)]
pub(crate) struct RtpPacketData {
    pub payload_type: u8,
    pub sequence_number: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub marker: bool,
    pub payload: Bytes,
}

/// Received RTP packet (after SRTP decryption by the rtc crate).
///
/// Pushed by the driver into a per-track channel registered via
/// [`ControlCommand::RegisterReceiver`]. The native audio/video stream
/// wrappers consume these and surface decoded frames to their callers.
#[derive(Debug, Clone)]
pub(crate) struct ReceivedRtpPacket {
    pub track_id: String,
    pub payload_type: u8,
    pub sequence_number: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub marker: bool,
    pub payload: Vec<u8>,
}

/// Commands sent from the public [`PeerConnection`] wrapper to the driver
/// task. Each command carries a [`oneshot::Sender`] used by the driver to
/// reply once the underlying rtc-crate call has returned.
pub(crate) enum ControlCommand {
    CreateOffer {
        options: OfferOptions,
        reply: oneshot::Sender<Result<ImpSessionDescription, RtcError>>,
    },
    CreateAnswer {
        options: AnswerOptions,
        reply: oneshot::Sender<Result<ImpSessionDescription, RtcError>>,
    },
    SetLocalDescription {
        desc: ImpSessionDescription,
        reply: oneshot::Sender<Result<(), RtcError>>,
    },
    SetRemoteDescription {
        desc: ImpSessionDescription,
        reply: oneshot::Sender<Result<(), RtcError>>,
    },
    AddIceCandidate {
        candidate: ImpIceCandidate,
        reply: oneshot::Sender<Result<(), RtcError>>,
    },
    CreateDataChannel {
        label: String,
        init: DataChannelInit,
        reply: oneshot::Sender<Result<DataChannelHandle, RtcError>>,
    },
    AddTrack {
        params: AddTrackParams,
        reply: oneshot::Sender<Result<(String, u32), RtcError>>,
    },
    RemoveTrack {
        sender_id: String,
        reply: oneshot::Sender<Result<(), RtcError>>,
    },
    AddTransceiverForMedia {
        kind: MediaType,
        direction: RtpTransceiverDirection,
        reply: oneshot::Sender<Result<TransceiverInfo, RtcError>>,
    },
    WriteRtp {
        track_id: String,
        packet: RtpPacketData,
    },
    /// Register an unbounded channel that will receive every inbound RTP
    /// packet for the given remote track. Sent by the public wrapper when
    /// it sees a `RemoteTrack` event, before the user can construct a
    /// `NativeAudioStream`/`NativeVideoStream` for the track.
    RegisterReceiver {
        track_id: String,
        sender: mpsc::UnboundedSender<ReceivedRtpPacket>,
    },
    /// Snapshot the rtc state machine's statistics. The driver translates
    /// the rtc-crate's `RTCStatsReport` into the public-facing
    /// [`crate::stats::RtcStats`] enum and replies on the oneshot channel.
    GetStats {
        reply: oneshot::Sender<Result<Vec<crate::stats::RtcStats>, RtcError>>,
    },
    /// Request an ICE restart on the rtc state machine. The next offer
    /// generated by the driver will carry fresh ICE credentials.
    RestartIce {
        reply: oneshot::Sender<Result<(), RtcError>>,
    },
    Close,
}

/// Lightweight handle returned to [`PeerConnection::create_data_channel`]
/// callers. The full [`crate::data_channel::DataChannel`] is constructed by
/// the public wrapper, which stores the underlying [`super::data_channel::DataChannel`]
/// imp keyed by `id` so subsequent driver events can reach it.
#[derive(Clone, Debug)]
pub(crate) struct DataChannelHandle {
    pub(crate) id: i32,
    pub(crate) label: String,
}

/// Events emitted by the driver toward the public [`PeerConnection`]
/// wrapper. The wrapper updates its mirrored state and fires user
/// callbacks based on these.
pub(crate) enum PcEvent {
    ConnectionStateChange(PeerConnectionState),
    IceConnectionStateChange(IceConnectionState),
    IceGatheringStateChange(IceGatheringState),
    SignalingStateChange(SignalingState),
    IceCandidate(ImpIceCandidate),
    IceCandidateError(IceCandidateError),
    NegotiationNeeded,
    DataChannelOpen(u16),
    DataChannelClosing(u16),
    DataChannelClosed(u16),
    DataChannelError(u16),
    DataChannelBufferedAmountLow(u16),
    DataChannelBufferedAmountHigh(u16),
    /// Inbound data channel announced by the remote peer.
    RemoteDataChannel(DataChannelHandle),
    /// Inbound media track announced by the remote peer.
    ///
    /// Surfaced from `rtc`'s `RTCTrackEvent::OnOpen`, which fires when the
    /// first RTP packet for the receiver arrives. Carries the metadata the
    /// public wrapper needs to synthesise an [`RtpReceiver`] and dispatch
    /// the `on_track` callback.
    RemoteTrack {
        receiver_id: String,
        track_id: String,
        stream_ids: Vec<String>,
        ssrc: u32,
        kind: MediaType,
        /// Negotiated codec MIME type (e.g., "video/VP8", "video/H264").
        codec_mime: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

/// Owns the rtc-crate state machine and a [`UdpSocket`], driving the
/// sans-I/O event loop. Created by the factory, run as a tokio task, and
/// stopped by sending [`ControlCommand::Close`] (or dropping the command
/// channel).
pub(crate) struct RtcIoDriver {
    cmd_rx: mpsc::UnboundedReceiver<ControlCommand>,
    event_tx: mpsc::UnboundedSender<PcEvent>,
    rtc_pc: RTCPeerConnection<NoopInterceptor>,
    // UDP socket (and optional TCP streams) managed by the transport layer.
    transport: TransportManager,
    local_addr: std::net::SocketAddr,
    /// Maps the public string handle returned to callers to the rtc-crate
    /// `RTCRtpSenderId`. Used to translate a `RemoveTrack` request back to
    /// the underlying transceiver index.
    senders: HashMap<String, RTCRtpSenderId>,
    /// Maps the public track id (as supplied to `add_track`) to the
    /// matching rtc-crate sender id. Used by `WriteRtp` to look up the
    /// correct sender when ferrying encoded packets into the state
    /// machine.
    tracks: HashMap<String, RTCRtpSenderId>,
    /// Per-track inbound RTP queues, keyed by the public track id used
    /// in `RTCMessage::RtpPacket`. Populated by `RegisterReceiver` and
    /// drained by `drain_reads` whenever the rtc state machine has a
    /// decrypted packet ready.
    rtp_receivers: HashMap<String, mpsc::UnboundedSender<ReceivedRtpPacket>>,
    /// Whether the local UDP socket address has already been registered
    /// with the rtc state machine as an ICE host candidate. The rtc crate
    /// is sans-I/O and does not perform candidate gathering itself, so we
    /// have to inject the host candidate explicitly. We do this on the
    /// first successful `set_local_description` so the candidate ends up
    /// associated with the right ICE generation.
    host_candidate_added: bool,
    /// Track IDs for which the first `do_write_rtp` call has been logged.
    rtp_write_logged: HashSet<String>,
    /// Track IDs for which an SSRC mismatch between the pipeline and the
    /// rtc crate has already been logged.  Deduplicated to avoid log spam.
    rtp_ssrc_mismatch_logged: HashSet<String>,
    /// Last observed ICE connection state (updated in drain_events).
    last_ice_state: RTCIceConnectionState,
}

impl RtcIoDriver {
    pub(crate) fn new(
        rtc_pc: RTCPeerConnection<NoopInterceptor>,
        transport: TransportManager,
        local_addr: std::net::SocketAddr,
        cmd_rx: mpsc::UnboundedReceiver<ControlCommand>,
        event_tx: mpsc::UnboundedSender<PcEvent>,
    ) -> Self {
        Self {
            cmd_rx,
            event_tx,
            rtc_pc,
            transport,
            local_addr,
            senders: HashMap::new(),
            tracks: HashMap::new(),
            rtp_receivers: HashMap::new(),
            host_candidate_added: false,
            rtp_write_logged: HashSet::new(),
            rtp_ssrc_mismatch_logged: HashSet::new(),
            last_ice_state: RTCIceConnectionState::Unspecified,
        }
    }

    /// Register the driver's UDP socket address with the rtc state
    /// machine as an ICE host candidate.
    ///
    /// `rtc-ice` is a sans-I/O library: it does not bind sockets nor
    /// gather candidates on its own. Without this call the connection's
    /// `candidate_pairs` collection stays empty and the first connection
    /// check log-spams `pingAllCandidates called with no candidate pairs`.
    /// Idempotent: only runs once per driver lifetime.
    fn add_host_candidate(&mut self) -> Result<(), RtcError> {
        if self.host_candidate_added {
            return Ok(());
        }
        let candidate = CandidateHostConfig {
            base_config: CandidateConfig {
                network: "udp".to_owned(),
                address: self.local_addr.ip().to_string(),
                port: self.local_addr.port(),
                component: 1,
                ..Default::default()
            },
            ..Default::default()
        }
        .new_candidate_host()
        .map_err(internal_err)?;
        let init = RTCIceCandidate::from(&candidate)
            .to_json()
            .map_err(internal_err)?;
        self.rtc_pc.add_local_candidate(init).map_err(internal_err)?;
        self.host_candidate_added = true;
        println!("[RtcIoDriver] added host candidate: {}:{}", self.local_addr.ip(), self.local_addr.port());
        log::info!(
            "rtc_io_driver: added local host ICE candidate {}:{}",
            self.local_addr.ip(),
            self.local_addr.port()
        );
        Ok(())
    }

    /// Main event loop.  Ordering is aligned with the proven tdnis-ohos
    /// loop so that STUN responses are sent in the same iteration that
    /// received them, avoiding ICE connectivity stalls:
    ///
    ///   1. `select!` – recv_from (20ms timeout), commands, timer expiry
    ///   2. `handle_timeout`  – drive ICE/DTLS state machine EVERY iteration
    ///   3. `drain_events`    – forward state-change events
    ///   4. `drain_reads`     – forward decrypted RTP to per-track queues
    ///   5. `drain_writes`    – poll_write + send_to (STUN responses zero-latency)
    pub(crate) async fn run(mut self) {
        log::info!(
            "[RtcIoDriver] run() started, local_addr={}",
            self.local_addr
        );
        let mut buf = vec![0u8; RECV_BUF_SIZE];
        let mut loop_count: u64 = 0;

        loop {
            loop_count += 1;

            // 1) Compute timeout, capped to IDLE_TICK for responsiveness.
            //    Unlike the old code, we NEVER skip the select! block — even
            //    when delay.is_zero(), we still enter select! so recv_from
            //    can drain pending STUN/DTLS packets.
            let delay = self
                .rtc_pc
                .poll_timeout()
                .map(|instant| instant.saturating_duration_since(Instant::now()))
                .unwrap_or(IDLE_TICK);
            let delay = delay.min(IDLE_TICK);

            // 2) Wait for next stimulus: incoming UDP packet, control command,
            //    or timer expiry.
            let sleep = tokio::time::sleep(delay);
            tokio::pin!(sleep);

            let should_break = tokio::select! {
                biased;

                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(ControlCommand::Close) => {
                            log::info!(
                                "[RtcIoDriver] received Close command after {} loops, shutting down",
                                loop_count
                            );
                            let _ = self.rtc_pc.close();
                            self.drain_writes().await;
                            self.drain_events();
                            true // should_break
                        }
                        None => {
                            log::info!(
                                "[RtcIoDriver] cmd_rx channel closed (all senders dropped) after {} loops, shutting down",
                                loop_count
                            );
                            let _ = self.rtc_pc.close();
                            self.drain_writes().await;
                            self.drain_events();
                            true // should_break
                        }
                        Some(cmd) => {
                            let result = std::panic::catch_unwind(
                                std::panic::AssertUnwindSafe(|| {
                                    self.handle_command(cmd);
                                }),
                            );
                            if let Err(e) = result {
                                let msg = if let Some(s) = e.downcast_ref::<&str>() {
                                    s.to_string()
                                } else if let Some(s) = e.downcast_ref::<String>() {
                                    s.clone()
                                } else {
                                    "unknown panic payload".to_string()
                                };
                                log::error!(
                                    "[RtcIoDriver] handle_command panicked (loop #{}): {}. Driver continues.",
                                    loop_count, msg
                                );
                            }
                            false // should_break
                        }
                    }
                }

                recv = self.transport.recv(&mut buf) => {
                    match recv {
                        Some(result) => {
                            let msg = TaggedBytesMut {
                                now: Instant::now(),
                                transport: TransportContext {
                                    local_addr: match result.protocol {
                                        TransportProtocol::UDP => self.local_addr,
                                        TransportProtocol::TCP => self.transport.local_tcp_addr.unwrap_or(self.local_addr),
                                    },
                                    peer_addr: result.peer_addr,
                                    transport_protocol: result.protocol,
                                    ecn: None,
                                },
                                message: BytesMut::from(&buf[..result.n]),
                            };
                            if let Err(err) = self.rtc_pc.handle_read(msg) {
                                log::warn!("rtc handle_read failed: {err}");
                            }
                            println!("[RtcIoDriver] handle_read: {} bytes from {} via {:?}", result.n, result.peer_addr, result.protocol);
                        }
                        None => {
                            // No data on any transport — proceed to timeout.
                        }
                    }
                    false // should_break
                }

                _ = &mut sleep => {
                    false // should_break
                }
            };

            if should_break {
                break;
            }

            // 3) Drive the ICE/DTLS state machine timers EVERY iteration.
            //    This must run unconditionally — skipping it (as the old
            //    delay.is_zero() fast-path did via `continue`) prevents the
            //    ICE agent from advancing connectivity checks to the
            //    "Connected" state.
            if let Err(err) = self.rtc_pc.handle_timeout(Instant::now()) {
                log::warn!("rtc handle_timeout failed: {err}");
            }
            if loop_count % 100 == 0 {
                println!("[RtcIoDriver] loop_count={}, ICE state: {:?}", loop_count, self.last_ice_state);
            }

            // 4) Drain protocol events (state changes, ICE candidates, ...)
            self.drain_events();

            // 5) Drain decrypted inbound RTP packets into per-track queues
            self.drain_reads();

            // 6) Drain outgoing packets (poll_write + send_to).
            //    Running this AFTER handle_timeout means STUN responses
            //    produced by this iteration's ICE state machine tick are
            //    sent with zero latency, matching tdnis-ohos behaviour.
            self.drain_writes().await;
        }

        log::info!(
            "[RtcIoDriver] run() exiting after {} loops, tracks={}, senders={}",
            loop_count,
            self.tracks.len(),
            self.senders.len()
        );
    }

    // ---- Sans-I/O pumping ---------------------------------------------------

    async fn drain_writes(&mut self) {
        let mut pkt_count: u64 = 0;
        let mut total_bytes: usize = 0;
        while let Some(out) = self.rtc_pc.poll_write() {
            let TaggedBytesMut { transport, message, .. } = out;
            pkt_count += 1;
            total_bytes += message.len();
            // Skip address family mismatches: ICE agent may produce IPv6 target
            // while socket is bound to IPv4, causing send_to to silently fail.
            if self.local_addr.is_ipv4() != transport.peer_addr.is_ipv4() {
                log::warn!(
                    "[RtcIoDriver] skipping address family mismatch: local={} peer={}",
                    self.local_addr, transport.peer_addr
                );
                continue;
            }
            if let Err(err) = self.transport.send_to(transport.peer_addr, &message, transport.transport_protocol).await {
                log::warn!(
                    "{:?} send_to {} failed: {err}",
                    transport.transport_protocol,
                    transport.peer_addr
                );
            }
            println!("[RtcIoDriver] send_to({:?}): {} bytes to {}", transport.transport_protocol, message.len(), transport.peer_addr);
        }
        if pkt_count > 0 {
            log::info!(
                "[RtcIoDriver] drain_writes: {} pkts ({} bytes) sent",
                pkt_count, total_bytes
            );
        }
    }

    fn drain_events(&mut self) {
        while let Some(evt) = self.rtc_pc.poll_event() {
            // The OnTrack mapping needs to query the receiver's kind via
            // the peer connection (it isn't carried on the event itself),
            // so it can't go through the stateless `map_rtc_event` helper.
            if let RTCPeerConnectionEvent::OnTrack(RTCTrackEvent::OnOpen(init)) = &evt {
                let (kind, codec_mime) = self
                    .rtc_pc
                    .rtp_receiver(init.receiver_id)
                    .map(|r| {
                        let track = r.track();
                        let kind = match track.kind() {
                            RtpCodecKind::Audio => MediaType::Audio,
                            RtpCodecKind::Video => MediaType::Video,
                            _ => MediaType::Unsupported,
                        };

                        // The negotiated codec for THIS particular incoming
                        // stream is bound to the SSRC at the moment the first
                        // RTP packet arrives (see endpoint::set_codec_by_ssrc
                        // in rtc-patched). Look it up by SSRC rather than via
                        // `codec_preferences()` — the latter returns the full
                        // negotiated codec list ordered by remote SDP, whose
                        // first entry may differ from the codec actually used
                        // on this stream (e.g. remote lists H264 first but
                        // sends VP8 on PT 96).
                        let mime = track
                            .codings()
                            .iter()
                            .find(|c| {
                                c.rtp_coding_parameters.ssrc == Some(init.ssrc)
                            })
                            .map(|c| c.codec.mime_type.clone())
                            .filter(|m| !m.is_empty());

                        log::info!(
                            "drain_events OnTrack: receiver_id={:?}, kind={:?}, ssrc={}, \
                             codec_mime={:?} (resolved via track.codings by ssrc)",
                            init.receiver_id, kind, init.ssrc, mime
                        );

                        (kind, mime)
                    })
                    .unwrap_or((MediaType::Unsupported, None));
                log::info!(
                    "drain_events RemoteTrack: track_id={}, kind={:?}, codec_mime={:?}",
                    init.track_id, kind, codec_mime
                );
                let pc_event = PcEvent::RemoteTrack {
                    receiver_id: format!("recv-{}", RTCRtpTransceiverId::from(init.receiver_id)),
                    track_id: init.track_id.clone(),
                    stream_ids: init.stream_ids.clone(),
                    ssrc: init.ssrc,
                    kind,
                    codec_mime,
                };
                if self.event_tx.send(pc_event).is_err() {
                    return;
                }
                continue;
            }

            for mapped in map_rtc_event(evt) {
                // Track last ICE connection state for periodic diagnostics
                if let PcEvent::IceConnectionStateChange(ref state) = mapped {
                    self.last_ice_state = match state {
                        IceConnectionState::New => RTCIceConnectionState::New,
                        IceConnectionState::Checking => RTCIceConnectionState::Checking,
                        IceConnectionState::Connected => RTCIceConnectionState::Connected,
                        IceConnectionState::Completed => RTCIceConnectionState::Completed,
                        IceConnectionState::Disconnected => RTCIceConnectionState::Disconnected,
                        IceConnectionState::Failed => RTCIceConnectionState::Failed,
                        IceConnectionState::Closed => RTCIceConnectionState::Closed,
                        IceConnectionState::Max => RTCIceConnectionState::Unspecified,
                    };
                }
                if self.event_tx.send(mapped).is_err() {
                    // Public wrapper has gone away; nothing else to do.
                    return;
                }
            }
        }
    }

    /// Drain every RTP packet the rtc state machine has decrypted and
    /// forward it to the per-track queue registered via
    /// [`ControlCommand::RegisterReceiver`]. Packets for tracks that have
    /// no registered consumer are dropped (the consumer hasn't been
    /// constructed yet, or has already gone away).
    fn drain_reads(&mut self) {
        while let Some(msg) = self.rtc_pc.poll_read() {
            match msg {
                RTCMessage::RtpPacket(track_id, packet) => {
                    let Some(tx) = self.rtp_receivers.get(&track_id) else {
                        log::trace!("drain_reads: no receiver for track {track_id}");
                        continue;
                    };
                    let received = ReceivedRtpPacket {
                        payload_type: packet.header.payload_type,
                        sequence_number: packet.header.sequence_number,
                        timestamp: packet.header.timestamp,
                        ssrc: packet.header.ssrc,
                        marker: packet.header.marker,
                        payload: packet.payload.to_vec(),
                        track_id,
                    };
                    if tx.send(received).is_err() {
                        // Receiver gone; drop the queue so subsequent
                        // packets stop allocating.
                        let dead = self
                            .rtp_receivers
                            .iter()
                            .find(|(_, s)| s.is_closed())
                            .map(|(k, _)| k.clone());
                        if let Some(k) = dead {
                            self.rtp_receivers.remove(&k);
                        }
                    }
                }
                // RTCP / data-channel messages flow through their own
                // pipelines and are surfaced as `PcEvent`s instead.
                RTCMessage::RtcpPacket(_, _) | RTCMessage::DataChannelMessage(_, _) => {}
            }
        }
    }

    // ---- Command dispatch ---------------------------------------------------

    fn handle_command(&mut self, cmd: ControlCommand) {
        match cmd {
            ControlCommand::CreateOffer { options, reply } => {
                let _ = reply.send(self.do_create_offer(options));
            }
            ControlCommand::CreateAnswer { options, reply } => {
                let _ = reply.send(self.do_create_answer(options));
            }
            ControlCommand::SetLocalDescription { desc, reply } => {
                let _ = reply.send(self.do_set_local_description(desc));
            }
            ControlCommand::SetRemoteDescription { desc, reply } => {
                let _ = reply.send(self.do_set_remote_description(desc));
            }
            ControlCommand::AddIceCandidate { candidate, reply } => {
                let _ = reply.send(self.do_add_ice_candidate(candidate));
            }
            ControlCommand::CreateDataChannel { label, init, reply } => {
                let _ = reply.send(self.do_create_data_channel(label, init));
            }
            ControlCommand::AddTrack { params, reply } => {
                let _ = reply.send(self.do_add_track(params));
            }
            ControlCommand::RemoveTrack { sender_id, reply } => {
                let _ = reply.send(self.do_remove_track(&sender_id));
            }
            ControlCommand::AddTransceiverForMedia { kind, direction, reply } => {
                let _ = reply.send(self.do_add_transceiver_for_media(kind, direction));
            }
            ControlCommand::WriteRtp { track_id, packet } => {
                self.do_write_rtp(&track_id, packet);
            }
            ControlCommand::RegisterReceiver { track_id, sender } => {
                self.rtp_receivers.insert(track_id, sender);
            }
            ControlCommand::GetStats { reply } => {
                let _ = reply.send(self.do_get_stats());
            }
            ControlCommand::RestartIce { reply } => {
                let _ = reply.send(self.do_restart_ice());
            }
            ControlCommand::Close => {
                // Handled in the loop's `select!` arm before we enter this
                // function; reach this branch only if we somehow get here
                // with a non-final close. Still attempt graceful close.
                let _ = self.rtc_pc.close();
            }
        }
    }

    // ---- do_* implementations ----------------------------------------------

    fn do_create_offer(
        &mut self,
        options: OfferOptions,
    ) -> Result<ImpSessionDescription, RtcError> {
        let opts = RTCOfferOptions { ice_restart: options.ice_restart, ..Default::default() };
        let desc = self
            .rtc_pc
            .create_offer(Some(opts))
            .map_err(internal_err)?;
        Ok(rtc_desc_to_imp(desc))
    }

    fn do_create_answer(
        &mut self,
        _options: AnswerOptions,
    ) -> Result<ImpSessionDescription, RtcError> {
        // The public layer's `AnswerOptions` is a placeholder today; the
        // rtc crate accepts `None` to use defaults.
        let desc = self
            .rtc_pc
            .create_answer(Some(RTCAnswerOptions::default()))
            .map_err(internal_err)?;
        Ok(rtc_desc_to_imp(desc))
    }

    fn do_set_local_description(&mut self, desc: ImpSessionDescription) -> Result<(), RtcError> {
        let rtc_desc = imp_desc_to_rtc(&desc)?;
        self.rtc_pc
            .set_local_description(rtc_desc)
            .map_err(internal_err)?;
        // The rtc crate is sans-I/O and never gathers candidates by
        // itself; inject our bound UDP address as a host candidate now
        // that ICE credentials are committed. Failures are non-fatal:
        // remote candidates may still allow the connection to progress
        // (e.g. via prflx).
        if let Err(err) = self.add_host_candidate() {
            log::warn!("rtc_io_driver: add_host_candidate failed: {}", err.message);
        }
        Ok(())
    }

    fn do_set_remote_description(&mut self, desc: ImpSessionDescription) -> Result<(), RtcError> {
        let rtc_desc = imp_desc_to_rtc(&desc)?;
        self.rtc_pc
            .set_remote_description(rtc_desc)
            .map_err(internal_err)
    }

    fn do_add_ice_candidate(&mut self, candidate: ImpIceCandidate) -> Result<(), RtcError> {
        println!("[RtcIoDriver] add_remote_candidate: {}", candidate.candidate);
        let init = RTCIceCandidateInit {
            candidate: candidate.candidate.clone(),
            sdp_mid: Some(candidate.sdp_mid.clone()),
            sdp_mline_index: Some(candidate.sdp_mline_index as u16),
            username_fragment: None,
            url: None,
        };
        self.rtc_pc.add_remote_candidate(init).map_err(internal_err)
    }

    fn do_create_data_channel(
        &mut self,
        label: String,
        init: DataChannelInit,
    ) -> Result<DataChannelHandle, RtcError> {
        let mut rtc_init = RTCDataChannelInit {
            ordered: init.ordered,
            // The public layer keeps this as `Option<i32>` (milliseconds);
            // clamp into the rtc crate's `Option<u16>` representation.
            max_packet_life_time: init
                .max_retransmit_time
                .filter(|v| *v >= 0)
                .map(|v| v.min(u16::MAX as i32) as u16),
            max_retransmits: init
                .max_retransmits
                .filter(|v| *v >= 0)
                .map(|v| v.min(u16::MAX as i32) as u16),
            protocol: init.protocol.clone(),
            negotiated: None,
        };
        if init.negotiated && init.id >= 0 {
            rtc_init.negotiated = Some(init.id.min(u16::MAX as i32) as u16);
        }
        let dc = self
            .rtc_pc
            .create_data_channel(&label, Some(rtc_init))
            .map_err(internal_err)?;
        let id = dc.id();
        // `dc` borrows the peer connection mutably; drop it before the
        // function returns so subsequent commands can borrow `self.rtc_pc`.
        drop(dc);
        Ok(DataChannelHandle { id: id as i32, label })
    }

    // ---- Track / transceiver implementations -------------------------------

    /// Construct an [`RtcMediaStreamTrack`] from the public-layer
    /// [`AddTrackParams`] and hand it to the rtc-crate, returning the
    /// stringified sender id that the public wrapper exposes to callers.
    fn do_add_track(&mut self, params: AddTrackParams) -> Result<(String, u32), RtcError> {
        log::info!(
            "do_add_track: track_id={} kind={:?} mime={} stream_id={}",
            params.track_id, params.kind, params.codec_mime, params.stream_id
        );
        let kind = media_type_to_codec_kind(params.kind)?;

        // For video tracks, register both H264 and VP8 as encodings so the
        // SDP offer includes both codecs.  The *order* depends on whether
        // this device actually has an H.264 hardware encoder:
        //   - H264 available (e.g. Mate X5) → H264 first, VP8 fallback
        //   - H264 unavailable                → VP8 first, H264 second
        // This ensures the SDP always lists the actually-used codec first,
        // preventing the SFU from seeing a codec mismatch at runtime.
        let encodings: Vec<RTCRtpEncodingParameters> = if params.kind == MediaType::Video {
            let h264_codec = RTCRtpCodec {
                mime_type: "video/H264".to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line:
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=64001e"
                        .to_owned(),
                rtcp_feedback: Vec::new(),
            };
            let vp8_codec = RTCRtpCodec {
                mime_type: "video/VP8".to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: String::new(),
                rtcp_feedback: Vec::new(),
            };
            let vp8_codec2 = RTCRtpCodec {
                mime_type: "video/VP8".to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: String::new(),
                rtcp_feedback: Vec::new(),
            };
            let ssrc = if params.ssrc != 0 {
                Some(params.ssrc)
            } else {
                None
            };
            let h264_enc = RTCRtpEncodingParameters {
                rtp_coding_parameters: RTCRtpCodingParameters {
                    ssrc,
                    ..Default::default()
                },
                active: true,
                codec: h264_codec,
                ..Default::default()
            };
            let vp8_enc = RTCRtpEncodingParameters {
                rtp_coding_parameters: RTCRtpCodingParameters::default(),
                active: true,
                codec: vp8_codec,
                ..Default::default()
            };
            let vp8_enc_with_ssrc = RTCRtpEncodingParameters {
                rtp_coding_parameters: RTCRtpCodingParameters {
                    ssrc,
                    ..Default::default()
                },
                active: true,
                codec: vp8_codec2,
                ..Default::default()
            };
            #[cfg(any(target_env = "ohos", target_os = "android"))]
            {
            if H264Encoder::is_available() {
                log::info!("do_add_track: H264 encoder available → H264 first in SDP");
                vec![h264_enc, vp8_enc]
            } else {
                log::info!("do_add_track: H264 encoder NOT available → VP8 first in SDP");
                vec![vp8_enc, h264_enc]
            }
            }
            #[cfg(not(any(target_env = "ohos", target_os = "android")))]
            {
                // No H264 HW encoder on non-OHOS/non-Android → VP8 first
                log::info!("do_add_track: non-OHOS/non-Android target → VP8 first in SDP");
                vec![vp8_enc, h264_enc]
            }
        } else {
            let codec = RTCRtpCodec {
                mime_type: params.codec_mime.clone(),
                clock_rate: params.clock_rate,
                channels: params.channels,
                sdp_fmtp_line: String::new(),
                rtcp_feedback: Vec::new(),
            };
            vec![RTCRtpEncodingParameters {
                rtp_coding_parameters: RTCRtpCodingParameters {
                    ssrc: if params.ssrc != 0 { Some(params.ssrc) } else { None },
                    ..Default::default()
                },
                active: true,
                codec,
                ..Default::default()
            }]
        };

        let track = RtcMediaStreamTrack::new(
            params.stream_id.clone(),
            params.track_id.clone(),
            params.label.clone(),
            kind,
            encodings,
        );
        // 先尝试 add_track（复用 setRemoteDescription 创建的 transceiver，共享 ICE transport）
        // 失败再 fallback 到 add_transceiver_from_track（创建新 transceiver）
        let sender_id = match self.rtc_pc.add_track(track.clone()) {
            Ok(sid) => {
                log::info!("do_add_track: add_track succeeded, reused existing transceiver, sender={:?}", sid);
                sid
            }
            Err(e) => {
                log::warn!("do_add_track: add_track failed ({}), falling back to add_transceiver_from_track", e);
                let init = RTCRtpTransceiverInit {
                    direction: RTCRtpTransceiverDirection::Sendonly,
                    streams: vec![],
                    send_encodings: vec![],
                };
                let tid = self.rtc_pc.add_transceiver_from_track(track, Some(init)).map_err(|e| {
                    log::error!("do_add_track: add_transceiver_from_track failed for {}: {}", params.track_id, e);
                    internal_err(e)
                })?;
                self.rtc_pc.rtp_transceiver(tid)
                    .and_then(|t| t.sender())
                    .ok_or_else(|| RtcError {
                        error_type: RtcErrorType::Internal,
                        message: format!("do_add_track: no sender on transceiver for {}", params.track_id),
                    })?
            }
        };
        
        // Get the actual SSRC assigned by the rtc crate
        let actual_ssrc = self.rtc_pc.rtp_sender(sender_id)
            .and_then(|s| s.track().ssrcs().next())
            .unwrap_or(params.ssrc);
        
        if actual_ssrc != params.ssrc {
            log::warn!("do_add_track: rtc crate assigned SSRC {} instead of requested {}", actual_ssrc, params.ssrc);
        }
        
        let handle = format!("sender-{}", RTCRtpTransceiverId::from(sender_id));
        self.senders.insert(handle.clone(), sender_id);
        self.tracks.insert(params.track_id.clone(), sender_id);
        let connection_state = String::from("connected"); // TODO: expose connection_state from rtc
        let ice_state = String::from("Unknown"); // TODO: update to match rtc crate API
        log::info!(
            "do_add_track: success track_id={} sender={} ssrc={} direction=Sendonly, PC state={:?}, ICE={}",
            params.track_id, handle, actual_ssrc, connection_state, ice_state
        );
        Ok((handle, actual_ssrc))
    }

    /// Look up the underlying `RTCRtpSenderId` and stop sending media on
    /// the associated transceiver.
    fn do_remove_track(&mut self, sender_id: &str) -> Result<(), RtcError> {
        let rtc_sender_id = self.senders.remove(sender_id).ok_or_else(|| RtcError {
            error_type: RtcErrorType::InvalidState,
            message: format!("sender {sender_id} not found"),
        })?;
        // Drop any track mapping that pointed at this sender so future
        // WriteRtp calls bounce instead of silently misrouting packets.
        self.tracks.retain(|_, sid| *sid != rtc_sender_id);
        self.rtc_pc
            .remove_track(rtc_sender_id)
            .map_err(internal_err)
    }

    /// Create a transceiver without an attached local track. Used when
    /// the public wrapper just wants to declare an `m=` section of a
    /// given media kind/direction.
    fn do_add_transceiver_for_media(
        &mut self,
        kind: MediaType,
        direction: RtpTransceiverDirection,
    ) -> Result<TransceiverInfo, RtcError> {
        let codec_kind = media_type_to_codec_kind(kind)?;
        let rtc_direction = imp_direction_to_rtc(direction);
        // rtc crate's add_transceiver_from_kind rejects Sendrecv/Sendonly
        // when send_encodings is empty (ErrInvalidDirection).
        let send_encodings = if rtc_direction.has_send() {
            vec![RTCRtpEncodingParameters::default()]
        } else {
            vec![]
        };
        let init = RTCRtpTransceiverInit {
            direction: rtc_direction,
            streams: Vec::new(),
            send_encodings,
        };
        let tid = self
            .rtc_pc
            .add_transceiver_from_kind(codec_kind, Some(init))
            .map_err(internal_err)?;
        let mid = self
            .rtc_pc
            .rtp_transceiver(tid)
            .and_then(|t| t.mid().clone());
        Ok(TransceiverInfo {
            mid,
            sender_id: format!("sender-{tid}"),
            receiver_id: format!("recv-{tid}"),
            direction,
        })
    }

    /// Fire-and-forget RTP write. Errors are logged but never bubbled up;
    /// callers (typically the encoder thread) cannot meaningfully react
    /// to per-packet failures.
    fn do_write_rtp(&mut self, track_id: &str, packet_data: RtpPacketData) {
        let Some(sender_id) = self.tracks.get(track_id).copied() else {
            log::warn!("write_rtp: unknown track id {track_id}");
            return;
        };

        // ── Resolve track label and actual SSRC from the rtc crate ──────────
        // The RtpSendPipeline is created *before* SDP negotiation, so its SSRC
        // may be stale.  The rtc crate's sender knows the SSRC that was actually
        // written into the SDP offer/answer.  Always use that one so the SFU can
        // correctly bind incoming RTP to the declared track.
        let rtp_sender = self.rtc_pc.rtp_sender(sender_id);
        let track_label = rtp_sender
            .as_ref()
            .map(|s| s.track().track_id().clone())
            .unwrap_or_else(|| track_id.to_string());

        let actual_ssrc = rtp_sender
            .and_then(|s| s.track().ssrcs().next())
            .unwrap_or(packet_data.ssrc);

        if actual_ssrc != packet_data.ssrc {
            // Log once per track to avoid spamming the log buffer.
            if self.rtp_ssrc_mismatch_logged.insert(track_id.to_string()) {
                log::warn!(
                    "do_write_rtp: SSRC mismatch for track={}: pipeline had {}, rtc crate has {}. Fixing.",
                    track_id, packet_data.ssrc, actual_ssrc
                );
            }
        }

        let ssrc = actual_ssrc;
        let payload_type = packet_data.payload_type;
        let packet = rtc_rtp::Packet {
            header: rtc_rtp::Header {
                version: 2,
                padding: false,
                extension: false,
                marker: packet_data.marker,
                payload_type,
                sequence_number: packet_data.sequence_number,
                timestamp: packet_data.timestamp,
                ssrc,
                csrc: Vec::new(),
                extension_profile: 0,
                extensions: Vec::new(),
                extensions_padding: 0,
            },
            payload: Bytes::from(packet_data.payload),
        };

        if let Err(err) = self
            .rtc_pc
            .handle_write(RTCMessage::RtpPacket(track_label, packet))
        {
            if self.rtp_write_logged.insert(track_id.to_string()) {
                log::info!("do_write_rtp: first write for track_id={} FAILED: {err}", track_id);
            }
            log::warn!("rtc handle_write(RtpPacket) failed: {err}");
        } else if self.rtp_write_logged.insert(track_id.to_string()) {
            log::info!("do_write_rtp: first write for track_id={} succeeded (ssrc={}, pt={})", track_id, ssrc, payload_type);
        }
    }

    // ---- Stats / ICE restart -----------------------------------------------

    /// Snapshot the rtc state machine's statistics report and translate
    /// each entry to the public [`crate::stats::RtcStats`] representation.
    ///
    /// The two enums share the W3C kebab-case discriminator and (for the
    /// fields they cover) camelCase field naming, so each entry is
    /// re-serialised through `serde_json` rather than mapped field by
    /// field. Entries the public layer doesn't model (or that fail to
    /// round-trip due to schema drift) are dropped with a debug log.
    fn do_get_stats(&mut self) -> Result<Vec<crate::stats::RtcStats>, RtcError> {
        use rtc::statistics::report::RTCStatsReportEntry;
        use rtc::statistics::StatsSelector;

        let report = self.rtc_pc.get_stats(Instant::now(), StatsSelector::None);
        let mut out = Vec::with_capacity(report.len());
        for entry in report.iter() {
            let value = match entry {
                RTCStatsReportEntry::PeerConnection(s) => serde_json::to_value(s),
                RTCStatsReportEntry::Transport(s) => serde_json::to_value(s),
                RTCStatsReportEntry::IceCandidatePair(s) => serde_json::to_value(s),
                RTCStatsReportEntry::LocalCandidate(s) => serde_json::to_value(s),
                RTCStatsReportEntry::RemoteCandidate(s) => serde_json::to_value(s),
                RTCStatsReportEntry::Certificate(s) => serde_json::to_value(s),
                RTCStatsReportEntry::Codec(s) => serde_json::to_value(s),
                RTCStatsReportEntry::DataChannel(s) => serde_json::to_value(s),
                RTCStatsReportEntry::InboundRtp(s) => serde_json::to_value(s),
                RTCStatsReportEntry::OutboundRtp(s) => serde_json::to_value(s),
                RTCStatsReportEntry::RemoteInboundRtp(s) => serde_json::to_value(s),
                RTCStatsReportEntry::RemoteOutboundRtp(s) => serde_json::to_value(s),
                RTCStatsReportEntry::AudioSource(s) => serde_json::to_value(s),
                RTCStatsReportEntry::VideoSource(s) => serde_json::to_value(s),
                RTCStatsReportEntry::AudioPlayout(s) => serde_json::to_value(s),
            };
            let value = match value {
                Ok(v) => v,
                Err(err) => {
                    log::debug!("get_stats: serialize {:?} failed: {err}", entry.stats_type());
                    continue;
                }
            };
            match serde_json::from_value::<crate::stats::RtcStats>(value) {
                Ok(stats) => out.push(stats),
                Err(err) => {
                    log::debug!(
                        "get_stats: skip entry {} ({:?}): {err}",
                        entry.id(),
                        entry.stats_type()
                    );
                }
            }
        }
        Ok(out)
    }

    /// Mark the rtc state machine for ICE restart. The actual restart
    /// happens on the next `create_offer` call (which the public wrapper
    /// is expected to issue after invoking `restart_ice`).
    fn do_restart_ice(&mut self) -> Result<(), RtcError> {
        self.rtc_pc.restart_ice();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

fn internal_err<E: std::fmt::Display>(err: E) -> RtcError {
    RtcError { error_type: RtcErrorType::Internal, message: err.to_string() }
}

fn invalid_sdp_err(msg: impl Into<String>) -> RtcError {
    RtcError { error_type: RtcErrorType::InvalidSdp, message: msg.into() }
}

/// Translate the public `MediaType` enum to the rtc-crate's `RtpCodecKind`.
///
/// `MediaType::Data` and `MediaType::Unsupported` are not valid for an RTP
/// track and are rejected with an `InvalidState` error.
fn media_type_to_codec_kind(kind: MediaType) -> Result<RtpCodecKind, RtcError> {
    match kind {
        MediaType::Audio => Ok(RtpCodecKind::Audio),
        MediaType::Video => Ok(RtpCodecKind::Video),
        _ => Err(RtcError {
            error_type: RtcErrorType::InvalidState,
            message: format!("media type {:?} cannot back an RTP track", kind),
        }),
    }
}

/// Convert the public transceiver direction to the rtc-crate's. `Stopped`
/// has no direct analogue and is mapped to `Inactive`, which is what the
/// rtc state machine uses to halt sending and receiving.
fn imp_direction_to_rtc(direction: RtpTransceiverDirection) -> RTCRtpTransceiverDirection {
    match direction {
        RtpTransceiverDirection::SendRecv => RTCRtpTransceiverDirection::Sendrecv,
        RtpTransceiverDirection::SendOnly => RTCRtpTransceiverDirection::Sendonly,
        RtpTransceiverDirection::RecvOnly => RTCRtpTransceiverDirection::Recvonly,
        RtpTransceiverDirection::Inactive | RtpTransceiverDirection::Stopped => {
            RTCRtpTransceiverDirection::Inactive
        }
    }
}

fn rtc_desc_to_imp(desc: RTCSessionDescription) -> ImpSessionDescription {
    ImpSessionDescription {
        sdp_type: match desc.sdp_type {
            RTCSdpType::Offer => SdpType::Offer,
            RTCSdpType::Answer => SdpType::Answer,
            RTCSdpType::Pranswer => SdpType::PrAnswer,
            RTCSdpType::Rollback => SdpType::Rollback,
            // The rtc crate models "no negotiation in progress" as
            // `Unspecified`; we surface this as `Rollback` because that's
            // the closest public-layer match (it carries an empty body).
            RTCSdpType::Unspecified => SdpType::Rollback,
        },
        sdp: desc.sdp,
    }
}

fn imp_desc_to_rtc(desc: &ImpSessionDescription) -> Result<RTCSessionDescription, RtcError> {
    let sdp = desc.sdp.clone();
    match desc.sdp_type {
        SdpType::Offer => RTCSessionDescription::offer(sdp).map_err(internal_err),
        SdpType::Answer => RTCSessionDescription::answer(sdp).map_err(internal_err),
        SdpType::PrAnswer => RTCSessionDescription::pranswer(sdp).map_err(internal_err),
        SdpType::Rollback => {
            // The rtc crate's public constructors don't expose a rollback
            // builder. Surface this so callers know to handle it at the
            // signalling layer instead.
            Err(invalid_sdp_err("SDP rollback is not supported by the OHOS backend"))
        }
    }
}

fn map_pc_state(state: RTCPeerConnectionState) -> PeerConnectionState {
    match state {
        RTCPeerConnectionState::Unspecified | RTCPeerConnectionState::New => {
            PeerConnectionState::New
        }
        RTCPeerConnectionState::Connecting => PeerConnectionState::Connecting,
        RTCPeerConnectionState::Connected => PeerConnectionState::Connected,
        RTCPeerConnectionState::Disconnected => PeerConnectionState::Disconnected,
        RTCPeerConnectionState::Failed => PeerConnectionState::Failed,
        RTCPeerConnectionState::Closed => PeerConnectionState::Closed,
    }
}

fn map_ice_conn_state(state: RTCIceConnectionState) -> IceConnectionState {
    match state {
        RTCIceConnectionState::Unspecified | RTCIceConnectionState::New => IceConnectionState::New,
        RTCIceConnectionState::Checking => IceConnectionState::Checking,
        RTCIceConnectionState::Connected => IceConnectionState::Connected,
        RTCIceConnectionState::Completed => IceConnectionState::Completed,
        RTCIceConnectionState::Disconnected => IceConnectionState::Disconnected,
        RTCIceConnectionState::Failed => IceConnectionState::Failed,
        RTCIceConnectionState::Closed => IceConnectionState::Closed,
    }
}

fn map_ice_gathering_state(state: RTCIceGatheringState) -> IceGatheringState {
    match state {
        RTCIceGatheringState::Unspecified | RTCIceGatheringState::New => IceGatheringState::New,
        RTCIceGatheringState::Gathering => IceGatheringState::Gathering,
        RTCIceGatheringState::Complete => IceGatheringState::Complete,
    }
}

fn map_signaling_state(state: RTCSignalingState) -> SignalingState {
    match state {
        RTCSignalingState::Stable | RTCSignalingState::Unspecified => SignalingState::Stable,
        RTCSignalingState::HaveLocalOffer => SignalingState::HaveLocalOffer,
        RTCSignalingState::HaveRemoteOffer => SignalingState::HaveRemoteOffer,
        RTCSignalingState::HaveLocalPranswer => SignalingState::HaveLocalPrAnswer,
        RTCSignalingState::HaveRemotePranswer => SignalingState::HaveRemotePrAnswer,
        RTCSignalingState::Closed => SignalingState::Closed,
    }
}

fn map_ice_candidate(evt: RTCPeerConnectionIceEvent) -> ImpIceCandidate {
    // The rtc crate's `RTCIceCandidate` carries individual fields; we
    // serialise it via `to_json()` to obtain a `candidate:` SDP attribute
    // suitable for trickle exchange. Failures are extremely rare at this
    // point (the candidate has just been gathered) but we degrade
    // gracefully by emitting an empty candidate.
    let init = evt.candidate.to_json().ok();
    ImpIceCandidate {
        sdp_mid: init.as_ref().and_then(|i| i.sdp_mid.clone()).unwrap_or_default(),
        sdp_mline_index: init
            .as_ref()
            .and_then(|i| i.sdp_mline_index)
            .map(|v| v as i32)
            .unwrap_or(0),
        candidate: init.map(|i| i.candidate).unwrap_or_default(),
    }
}

fn map_ice_candidate_error(evt: RTCPeerConnectionIceErrorEvent) -> IceCandidateError {
    IceCandidateError {
        address: evt.address,
        port: evt.port as i32,
        url: evt.url,
        error_code: evt.error_code as i32,
        error_text: evt.error_text,
    }
}

fn map_data_channel_event(evt: RTCDataChannelEvent) -> Vec<PcEvent> {
    match evt {
        RTCDataChannelEvent::OnOpen(id) => vec![PcEvent::DataChannelOpen(id)],
        RTCDataChannelEvent::OnError(id) => vec![PcEvent::DataChannelError(id)],
        RTCDataChannelEvent::OnClosing(id) => vec![PcEvent::DataChannelClosing(id)],
        RTCDataChannelEvent::OnClose(id) => vec![PcEvent::DataChannelClosed(id)],
        RTCDataChannelEvent::OnBufferedAmountLow(id) => {
            vec![PcEvent::DataChannelBufferedAmountLow(id)]
        }
        RTCDataChannelEvent::OnBufferedAmountHigh(id) => {
            vec![PcEvent::DataChannelBufferedAmountHigh(id)]
        }
    }
}

/// Map a single rtc-crate event into zero-or-more public-layer events.
fn map_rtc_event(evt: RTCPeerConnectionEvent) -> Vec<PcEvent> {
    match evt {
        RTCPeerConnectionEvent::OnNegotiationNeededEvent => vec![PcEvent::NegotiationNeeded],
        RTCPeerConnectionEvent::OnIceCandidateEvent(ice) => {
            vec![PcEvent::IceCandidate(map_ice_candidate(ice))]
        }
        RTCPeerConnectionEvent::OnIceCandidateErrorEvent(err) => {
            vec![PcEvent::IceCandidateError(map_ice_candidate_error(err))]
        }
        RTCPeerConnectionEvent::OnSignalingStateChangeEvent(state) => {
            vec![PcEvent::SignalingStateChange(map_signaling_state(state))]
        }
        RTCPeerConnectionEvent::OnIceConnectionStateChangeEvent(state) => {
            println!("[RtcIoDriver] ICE event: IceConnectionStateChange => {:?}", state);
            vec![PcEvent::IceConnectionStateChange(map_ice_conn_state(state))]
        }
        RTCPeerConnectionEvent::OnIceGatheringStateChangeEvent(state) => {
            println!("[RtcIoDriver] ICE event: IceGatheringStateChange => {:?}", state);
            vec![PcEvent::IceGatheringStateChange(map_ice_gathering_state(state))]
        }
        RTCPeerConnectionEvent::OnConnectionStateChangeEvent(state) => {
            vec![PcEvent::ConnectionStateChange(map_pc_state(state))]
        }
        RTCPeerConnectionEvent::OnDataChannel(dc) => map_data_channel_event(dc),
        // `OnOpen` is handled inline in `drain_events` because mapping
        // requires a borrow on the peer connection to determine the
        // track kind. The other variants don't currently carry through
        // to a public callback: `OnError`/`OnClosing`/`OnClose` will be
        // surfaced once the wrapper grows the corresponding hooks.
        RTCPeerConnectionEvent::OnTrack(_) => Vec::new(),
    }
}
