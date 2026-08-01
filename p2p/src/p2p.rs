//! P2P WebRTC 引擎 — 管理 PeerConnection 直连
//!
//! 不包含任何 MQTT/信令逻辑。上层负责：
//! 1. 通过 `create_offer()` / `create_answer()` 获取 SDP，用 MQTT 发送
//! 2. 收到远端 SDP 后调用 `set_remote_sdp()`
//! 3. 从 `poll_events()` 拿到 ICE 候选后，用 MQTT 发送
//! 4. 收到远端 ICE 后调用 `add_ice_candidate()`

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::block_on;
use crate::types::*;

// ============================================================
// p2p-native 类型转换
// ============================================================

mod conv {
    use crate::types::*;
    use libwebrtc::ice_candidate::IceCandidate as LwIce;
    use libwebrtc::peer_connection_factory::{IceServer as LwIceServer, RtcConfiguration};
    use libwebrtc::session_description::{SdpType as LwSdpType, SessionDescription as LwSdp};

    pub fn to_rtc_config(config: &P2pConfig) -> RtcConfiguration {
        RtcConfiguration {
            ice_servers: config
                .ice_servers
                .iter()
                .map(|s| LwIceServer {
                    urls: s.urls.clone(),
                    username: s.username.clone().unwrap_or_default(),
                    password: s.credential.clone().unwrap_or_default(),
                })
                .collect(),
            ..Default::default()
        }
    }

    pub fn to_lw_sdp(sdp: &SessionDescription) -> Result<LwSdp, RtcError> {
        let sdp_type = match sdp.sdp_type {
            SdpType::Offer => LwSdpType::Offer,
            SdpType::Answer => LwSdpType::Answer,
        };
        LwSdp::parse(&sdp.sdp, sdp_type).map_err(|e| RtcError::Sdp(e.to_string()))
    }

    pub fn from_lw_sdp(sdp: &LwSdp) -> SessionDescription {
        SessionDescription {
            sdp_type: match sdp.sdp_type() {
                LwSdpType::Offer => SdpType::Offer,
                LwSdpType::Answer => SdpType::Answer,
                _ => SdpType::Offer,
            },
            sdp: sdp.to_string(),
        }
    }

    pub fn to_lw_ice(c: &IceCandidate) -> Result<LwIce, RtcError> {
        LwIce::parse(&c.sdp_mid, c.sdp_m_line_index, &c.candidate)
            .map_err(|e| RtcError::Ice(e.to_string()))
    }

    pub fn from_lw_ice(c: &LwIce) -> IceCandidate {
        IceCandidate {
            sdp_mid: c.sdp_mid(),
            sdp_m_line_index: c.sdp_mline_index(),
            candidate: c.candidate(),
        }
    }

    pub fn to_p2p_state(s: libwebrtc::peer_connection::PeerConnectionState) -> P2pState {
        match s {
            libwebrtc::peer_connection::PeerConnectionState::New => P2pState::New,
            libwebrtc::peer_connection::PeerConnectionState::Connecting => P2pState::Connecting,
            libwebrtc::peer_connection::PeerConnectionState::Connected => P2pState::Connected,
            libwebrtc::peer_connection::PeerConnectionState::Disconnected => P2pState::Disconnected,
            libwebrtc::peer_connection::PeerConnectionState::Failed => P2pState::Failed,
            libwebrtc::peer_connection::PeerConnectionState::Closed => P2pState::Closed,
        }
    }

    pub fn map_err(e: libwebrtc::RtcError) -> RtcError {
        match e.error_type {
            libwebrtc::RtcErrorType::InvalidSdp => RtcError::Sdp(e.message),
            libwebrtc::RtcErrorType::InvalidState => RtcError::InvalidState {
                current: "unknown".into(),
                expected: "unknown".into(),
            },
            _ => RtcError::Internal(e.message),
        }
    }

}

// ============================================================
// 内部会话
// ============================================================

