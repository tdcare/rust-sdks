//! ArkTS-facing wrapper around [`smartward_call::WebRtcEngine`].
//!
//! Exposes the SmartWard call engine to ArkTS via napi-ohos.
//! All complex types cross the boundary as JSON strings.

use napi_derive_ohos::napi;
use napi_ohos::bindgen_prelude::*;
use parking_lot::Mutex;
use smartward_call::{
    EngineEvent, IceCandidate, P2pConfig, PeerHandle, SessionDescription, SfuConfig,
    SfuHandle, WebRtcEngine,
};

// ============================================================
// Error mapping
// ============================================================

fn map_err(e: smartward_call::RtcError) -> Error {
    Error::from_reason(e.to_string())
}

fn json_err(e: impl std::fmt::Display) -> Error {
    Error::from_reason(format!("JSON error: {}", e))
}

// ============================================================
// LkSwcEngine – main entry point
// ============================================================

/// SmartWard WebRTC 呼叫引擎。
///
/// 统一管理 P2P PeerConnection 和 SFU LiveKit 会话。
/// ArkTS 用法:
/// ```typescript
/// import { LkSwcEngine } from 'liblivekit.so';
/// let engine = new LkSwcEngine();
/// let handle = engine.createP2p('{"ice_servers":[]}');
/// let offerJson = engine.createOffer(handle);
/// ```
#[napi]
pub struct LkSwcEngine {
    inner: Mutex<WebRtcEngine>,
}

