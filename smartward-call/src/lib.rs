//! SmartWard Call — 纯 WebRTC 客户端引擎
//!
//! 为 SmartWard 客户端设备 (Android Pad / OHOS 床头屏等) 提供
//! P2P WebRTC 和 SFU LiveKit 双模音视频通信能力。
//!
//! # 架构
//!
//! ```text
//! 上层 (Android / OHOS)
//!   ├── MQTT 信令 (连接科内 rumqttd)
//!   ├── Topic 路由 (local/ / cascade/)
//!   └── 会话编排 (CallSessionManager)
//!        │
//!        │ FFI: push SDP/ICE in, get events out
//!        ▼
//! WebRtcEngine (本 crate)
//!   ├── P2pManager  → PeerConnection 直连
//!   ├── SfuManager  → LiveKit SFU
//!   └── SessionRouter → 纯决策函数
//! ```
//!
//! # 用法
//!
//! ```ignore
//! use smartward_call::*;
//!
//! let mut engine = WebRtcEngine::new();
//!
//! // ── 科内 P2P 呼叫 ──
//! let mode = SessionRouter::resolve("wardA", "wardA", false);
//! // mode == TransportMode::P2P
//!
//! let handle = engine.create_p2p_connection(&P2pConfig::default());
//! let offer = engine.create_offer(handle).unwrap();
//! // → 上层通过 MQTT 发送 offer SDP 给目标设备
//!
//! // 收到远端 Answer 后：
//! engine.set_remote_sdp(handle, &answer_sdp).unwrap();
//!
//! // 轮询事件：
//! for event in engine.poll_events() {
//!     match event {
//!         EngineEvent::P2pIceCandidate { handle, candidate } => {
//!             // → 上层通过 MQTT 发送 ICE 候选
//!         }
//!         EngineEvent::P2pStateChange { handle, state } => {
//!             // → 通知 UI 连接状态
//!         }
//!         _ => {}
//!     }
//! }
//!
//! // ── 跨科室 SFU 呼叫 ──
//! let mode = SessionRouter::resolve("wardA", "wardB", false);
//! // mode == TransportMode::SFU
//!
//! let sfu_handle = engine.create_sfu_session(SfuConfig {
//!     room_name: "consult-wardA-wardB".into(),
//!     participant_identity: "nurse-ns001".into(),
//!     token: "livekit-jwt-token".into(),
//! });
//! engine.connect_sfu(sfu_handle).unwrap();
//! ```

mod types;
mod p2p;
mod sfu;
mod router;

pub use types::*;
pub use router::SessionRouter;

use p2p::P2pManager;
use sfu::SfuManager;

// ============================================================
// WebRTC 引擎
// ============================================================

/// SmartWard WebRTC 客户端引擎
///
/// 统一管理 P2P PeerConnection 和 SFU LiveKit 会话。
/// 所有方法均为同步、FFI 友好。
pub struct WebRtcEngine {
    p2p: P2pManager,
    sfu: SfuManager,
}

impl WebRtcEngine {
    /// 创建新引擎实例
    pub fn new() -> Self {
        Self {
            p2p: P2pManager::new(),
            sfu: SfuManager::new(),
        }
    }

    // ============================================================
    // P2P API
    // ============================================================

    /// 创建 P2P PeerConnection，返回句柄
    pub fn create_p2p_connection(&mut self, config: &P2pConfig) -> PeerHandle {
        self.p2p.create(config)
    }

    /// 创建 Offer SDP，上层通过 MQTT 发送给对端
    pub fn create_offer(&mut self, handle: PeerHandle) -> Result<SessionDescription, RtcError> {
        self.p2p.create_offer(handle)
    }

    /// 创建 Answer SDP（收到对端 Offer 后）
    pub fn create_answer(
        &mut self,
        handle: PeerHandle,
        offer: &SessionDescription,
    ) -> Result<SessionDescription, RtcError> {
        self.p2p.create_answer(handle, offer)
    }

