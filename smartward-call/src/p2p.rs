//! P2P WebRTC 引擎 — 管理 PeerConnection 直连
//!
//! 不包含任何 MQTT/信令逻辑。上层负责：
//! 1. 通过 `create_offer()` / `create_answer()` 获取 SDP，用 MQTT 发送
//! 2. 收到远端 SDP 后调用 `set_remote_sdp()`
//! 3. 从 `poll_events()` 拿到 ICE 候选后，用 MQTT 发送
//! 4. 收到远端 ICE 后调用 `add_ice_candidate()`

use std::collections::HashMap;
use crate::types::*;

// ============================================================
// 内部会话
// ============================================================

struct P2pSession {
    state: P2pState,
    /// 本地生成的待发送事件 (ICE 候选、状态变更等)
    events: Vec<EngineEvent>,
}

impl P2pSession {
    fn new() -> Self {
        Self { state: P2pState::New, events: Vec::new() }
    }

    fn transition(&mut self, new_state: P2pState, handle: PeerHandle) {
        if self.state != new_state {
            log::info!("[P2P] {:?} {:?} → {:?}", handle, self.state, new_state);
            self.state = new_state;
            self.events.push(EngineEvent::P2pStateChange { handle, state: new_state });
        }
    }
}

// ============================================================
// P2P 管理器
// ============================================================

pub(crate) struct P2pManager {
    sessions: HashMap<PeerHandle, P2pSession>,
    next_handle: u64,
}

impl P2pManager {
    pub fn new() -> Self {
        Self { sessions: HashMap::new(), next_handle: 1 }
    }

    // ---- 创建/销毁 ----

    /// 创建一个 P2P PeerConnection，返回句柄
    pub fn create(&mut self, _config: &P2pConfig) -> PeerHandle {
        let handle = PeerHandle(self.next_handle);
        self.next_handle += 1;
        self.sessions.insert(handle, P2pSession::new());
        log::info!("[P2P] {:?} created", handle);

        #[cfg(feature = "p2p-native")]
        {
            // TODO: 创建 libwebrtc::PeerConnection
            let _ = _config;
        }

        handle
    }

    /// 关闭 PeerConnection
    pub fn close(&mut self, handle: PeerHandle) {
        if let Some(session) = self.sessions.get_mut(&handle) {
            session.transition(P2pState::Closed, handle);
        }
        self.sessions.remove(&handle);
        log::info!("[P2P] {:?} closed", handle);

        #[cfg(feature = "p2p-native")]
        {
            // TODO: pc.close()
        }
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
        let session = self.sessions.get_mut(&handle).ok_or(RtcError::InvalidHandle)?;
        session.transition(P2pState::Connecting, handle);

        self.do_create_offer(handle)
    }

    #[cfg(feature = "p2p-native")]
    fn do_create_offer(&mut self, _handle: PeerHandle) -> Result<SessionDescription, RtcError> {
        // TODO: 调用 libwebrtc::PeerConnection::create_offer()
        // let offer = pc.create_offer()?;
        // pc.set_local_description(&offer)?;
        // Ok(offer.into())
        todo!("libwebrtc create_offer not yet integrated")
    }

    #[cfg(not(feature = "p2p-native"))]
    fn do_create_offer(&mut self, handle: PeerHandle) -> Result<SessionDescription, RtcError> {
        log::warn!("[P2P] {:?} create_offer: using stub SDP (enable p2p-native feature)", handle);
        Ok(SessionDescription {
            sdp_type: SdpType::Offer,
            sdp: format!("v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\ns=smartward-p2p-{}\r\nt=0 0\r\n", handle.0),
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
        let session = self.sessions.get_mut(&handle).ok_or(RtcError::InvalidHandle)?;
        session.transition(P2pState::Connecting, handle);
        self.do_create_answer(handle, offer)
    }

    #[cfg(feature = "p2p-native")]
    fn do_create_answer(&mut self, _handle: PeerHandle, _offer: &SessionDescription) -> Result<SessionDescription, RtcError> {
        // TODO: pc.set_remote_description(offer)?;
        // let answer = pc.create_answer()?;
        // pc.set_local_description(&answer)?;
        // Ok(answer.into())
        todo!("libwebrtc create_answer not yet integrated")
    }

    #[cfg(not(feature = "p2p-native"))]
    fn do_create_answer(&mut self, handle: PeerHandle, _offer: &SessionDescription) -> Result<SessionDescription, RtcError> {
        log::warn!("[P2P] {:?} create_answer: using stub SDP (enable p2p-native feature)", handle);
        Ok(SessionDescription {
            sdp_type: SdpType::Answer,
            sdp: format!("v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\ns=smartward-p2p-{}\r\nt=0 0\r\n", handle.0),
        })
    }

    /// 设置远端 SDP
    ///
    /// 收到对端的 Offer 或 Answer 后调用。
    /// - 收到 Answer → 本端进入 Connected 状态
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

        let session = self.sessions.get_mut(&handle).unwrap();
        match sdp.sdp_type {
            SdpType::Answer => {
                session.transition(P2pState::Connected, handle);
            }
            SdpType::Offer => {
                log::warn!("[P2P] {:?} set_remote_sdp with Offer: expected Answer after create_answer", handle);
            }
        }
        Ok(())
    }

    #[cfg(feature = "p2p-native")]
    fn do_set_remote_sdp(&mut self, _handle: PeerHandle, _sdp: &SessionDescription) -> Result<(), RtcError> {
        // TODO: pc.set_remote_description(sdp)?;
        Ok(())
    }

    #[cfg(not(feature = "p2p-native"))]
    fn do_set_remote_sdp(&mut self, _handle: PeerHandle, _sdp: &SessionDescription) -> Result<(), RtcError> {
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

    #[cfg(feature = "p2p-native")]
    fn do_add_ice_candidate(&mut self, _handle: PeerHandle, _candidate: &IceCandidate) -> Result<(), RtcError> {
        // TODO: pc.add_ice_candidate(candidate)?;
        Ok(())
    }

    #[cfg(not(feature = "p2p-native"))]
    fn do_add_ice_candidate(&mut self, _handle: PeerHandle, _candidate: &IceCandidate) -> Result<(), RtcError> {
        Ok(())
    }

    // ---- 事件 ----

    /// 获取 P2P 管理器产生的待处理事件并清空队列
    pub fn drain_events(&mut self) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        for session in self.sessions.values_mut() {
            events.append(&mut session.events);
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
        mgr.set_remote_sdp(h, &SessionDescription { sdp_type: SdpType::Answer, sdp: String::new() }).unwrap();
        let events = mgr.drain_events();
        assert!(events.iter().any(|e| matches!(e, EngineEvent::P2pStateChange { state: P2pState::Connected, .. })));
    }
}