#[napi]
impl LkSwcEngine {
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(WebRtcEngine::new()),
        }
    }

    // ============================================================
    // P2P
    // ============================================================

    /// 创建 P2P PeerConnection。
    ///
    /// `config_json` - JSON 格式的 P2pConfig，传 `"{}"` 使用默认值。
    /// 返回连接句柄 (i64)。
    #[napi]
    pub fn create_p2p(&self, config_json: String) -> Result<i64> {
        let config: P2pConfig = if config_json.is_empty() || config_json == "{}" {
            P2pConfig::default()
        } else {
            serde_json::from_str(&config_json).map_err(json_err)?
        };
        let mut engine = self.inner.lock();
        let handle = engine.create_p2p_connection(&config);
        Ok(handle.as_u64() as i64)
    }

    /// 创建 Offer SDP，上层通过 MQTT 发送给对端。
    ///
    /// 返回 JSON 格式的 SessionDescription 字符串。
    #[napi]
    pub fn create_offer(&self, handle: i64) -> Result<String> {
        let mut engine = self.inner.lock();
        let sdp = engine
            .create_offer(PeerHandle::from(handle as u64))
            .map_err(map_err)?;
        serde_json::to_string(&sdp).map_err(json_err)
    }

    /// 创建 Answer SDP（收到对端 Offer 后）。
    ///
    /// `offer_json` - 对端 Offer 的 JSON 字符串。
    /// 返回 Answer 的 JSON 字符串。
    #[napi]
    pub fn create_answer(&self, handle: i64, offer_json: String) -> Result<String> {
        let offer: SessionDescription =
            serde_json::from_str(&offer_json).map_err(json_err)?;
        let mut engine = self.inner.lock();
        let answer = engine
            .create_answer(PeerHandle::from(handle as u64), &offer)
            .map_err(map_err)?;
        serde_json::to_string(&answer).map_err(json_err)
    }

    /// 设置远端 SDP。
    ///
    /// `sdp_json` - 远端 SDP 的 JSON 字符串。
    #[napi]
    pub fn set_remote_sdp(&self, handle: i64, sdp_json: String) -> Result<()> {
        let sdp: SessionDescription =
            serde_json::from_str(&sdp_json).map_err(json_err)?;
        let mut engine = self.inner.lock();
        engine
            .set_remote_sdp(PeerHandle::from(handle as u64), &sdp)
            .map_err(map_err)
    }

    /// 添加远端 ICE 候选。
    ///
    /// `candidate_json` - ICE 候选的 JSON 字符串。
    #[napi]
    pub fn add_ice_candidate(&self, handle: i64, candidate_json: String) -> Result<()> {
        let candidate: IceCandidate =
            serde_json::from_str(&candidate_json).map_err(json_err)?;
        let mut engine = self.inner.lock();
        engine
            .add_ice_candidate(PeerHandle::from(handle as u64), &candidate)
            .map_err(map_err)
    }

    /// 关闭 P2P 连接。
    #[napi]
    pub fn close_p2p(&self, handle: i64) -> Result<()> {
        let mut engine = self.inner.lock();
        engine.close_p2p(PeerHandle::from(handle as u64));
        Ok(())
    }

    // ============================================================
    // SFU
    // ============================================================

    /// 创建 SFU 会话。
    ///
    /// `config_json` - JSON 格式的 SfuConfig。
    /// 返回会话句柄 (i64)。
    #[napi]
    pub fn create_sfu_session(&self, config_json: String) -> Result<i64> {
        let config: SfuConfig = serde_json::from_str(&config_json).map_err(json_err)?;
        let mut engine = self.inner.lock();
        let handle = engine.create_sfu_session(config);
        Ok(handle.as_u64() as i64)
    }

    /// 连接 SFU 会话到 LiveKit 服务器。
    #[napi]
    pub fn connect_sfu(&self, handle: i64) -> Result<()> {
        let mut engine = self.inner.lock();
        engine
            .connect_sfu(SfuHandle::from(handle as u64))
            .map_err(map_err)
    }

    /// 断开 SFU 连接。
    #[napi]
    pub fn disconnect_sfu(&self, handle: i64) -> Result<()> {
        let mut engine = self.inner.lock();
        engine.disconnect_sfu(SfuHandle::from(handle as u64));
        Ok(())
    }

    /// 关闭（销毁）SFU 会话。
    #[napi]
    pub fn close_sfu(&self, handle: i64) -> Result<()> {
        let mut engine = self.inner.lock();
        engine.close_sfu(SfuHandle::from(handle as u64));
        Ok(())
    }

    /// SFU 是否可用（至少有一个已连接的会话）。
    #[napi]
    pub fn is_sfu_available(&self) -> Result<bool> {
        let engine = self.inner.lock();
        Ok(engine.is_sfu_available())
    }

    // ============================================================
    // 事件
    // ============================================================

    /// 轮询所有待处理事件 (P2P + SFU)，返回 JSON 数组字符串。
    ///
    /// 空事件时返回 `"[]"`。
    #[napi]
    pub fn poll_events(&self) -> Result<String> {
        let mut engine = self.inner.lock();
        let events: Vec<EngineEvent> = engine.poll_events();
        serde_json::to_string(&events).map_err(json_err)
    }

    // ============================================================
    // 生命周期
    // ============================================================

    /// 关闭所有连接和会话。
    #[napi]
    pub fn shutdown(&self) -> Result<()> {
        let mut engine = self.inner.lock();
        engine.shutdown();
        Ok(())
    }
}

// ============================================================
// SessionRouter – 纯函数，无需实例
// ============================================================

/// 解析传输模式。
///
/// - `local_ward` / `target_ward` - 病区编号
/// - `is_broadcast` - 是否为一对多广播
///
/// 返回值:
/// - `0` → P2P（科内点对点）
/// - `1` → SFU（跨科室/广播）
#[napi]
pub fn resolve_transport_mode(
    local_ward: String,
    target_ward: String,
    is_broadcast: bool,
) -> i32 {
    let mode = smartward_call::SessionRouter::resolve(
        &local_ward,
        &target_ward,
        is_broadcast,
    );
    match mode {
        smartward_call::TransportMode::P2P => 0,
        smartward_call::TransportMode::SFU => 1,
    }
}
