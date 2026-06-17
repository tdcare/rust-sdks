//! P2P WebRTC 引擎 — 管理 PeerConnection 直连
//!
//! 不包含任何 MQTT/信令逻辑。上层负责：
//! 1. 通过 `create_offer()` / `create_answer()` 获取 SDP，用 MQTT 发送
//! 2. 收到远端 SDP 后调用 `set_remote_sdp()`
//! 3. 从 `poll_events()` 拿到 ICE 候选后，用 MQTT 发送
//! 4. 收到远端 ICE 后调用 `add_ice_candidate()`

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::types::*;

// ============================================================
// p2p-native 类型转换
// ============================================================

#[cfg(feature = "call-p2p")]
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

    pub fn to_lw_sdp(sdp: &SessionDescription) -> LwSdp {
        let sdp_type = match sdp.sdp_type {
            SdpType::Offer => LwSdpType::Offer,
            SdpType::Answer => LwSdpType::Answer,
        };
        LwSdp::parse(&sdp.sdp, sdp_type)
            .unwrap_or_else(|_| panic!("invalid SDP string from smartward-call"))
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

    pub fn to_lw_ice(c: &IceCandidate) -> LwIce {
        LwIce::parse(&c.sdp_mid, c.sdp_m_line_index, &c.candidate)
            .unwrap_or_else(|_| panic!("invalid ICE candidate string from smartward-call"))
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

    /// 在当前 tokio runtime 上阻塞执行 future；若未进入 runtime，则创建临时 runtime
    pub fn block_on<F: std::future::Future>(f: F) -> F::Output {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => handle.block_on(f),
            Err(_) => {
                let rt = tokio::runtime::Runtime::new()
                    .expect("failed to create tokio runtime for p2p-native");
                rt.block_on(f)
            }
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
    #[cfg(feature = "call-p2p")]
    pc: Option<libwebrtc::peer_connection::PeerConnection>,
}

impl P2pSession {
    fn new() -> Self {
        Self {
            state: P2pState::New,
            events: Arc::new(Mutex::new(Vec::new())),
            #[cfg(feature = "call-p2p")]
            pc: None,
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
    #[cfg(feature = "call-p2p")]
    factory: libwebrtc::peer_connection_factory::PeerConnectionFactory,
}

impl P2pManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            next_handle: 1,
            #[cfg(feature = "call-p2p")]
            factory: libwebrtc::peer_connection_factory::PeerConnectionFactory::default(),
        }
    }

    // ---- 创建/销毁 ----

    /// 创建一个 P2P PeerConnection，返回句柄
    pub fn create(&mut self, _config: &P2pConfig) -> PeerHandle {
        let handle = PeerHandle(self.next_handle);
        self.next_handle += 1;
        #[cfg_attr(not(feature = "call-p2p"), allow(unused_mut))]
        let mut session = P2pSession::new();

        #[cfg(feature = "call-p2p")]
        {
            use conv::*;
            use libwebrtc::peer_connection::{
                AnswerOptions, OfferOptions, OnConnectionChange, OnIceCandidate,
            };

            let rtc_config = to_rtc_config(_config);
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

                    session.pc = Some(pc);
                }
                Err(e) => {
                    log::error!(
                        "[P2P] {:?} failed to create peer connection: {}",
                        handle,
                        e
                    );
                    session.state = P2pState::Failed;
                }
            }

            // suppress unused-import warnings for these types (used above in Box::new)
            let _ = (AnswerOptions::default(), OfferOptions::default());
        }

        self.sessions.insert(handle, session);
        log::info!("[P2P] {:?} created", handle);

        handle
    }

    /// 关闭 PeerConnection
    pub fn close(&mut self, handle: PeerHandle) {
        if let Some(session) = self.sessions.get_mut(&handle) {
            session.transition(P2pState::Closed, handle);
            #[cfg(feature = "call-p2p")]
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
        let session = self
            .sessions
            .get_mut(&handle)
            .ok_or(RtcError::InvalidHandle)?;
        session.transition(P2pState::Connecting, handle);

        self.do_create_offer(handle)
    }

    #[cfg(feature = "call-p2p")]
    fn do_create_offer(&mut self, handle: PeerHandle) -> Result<SessionDescription, RtcError> {
        use conv::*;
        use libwebrtc::peer_connection::OfferOptions;

        let session = self
            .sessions
            .get(&handle)
            .ok_or(RtcError::InvalidHandle)?;
        let pc = session
            .pc
            .as_ref()
            .ok_or_else(|| RtcError::Internal("no peer connection".into()))?;

        let offer = block_on(pc.create_offer(OfferOptions::default())).map_err(map_err)?;
        block_on(pc.set_local_description(offer.clone())).map_err(map_err)?;

        Ok(from_lw_sdp(&offer))
    }

    #[cfg(not(feature = "call-p2p"))]
    fn do_create_offer(&mut self, handle: PeerHandle) -> Result<SessionDescription, RtcError> {
        log::warn!(
            "[P2P] {:?} create_offer: using stub SDP (enable call-p2p feature)",
            handle
        );
        Ok(SessionDescription {
            sdp_type: SdpType::Offer,
            sdp: format!(
                "v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\ns=smartward-p2p-{}\r\nt=0 0\r\n",
                handle.0
            ),
        })
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
        let session = self
            .sessions
            .get_mut(&handle)
            .ok_or(RtcError::InvalidHandle)?;
        session.transition(P2pState::Connecting, handle);
        self.do_create_answer(handle, offer)
    }

    #[cfg(feature = "call-p2p")]
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

        // 1. 设置远端 Offer
        let lw_offer = to_lw_sdp(offer);
        block_on(pc.set_remote_description(lw_offer)).map_err(map_err)?;

        // 2. 创建 Answer
        let answer = block_on(pc.create_answer(AnswerOptions::default())).map_err(map_err)?;

        // 3. 设置本地 Answer
        block_on(pc.set_local_description(answer.clone())).map_err(map_err)?;

        Ok(from_lw_sdp(&answer))
    }

    #[cfg(not(feature = "call-p2p"))]
    fn do_create_answer(
        &mut self,
        handle: PeerHandle,
        _offer: &SessionDescription,
    ) -> Result<SessionDescription, RtcError> {
        log::warn!(
            "[P2P] {:?} create_answer: using stub SDP (enable call-p2p feature)",
            handle
        );
        Ok(SessionDescription {
            sdp_type: SdpType::Answer,
            sdp: format!(
                "v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\ns=smartward-p2p-{}\r\nt=0 0\r\n",
                handle.0
            ),
        })
    }

    /// 设置远端 SDP
    ///
    /// 收到对端的 Offer 或 Answer 后调用。
    /// - 收到 Answer → 本端进入 Connected 状态（仅在 stub 模式下；native 模式由 PC 回调解状态）
    pub fn set_remote_sdp(
        &mut self,
        handle: PeerHandle,
        sdp: &SessionDescription,
    ) -> Result<(), RtcError> {
        // 先确保 handle 有效
        if !self.sessions.contains_key(&handle) {
            return Err(RtcError::InvalidHandle);
        }

        self.do_set_remote_sdp(handle, sdp)?;

        // stub 模式：手动推进状态；native 模式：PC 回调会自动更新状态
        #[cfg(not(feature = "call-p2p"))]
        {
            let session = self.sessions.get_mut(&handle).unwrap();
            match sdp.sdp_type {
                SdpType::Answer => {
                    session.transition(P2pState::Connected, handle);
                }
                SdpType::Offer => {
                    log::warn!(
                        "[P2P] {:?} set_remote_sdp with Offer: expected Answer after create_answer",
                        handle
                    );
                }
            }
        }
        Ok(())
    }

    #[cfg(feature = "call-p2p")]
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

        let lw_sdp = to_lw_sdp(sdp);
        block_on(pc.set_remote_description(lw_sdp)).map_err(map_err)
    }

    #[cfg(not(feature = "call-p2p"))]
    fn do_set_remote_sdp(
        &mut self,
        _handle: PeerHandle,
        _sdp: &SessionDescription,
    ) -> Result<(), RtcError> {
        Ok(())
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

    #[cfg(feature = "call-p2p")]
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

        let lw_candidate = to_lw_ice(candidate);
        block_on(pc.add_ice_candidate(lw_candidate)).map_err(map_err)
    }

    #[cfg(not(feature = "call-p2p"))]
    fn do_add_ice_candidate(
        &mut self,
        _handle: PeerHandle,
        _candidate: &IceCandidate,
    ) -> Result<(), RtcError> {
        Ok(())
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
    fn test_create_offer_stub() {
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
    fn test_full_handshake_stub() {
        let mut mgr = P2pManager::new();
        let h = mgr.create(&P2pConfig::default());
        let _offer = mgr.create_offer(h).unwrap();
        mgr.set_remote_sdp(
            h,
            &SessionDescription {
                sdp_type: SdpType::Answer,
                sdp: String::new(),
            },
        )
        .unwrap();
        let events = mgr.drain_events();
        assert!(events.iter().any(|e| matches!(
            e,
            EngineEvent::P2pStateChange {
                state: P2pState::Connected,
                ..
            }
        )));
    }
}
