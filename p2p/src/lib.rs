//! SmartWard Call — 纯 WebRTC P2P 客户端引擎
//!
//! 为 SmartWard 客户端设备 (Android Pad / OHOS 床头屏等) 提供
//! P2P WebRTC 音视频通信能力。
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
//!   └── P2pManager  → PeerConnection 直连
//! ```
//!
//! # 用法
//!
//! ```ignore
//! use p2p::*;
//!
//! let mut engine = WebRtcEngine::new();
//!
//! // ── 科内 P2P 呼叫 ──
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
//! ```

mod types;
mod p2p;
mod ffi;

pub use types::*;
pub use libwebrtc::audio_track::RtcAudioTrack;

use p2p::P2pManager;

// ============================================================
// 公共辅助
// ============================================================

/// 在当前 tokio runtime 上阻塞执行 future；若未进入 runtime，则创建临时 runtime
pub(crate) fn block_on<F: std::future::Future>(f: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle.block_on(f),
        Err(_) => {
            let rt = tokio::runtime::Runtime::new()
                .expect("failed to create tokio runtime for p2p");
            rt.block_on(f)
        }
    }
}

// ============================================================
// WebRTC 引擎
// ============================================================

/// SmartWard WebRTC 客户端引擎
///
/// 管理 P2P PeerConnection 音视频通信。
/// 所有方法均为同步、FFI 友好。
pub struct WebRtcEngine {
    p2p: P2pManager,
}

impl WebRtcEngine {
    /// 创建新引擎实例
    pub fn new() -> Self {
        Self {
            p2p: P2pManager::new(),
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

    /// 为 P2P 连接绑定本地音频 track
    ///
    /// 创建音频源并添加到 PeerConnection，之后可通过 [`push_p2p_audio_frame`] 推送 PCM 数据。
    pub fn attach_p2p_audio(&mut self, handle: PeerHandle) -> Result<(), RtcError> {
        self.p2p.attach_audio(handle)
    }

    /// 推送 PCM 音频帧到 P2P 音频源
    ///
    /// 调用前必须先调用 [`attach_p2p_audio`]。
    pub fn push_p2p_audio_frame(
        &self,
        handle: PeerHandle,
        data: &[i16],
        sample_rate: u32,
        channels: u32,
        samples_per_channel: u32,
    ) -> Result<(), RtcError> {
        self.p2p.push_audio_frame(handle, data, sample_rate, channels, samples_per_channel)
    }

    /// 推送远端参考帧用于 AEC（回声消除）
    pub fn push_p2p_reference_frame(&self, handle: PeerHandle, data: &[i16]) {
        self.p2p.push_reference_frame(handle, data);
    }

    /// 取出远端 P2P 音频 track（用于创建 NativeAudioStream 播放）
    ///
    /// 远端 track 通过 `on_track` 回调到达后，内部存储；
    /// 上层在收到 [`EngineEvent::P2pRemoteTrack`] 后调用此方法取出。
    /// 返回 `None` 表示 track 不存在或已被取出。
    pub fn take_p2p_remote_audio_track(
        &self,
        handle: PeerHandle,
        track_id: &str,
    ) -> Option<RtcAudioTrack> {
        self.p2p.take_remote_audio_track(handle, track_id)
    }

    /// 检查远端 P2P 音频 track 是否已到达
    pub fn has_p2p_remote_audio_track(&self, handle: PeerHandle, track_id: &str) -> bool {
        self.p2p.has_remote_audio_track(handle, track_id)
    }

    // ============================================================
    // 事件
    // ============================================================

    /// 轮询所有待处理事件（P2P），清空内部队列
    pub fn poll_events(&mut self) -> Vec<EngineEvent> {
        self.p2p.drain_events()
    }

    // ============================================================
    // 生命周期
    // ============================================================

    /// 关闭所有连接
    pub fn shutdown(&mut self) {
        self.p2p.close_all();
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
    fn test_empty_sdp_rejected() {
        let mut engine = WebRtcEngine::new();
        let h = engine.create_p2p_connection(&P2pConfig::default());
        let _offer = engine.create_offer(h).unwrap();

        let answer = SessionDescription { sdp_type: SdpType::Answer, sdp: String::new() };
        let result = engine.set_remote_sdp(h, &answer);
        assert!(result.is_err());
        // 失败后状态应为 Failed
        let events = engine.poll_events();
        assert!(events.iter().any(|e| matches!(e, EngineEvent::P2pStateChange { state: P2pState::Failed, .. })));
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