    /// 设置远端 SDP（收到对端 Offer/Answer 后）
    pub fn set_remote_sdp(
        &mut self,
        handle: PeerHandle,
        sdp: &SessionDescription,
    ) -> Result<(), RtcError> {
        self.p2p.set_remote_sdp(handle, sdp)
    }

    /// 添加远端 ICE 候选（从 MQTT 收到后）
    pub fn add_ice_candidate(
        &mut self,
        handle: PeerHandle,
        candidate: &IceCandidate,
    ) -> Result<(), RtcError> {
        self.p2p.add_ice_candidate(handle, candidate)
    }

    /// 关闭 P2P 连接
    pub fn close_p2p(&mut self, handle: PeerHandle) {
        self.p2p.close(handle);
    }

    // ============================================================
    // SFU API
    // ============================================================

    /// 创建 SFU 会话，返回句柄
    pub fn create_sfu_session(&mut self, config: SfuConfig) -> SfuHandle {
        self.sfu.create_session(config)
    }

    /// 连接到 LiveKit 房间
    pub fn connect_sfu(&mut self, handle: SfuHandle) -> Result<(), RtcError> {
        self.sfu.connect(handle)
    }

    /// 断开 LiveKit 连接
    pub fn disconnect_sfu(&mut self, handle: SfuHandle) {
        self.sfu.disconnect(handle);
    }

    /// 关闭 SFU 会话
    pub fn close_sfu(&mut self, handle: SfuHandle) {
        self.sfu.close(handle);
    }

    /// SFU 是否已连接
    pub fn is_sfu_available(&self) -> bool {
        self.sfu.is_available()
    }

    // ============================================================
    // 事件
    // ============================================================

    /// 轮询所有待处理事件（P2P + SFU），清空内部队列
    pub fn poll_events(&mut self) -> Vec<EngineEvent> {
        let mut events = self.p2p.drain_events();
        events.append(&mut self.sfu.drain_events());
        events
    }

    // ============================================================
    // 生命周期
    // ============================================================

    /// 关闭所有连接和会话
    pub fn shutdown(&mut self) {
        self.p2p.close_all();
        self.sfu.close_all();
    }
}

impl Default for WebRtcEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for WebRtcEngine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_p2p_stub_flow() {
        let mut engine = WebRtcEngine::new();
        let h = engine.create_p2p_connection(&P2pConfig::default());
        let offer = engine.create_offer(h).unwrap();
        assert_eq!(offer.sdp_type, SdpType::Offer);

        let answer = SessionDescription { sdp_type: SdpType::Answer, sdp: String::new() };
        engine.set_remote_sdp(h, &answer).unwrap();

        let events = engine.poll_events();
        assert!(events.iter().any(|e| matches!(e, EngineEvent::P2pStateChange { state: P2pState::Connected, .. })));
    }

    #[test]
    fn test_sfu_stub_flow() {
        let mut engine = WebRtcEngine::new();
        let h = engine.create_sfu_session(SfuConfig {
            room_name: "test-room".into(),
            participant_identity: "test-user".into(),
            token: "test-token".into(),
        });
        engine.connect_sfu(h).unwrap();
        let events = engine.poll_events();
        assert!(events.iter().any(|e| matches!(e, EngineEvent::SfuConnected { .. })));
    }

    #[test]
    fn test_shutdown() {
        let mut engine = WebRtcEngine::new();
        let h = engine.create_p2p_connection(&P2pConfig::default());
        engine.shutdown();
        assert!(engine.create_offer(h).is_err());
    }

    #[test]
    fn test_poll_events_clears_queue() {
        let mut engine = WebRtcEngine::new();
        let h = engine.create_p2p_connection(&P2pConfig::default());
        engine.create_offer(h).unwrap();
        // First poll gets events
        let events1 = engine.poll_events();
        assert!(!events1.is_empty());
        // Second poll is empty
        let events2 = engine.poll_events();
        assert!(events2.is_empty());
    }
}