struct P2pSession {
    state: P2pState,
    /// 本地生成的待发送事件 (ICE 候选、状态变更等)
    /// 使用 Arc<Mutex<>> 以便 p2p-native 回调线程安全写入
    events: Arc<Mutex<Vec<EngineEvent>>>,
    pc: Option<libwebrtc::peer_connection::PeerConnection>,
    /// 本地音频源（通过 attach_audio 创建）
    audio_source: Option<libwebrtc::audio_source::native::NativeAudioSource>,
    /// 本地音频 track
    audio_track: Option<libwebrtc::audio_track::RtcAudioTrack>,
    /// 远端音频 track 存储（on_track 回调写入，take_remote_audio_track 取出）
    remote_audio_tracks: Arc<Mutex<HashMap<String, libwebrtc::audio_track::RtcAudioTrack>>>,
}

impl P2pSession {
    fn new() -> Self {
        Self {
            state: P2pState::New,
            events: Arc::new(Mutex::new(Vec::new())),
            pc: None,
            audio_source: None,
            audio_track: None,
            remote_audio_tracks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn transition(&mut self, new_state: P2pState, handle: PeerHandle) {
        if self.state != new_state {
            log::info!("[P2P] {:?} {:?} → {:?}", handle, self.state, new_state);
            self.state = new_state;
            self.events.lock().unwrap().push(EngineEvent::P2pStateChange {
                handle,
                state: new_state,
            });
        }
    }
}

// ============================================================
// P2P 管理器
// ============================================================

pub(crate) struct P2pManager {
    sessions: HashMap<PeerHandle, P2pSession>,
    next_handle: u64,
    factory: libwebrtc::peer_connection_factory::PeerConnectionFactory,
}

impl P2pManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            next_handle: 1,
            factory: libwebrtc::peer_connection_factory::PeerConnectionFactory::default(),
        }
    }

    // ---- 创建/销毁 ----

    /// 创建一个 P2P PeerConnection，返回句柄
    pub fn create(&mut self, config: &P2pConfig) -> PeerHandle {
        let handle = PeerHandle(self.next_handle);
        self.next_handle += 1;
        let mut session = P2pSession::new();

        {
            use conv::*;
            use libwebrtc::peer_connection::{
                AnswerOptions, OfferOptions, OnConnectionChange, OnIceCandidate,
            };

            let rtc_config = to_rtc_config(config);
            match self.factory.create_peer_connection(rtc_config) {
                Ok(pc) => {
                    // ── ICE 候选回调 ──
                    let events = session.events.clone();
                    let cb_handle = handle;
                    pc.on_ice_candidate(Some(Box::new(move |c| {
                        if let Ok(mut evts) = events.lock() {
                            evts.push(EngineEvent::P2pIceCandidate {
                                handle: cb_handle,
                                candidate: from_lw_ice(&c),
                            });
                        }
                    }) as OnIceCandidate));

                    // ── 连接状态回调 ──
                    let events = session.events.clone();
                    let cb_handle = handle;
                    pc.on_connection_state_change(Some(Box::new(move |state| {
                        if let Ok(mut evts) = events.lock() {
                            evts.push(EngineEvent::P2pStateChange {
                                handle: cb_handle,
                                state: to_p2p_state(state),
                            });
                        }
                    }) as OnConnectionChange));

                    // ── 远端 track 回调 ──
                    let events = session.events.clone();
                    let remote_audio_tracks = session.remote_audio_tracks.clone();
                    let cb_handle = handle;
                    pc.on_track(Some(Box::new(move |track_event: libwebrtc::peer_connection::TrackEvent| {
                        let track_id = track_event.track.id();
                        let kind = match &track_event.track {
                            libwebrtc::media_stream_track::MediaStreamTrack::Audio(audio_track) => {
                                // 存储远端音频 track，供后续创建 NativeAudioStream
                                if let Ok(mut tracks) = remote_audio_tracks.lock() {
                                    tracks.insert(track_id.clone(), audio_track.clone());
                                }
                                MediaKind::Audio
                            }
                            libwebrtc::media_stream_track::MediaStreamTrack::Video(_) => {
                                MediaKind::Video
                            }
                        };
                        log::info!(
                            "[P2P] {:?} remote track arrived: id={}, kind={:?}",
                            cb_handle,
                            track_id,
                            kind
                        );
                        if let Ok(mut evts) = events.lock() {
                            evts.push(EngineEvent::P2pRemoteTrack {
                                handle: cb_handle,
                                track_id,
                                kind,
                            });
                        }
                    }) as libwebrtc::peer_connection::OnTrack));

                    session.pc = Some(pc);
                }
                Err(e) => {
                    log::error!(
                        "[P2P] {:?} failed to create peer connection: {}",
                        handle,
                        e
                    );
                    session.transition(P2pState::Failed, handle);
                }
            }

            // suppress unused-import warnings for these types (used above in Box::new)
            let _ = (
                AnswerOptions::default(),
                OfferOptions::default(),
            );
            let _: libwebrtc::peer_connection::OnTrack = Box::new(|_| {});
        }

        self.sessions.insert(handle, session);
        log::info!("[P2P] {:?} created", handle);

        handle
    }

    /// 关闭 PeerConnection
    pub fn close(&mut self, handle: PeerHandle) {
        if let Some(session) = self.sessions.get_mut(&handle) {
            session.transition(P2pState::Closed, handle);
            if let Some(pc) = &session.pc {
                pc.close();
            }
        }
        self.sessions.remove(&handle);
        log::info!("[P2P] {:?} closed", handle);
    }

    /// 关闭所有连接
    pub fn close_all(&mut self) {
        let handles: Vec<PeerHandle> = self.sessions.keys().copied().collect();
        for h in handles {
            self.close(h);
        }
    }

    // ---- SDP 协商 ----

    /// 创建 Offer SDP
    ///
    /// 上层拿到返回的 SDP 后，通过 MQTT 发送给目标设备。
    /// 随后本端进入 Connecting 状态，等待远端 Answer。
    pub fn create_offer(&mut self, handle: PeerHandle) -> Result<SessionDescription, RtcError> {
        if !self.sessions.contains_key(&handle) {
            return Err(RtcError::InvalidHandle);
        }

        match self.do_create_offer(handle) {
            Ok(sdp) => {
                if let Some(session) = self.sessions.get_mut(&handle) {
                    session.transition(P2pState::Connecting, handle);
                }
                Ok(sdp)
            }
            Err(e) => {
                if let Some(session) = self.sessions.get_mut(&handle) {
                    session.transition(P2pState::Failed, handle);
                }
                Err(e)
            }
        }
    }

    fn do_create_offer(&mut self, handle: PeerHandle) -> Result<SessionDescription, RtcError> {
        use conv::*;
        use libwebrtc::peer_connection::OfferOptions;
        use libwebrtc::rtp_transceiver::{RtpTransceiverDirection, RtpTransceiverInit};
        use libwebrtc::MediaType;

        let session = self
            .sessions
            .get(&handle)
            .ok_or(RtcError::InvalidHandle)?;
        let pc = session
            .pc
            .as_ref()
            .ok_or_else(|| RtcError::Internal("no peer connection".into()))?;

        let existing = pc.transceivers();
        log::info!(
            "[P2P] {:?} do_create_offer: existing transceivers={}",
            handle,
            existing.len()
        );

        // Only add media transceivers if none exist yet.
        // If attachP2PAudio was already called (which creates a transceiver
        // via add_transceiver_from_track in the OHOS driver), skip adding
        // empty transceivers here to avoid duplicate m= lines in the Offer
        // that would cause ErrRTPReceiverForSSRCTrackStreamNotFound later.
        if existing.is_empty() {
            pc.add_transceiver_for_media(
                MediaType::Audio,
                RtpTransceiverInit {
                    direction: RtpTransceiverDirection::SendRecv,
                    stream_ids: vec![],
                    send_encodings: vec![],
                },
            )
            .map_err(map_err)?;

            pc.add_transceiver_for_media(
                MediaType::Video,
                RtpTransceiverInit {
                    direction: RtpTransceiverDirection::SendRecv,
                    stream_ids: vec![],
                    send_encodings: vec![],
                },
            )
            .map_err(map_err)?;

            log::info!(
                "[P2P] {:?} do_create_offer: created audio+video transceivers",
                handle
            );
        } else {
            log::info!(
                "[P2P] {:?} do_create_offer: skipping transceiver creation (already have {})",
                handle,
                existing.len()
            );
        }

        let offer = block_on(pc.create_offer(OfferOptions::default())).map_err(map_err)?;
        log::info!(
            "[P2P] {:?} do_create_offer: Offer SDP ({} bytes) m= lines: {}",
            handle,
            offer.to_string().len(),
            offer.to_string().lines().filter(|l| l.starts_with("m=")).collect::<Vec<_>>().join(" | ")
        );
        block_on(pc.set_local_description(offer.clone())).map_err(map_err)?;

        Ok(from_lw_sdp(&offer))
    }

    /// 创建 Answer SDP
    ///
    /// 上层在收到对端 Offer 后调用此方法生成 Answer，
    /// 再通过 MQTT 将 Answer SDP 发回对端。
    pub fn create_answer(
        &mut self,
        handle: PeerHandle,
        offer: &SessionDescription,
    ) -> Result<SessionDescription, RtcError> {
        if !self.sessions.contains_key(&handle) {
            return Err(RtcError::InvalidHandle);
        }

        match self.do_create_answer(handle, offer) {
            Ok(answer) => {
                if let Some(session) = self.sessions.get_mut(&handle) {
                    session.transition(P2pState::Connecting, handle);
                }
                Ok(answer)
            }
            Err(e) => {
                if let Some(session) = self.sessions.get_mut(&handle) {
                    session.transition(P2pState::Failed, handle);
                }
                Err(e)
            }
        }
    }

    fn do_create_answer(
        &mut self,
        handle: PeerHandle,
        offer: &SessionDescription,
    ) -> Result<SessionDescription, RtcError> {
        use conv::*;
        use libwebrtc::peer_connection::AnswerOptions;

        let session = self
            .sessions
            .get(&handle)
            .ok_or(RtcError::InvalidHandle)?;
        let pc = session
            .pc
            .as_ref()
            .ok_or_else(|| RtcError::Internal("no peer connection".into()))?;

        // 1. 设置远端 Offer（如果尚未设置，允许 ArkTS 层提前调用 setRemoteSdp）
        let sig_state = pc.signaling_state();
        if !matches!(sig_state, libwebrtc::peer_connection::SignalingState::HaveRemoteOffer) {
            let lw_offer = to_lw_sdp(offer)?;
            block_on(pc.set_remote_description(lw_offer)).map_err(map_err)?;
        }

        // 2. 创建 Answer
        let answer = block_on(pc.create_answer(AnswerOptions::default())).map_err(map_err)?;

        // 3. 设置本地 Answer
        block_on(pc.set_local_description(answer.clone())).map_err(map_err)?;

        Ok(from_lw_sdp(&answer))
    }

    /// 设置远端 SDP
    ///
    /// 收到对端的 Offer 或 Answer 后调用。
    /// 连接状态由 PeerConnection 回调自动更新。
    pub fn set_remote_sdp(
        &mut self,
        handle: PeerHandle,
        sdp: &SessionDescription,
    ) -> Result<(), RtcError> {
        if !self.sessions.contains_key(&handle) {
            return Err(RtcError::InvalidHandle);
        }

        match self.do_set_remote_sdp(handle, sdp) {
            Ok(()) => Ok(()),
            Err(e) => {
                if let Some(session) = self.sessions.get_mut(&handle) {
                    session.transition(P2pState::Failed, handle);
                }
                Err(e)
            }
        }
    }

    fn do_set_remote_sdp(
        &mut self,
        handle: PeerHandle,
        sdp: &SessionDescription,
    ) -> Result<(), RtcError> {
        use conv::*;

        let session = self
            .sessions
            .get(&handle)
            .ok_or(RtcError::InvalidHandle)?;
        let pc = session
            .pc
            .as_ref()
            .ok_or_else(|| RtcError::Internal("no peer connection".into()))?;

        let lw_sdp = to_lw_sdp(sdp)?;
        let sig_before = pc.signaling_state();
        let transceiver_count_before = pc.transceivers().len();

        // Log the incoming SDP for diagnostics — extract m= lines and directions
        {
            let sdp_str = &lw_sdp.to_string();
            let m_lines: Vec<String> = sdp_str
                .lines()
                .filter(|l| l.starts_with("m=") || l.starts_with("a=send") || l.starts_with("a=recv") || l.starts_with("a=inactive") || l.starts_with("a=mid:"))
                .map(|l| l.to_string())
                .collect();
            log::info!(
                "[P2P] {:?} do_set_remote_sdp: type={:?}, sig_state_before={:?}, existing_transceivers={}, SDP m/dir lines: [{}]",
                handle,
                sdp.sdp_type,
                sig_before,
                transceiver_count_before,
                m_lines.join(", ")
            );
        }

        // NOTE: We no longer pre-create an audio transceiver here.
        // The ArkTS layer now calls attachP2PAudio BEFORE setRemoteSdp
        // (on the callee side), so the track-bearing transceiver already
        // exists and will be matched to the Offer's m= line by
        // set_remote_description. This avoids duplicate m= lines
        // that caused ErrRTPReceiverForSSRCTrackStreamNotFound.

        let result = block_on(pc.set_remote_description(lw_sdp)).map_err(map_err);

        let sig_after = pc.signaling_state();
        let transceiver_count_after = pc.transceivers().len();
        log::info!(
            "[P2P] {:?} do_set_remote_sdp: result={:?}, sig_state={:?}→{:?}, transceivers={}→{}",
            handle,
            result.is_ok(),
            sig_before,
            sig_after,
            transceiver_count_before,
            transceiver_count_after
        );

        result
    }

    // ---- ICE 候选 ----

    /// 添加远端 ICE 候选
    ///
    /// 上层从 MQTT 收到对端的 ICE 候选后调用。
    pub fn add_ice_candidate(
        &mut self,
        handle: PeerHandle,
        candidate: &IceCandidate,
    ) -> Result<(), RtcError> {
        let _session = self.sessions.get(&handle).ok_or(RtcError::InvalidHandle)?;
        self.do_add_ice_candidate(handle, candidate)
    }

    fn do_add_ice_candidate(
        &mut self,
        handle: PeerHandle,
        candidate: &IceCandidate,
    ) -> Result<(), RtcError> {
        use conv::*;

        let session = self
            .sessions
            .get(&handle)
            .ok_or(RtcError::InvalidHandle)?;
        let pc = session
            .pc
            .as_ref()
            .ok_or_else(|| RtcError::Internal("no peer connection".into()))?;

        let lw_candidate = to_lw_ice(candidate)?;
        block_on(pc.add_ice_candidate(lw_candidate)).map_err(map_err)
    }

    // ---- 音频 ---- 

    /// 为 P2P 连接绑定本地音频 track
    ///
    /// 创建 NativeAudioSource → 创建 RtcAudioTrack → add_track 到 PeerConnection。
    /// add_track 会自动创建 RtpSendPipeline 并绑定到 audio source，
    /// 之后通过 [`push_audio_frame`] 推送的 PCM 数据会被 Opus 编码后自动发送。
    pub fn attach_audio(&mut self, handle: PeerHandle) -> Result<(), RtcError> {
        let session = self
            .sessions
            .get_mut(&handle)
            .ok_or(RtcError::InvalidHandle)?;
        let pc = session
            .pc
            .as_ref()
            .ok_or_else(|| RtcError::Internal("no peer connection".into()))?;

        use libwebrtc::audio_source::AudioSourceOptions;
        use libwebrtc::audio_source::native::NativeAudioSource;
        use libwebrtc::media_stream_track::MediaStreamTrack;
        use libwebrtc::peer_connection_factory::native::PeerConnectionFactoryExt;

        // 创建音频源 (48kHz, mono, 20ms frames)
        let source = NativeAudioSource::new(AudioSourceOptions::default(), 48000, 1, 20);
        // 启用软件 AEC（sonora WebRTC AEC3）
        source.init_aec();

        // 创建 audio track
        let track = self.factory.create_audio_track("p2p_audio", source.clone());

        // add_track 到 PeerConnection — 这会自动创建 RtpSendPipeline
        pc.add_track(
            MediaStreamTrack::Audio(track.clone()),
            &["p2p_stream"],
        )
        .map_err(conv::map_err)?;

        log::info!(
            "[P2P] {:?} audio source attached: track_id={}",
            handle,
            track.id()
        );

        session.audio_source = Some(source);
        session.audio_track = Some(track);
        Ok(())
    }

    /// 推送 PCM 音频帧到 P2P 音频源
    ///
    /// PCM 数据会被内部 Opus 编码器处理，然后通过 RTP 发送到对端。
    /// 调用前必须先调用 [`attach_audio`] 绑定音频源。
    ///
    /// `data` - 交错的 PCM 采样数据 (i16)
    /// `sample_rate` - 采样率，必须与 attach_audio 时的参数匹配
    /// `channels` - 声道数，必须与 attach_audio 时的参数匹配
    /// `samples_per_channel` - 每声道采样数
    pub fn push_audio_frame(
        &self,
        handle: PeerHandle,
        data: &[i16],
        sample_rate: u32,
        channels: u32,
        samples_per_channel: u32,
    ) -> Result<(), RtcError> {
        use libwebrtc::audio_frame::AudioFrame;

        let session = self
            .sessions
            .get(&handle)
            .ok_or(RtcError::InvalidHandle)?;
        let source = session
            .audio_source
            .as_ref()
            .ok_or_else(|| RtcError::Internal("no audio source attached".into()))?;

        let frame = AudioFrame {
            data: std::borrow::Cow::Borrowed(data),
            sample_rate,
            num_channels: channels,
            samples_per_channel,
        };

        crate::block_on(source.capture_frame(&frame)).map_err(conv::map_err)
    }

    /// 推送远端参考帧用于 AEC（回声消除）
    pub fn push_reference_frame(&self, handle: PeerHandle, data: &[i16]) {
        if let Some(session) = self.sessions.get(&handle) {
            if let Some(source) = &session.audio_source {
                source.push_reference_frame(data);
            }
        }
    }

    /// 取出远端音频 track（内部从 on_track 回调中存储的）
    ///
    /// 取出后该 track 从 session 中移除，调用方负责创建 NativeAudioStream。
    /// 返回 `None` 表示 track 不存在或已被取出。
    pub fn take_remote_audio_track(
        &self,
        handle: PeerHandle,
        track_id: &str,
    ) -> Option<libwebrtc::audio_track::RtcAudioTrack> {
        let session = self.sessions.get(&handle)?;
        let mut tracks = session.remote_audio_tracks.lock().ok()?;
        tracks.remove(track_id)
    }

    /// 远端音频 track 是否已到达
    pub fn has_remote_audio_track(&self, handle: PeerHandle, track_id: &str) -> bool {
        self.sessions
            .get(&handle)
            .and_then(|s| s.remote_audio_tracks.lock().ok())
            .map(|tracks| tracks.contains_key(track_id))
            .unwrap_or(false)
    }

    // ---- 事件 ----

    /// 获取 P2P 管理器产生的待处理事件并清空队列
    pub fn drain_events(&mut self) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        for session in self.sessions.values() {
            if let Ok(mut session_events) = session.events.lock() {
                events.append(&mut session_events);
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_close() {
        let mut mgr = P2pManager::new();
        let config = P2pConfig::default();
        let h = mgr.create(&config);
        assert!(mgr.sessions.contains_key(&h));
        mgr.close(h);
        assert!(!mgr.sessions.contains_key(&h));
    }

    #[test]
    fn test_create_offer() {
        let mut mgr = P2pManager::new();
        let h = mgr.create(&P2pConfig::default());
        let offer = mgr.create_offer(h).unwrap();
        assert_eq!(offer.sdp_type, SdpType::Offer);
        assert!(!offer.sdp.is_empty());
    }

    #[test]
    fn test_invalid_handle() {
        let mut mgr = P2pManager::new();
        assert!(mgr.create_offer(PeerHandle(999)).is_err());
    }

    #[test]
    fn test_empty_sdp_rejected() {
        let mut mgr = P2pManager::new();
        let h = mgr.create(&P2pConfig::default());
        let _offer = mgr.create_offer(h).unwrap();
        // 空 SDP 应返回错误，而不是 panic
        let result = mgr.set_remote_sdp(
            h,
            &SessionDescription {
                sdp_type: SdpType::Answer,
                sdp: String::new(),
            },
        );
        assert!(result.is_err());
        // 失败后状态应为 Failed
        let events = mgr.drain_events();
        assert!(events.iter().any(|e| matches!(
            e,
            EngineEvent::P2pStateChange {
                state: P2pState::Failed,
                ..
            }
        )));
    }
}
