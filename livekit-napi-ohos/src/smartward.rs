//! ArkTS-facing wrapper around [`smartward_call::WebRtcEngine`].
//!
//! Exposes the SmartWard call engine to ArkTS via napi-ohos.
//! All complex types cross the boundary as JSON strings.

use std::sync::Arc;

use napi_derive_ohos::napi;
use napi_ohos::bindgen_prelude::*;
use parking_lot::Mutex;
use smartward_call::{
    EngineEvent, IceCandidate, P2pConfig, PeerHandle, RtcAudioTrack, SessionDescription, SfuConfig,
    SfuHandle, WebRtcEngine,
};

use crate::audio_stream::LkAudioStream;
use libwebrtc::audio_stream::native::NativeAudioStream;

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
    runtime: Arc<tokio::runtime::Runtime>,
    inner: Mutex<WebRtcEngine>,
}

#[napi]
impl LkSwcEngine {
    #[napi(constructor)]
    pub fn new() -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| {
                Error::from_reason(format!("Failed to create Tokio runtime: {}", e))
            })?;
        // Initialize Rust→hilog bridge so native logs are visible
        crate::init_logger();
        Ok(Self {
            runtime: Arc::new(runtime),
            inner: Mutex::new(WebRtcEngine::new()),
        })
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
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = self.runtime.enter();
            let config: P2pConfig = if config_json.is_empty() || config_json == "{}" {
                P2pConfig::default()
            } else {
                serde_json::from_str(&config_json).map_err(json_err)?
            };
            let mut engine = self.inner.lock();
            let handle = engine.create_p2p_connection(&config);
            Ok(handle.as_u64() as i64)
        }))
        .map_err(|e| {
            let msg = if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else {
                "Unknown panic".to_string()
            };
            log::error!("create_p2p panicked: {}", msg);
            Error::from_reason(format!("create_p2p panicked: {}", msg))
        })?
    }

    /// 创建 Offer SDP，上层通过 MQTT 发送给对端。
    ///
    /// 返回 JSON 格式的 SessionDescription 字符串。
    #[napi]
    pub fn create_offer(&self, handle: i64) -> Result<String> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = self.runtime.enter();
            let mut engine = self.inner.lock();
            let sdp = engine
                .create_offer(PeerHandle::from(handle as u64))
                .map_err(map_err)?;
            serde_json::to_string(&sdp).map_err(json_err)
        }))
        .map_err(|e| {
            let msg = if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else {
                "Unknown panic".to_string()
            };
            log::error!("create_offer panicked: {}", msg);
            Error::from_reason(format!("create_offer panicked: {}", msg))
        })?
    }

    /// 创建 Answer SDP（收到对端 Offer 后）。
    ///
    /// `offer_json` - 对端 Offer 的 JSON 字符串。
    /// 返回 Answer 的 JSON 字符串。
    #[napi]
    pub fn create_answer(&self, handle: i64, offer_json: String) -> Result<String> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = self.runtime.enter();
            let offer: SessionDescription =
                serde_json::from_str(&offer_json).map_err(json_err)?;
            let mut engine = self.inner.lock();
            let answer = engine
                .create_answer(PeerHandle::from(handle as u64), &offer)
                .map_err(map_err)?;
            serde_json::to_string(&answer).map_err(json_err)
        }))
        .map_err(|e| {
            let msg = if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else {
                "Unknown panic".to_string()
            };
            log::error!("create_answer panicked: {}", msg);
            Error::from_reason(format!("create_answer panicked: {}", msg))
        })?
    }

    /// 设置远端 SDP。
    ///
    /// `sdp_json` - 远端 SDP 的 JSON 字符串。
    #[napi]
    pub fn set_remote_sdp(&self, handle: i64, sdp_json: String) -> Result<()> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = self.runtime.enter();
            let sdp: SessionDescription =
                serde_json::from_str(&sdp_json).map_err(json_err)?;
            let mut engine = self.inner.lock();
            engine
                .set_remote_sdp(PeerHandle::from(handle as u64), &sdp)
                .map_err(map_err)
        }))
        .map_err(|e| {
            let msg = if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else {
                "Unknown panic".to_string()
            };
            log::error!("set_remote_sdp panicked: {}", msg);
            Error::from_reason(format!("set_remote_sdp panicked: {}", msg))
        })?
    }

    /// 添加远端 ICE 候选。
    ///
    /// `candidate_json` - ICE 候选的 JSON 字符串。
    #[napi]
    pub fn add_ice_candidate(&self, handle: i64, candidate_json: String) -> Result<()> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = self.runtime.enter();
            let candidate: IceCandidate =
                serde_json::from_str(&candidate_json).map_err(json_err)?;
            let mut engine = self.inner.lock();
            engine
                .add_ice_candidate(PeerHandle::from(handle as u64), &candidate)
                .map_err(map_err)
        }))
        .map_err(|e| {
            let msg = if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else {
                "Unknown panic".to_string()
            };
            log::error!("add_ice_candidate panicked: {}", msg);
            Error::from_reason(format!("add_ice_candidate panicked: {}", msg))
        })?
    }

    /// 关闭 P2P 连接。
    #[napi]
    pub fn close_p2p(&self, handle: i64) -> Result<()> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = self.runtime.enter();
            let mut engine = self.inner.lock();
            engine.close_p2p(PeerHandle::from(handle as u64));
            Ok(())
        }))
        .map_err(|e| {
            let msg = if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else {
                "Unknown panic".to_string()
            };
            log::error!("close_p2p panicked: {}", msg);
            Error::from_reason(format!("close_p2p panicked: {}", msg))
        })?
    }

    /// 为 P2P 连接绑定本地音频 track。
    ///
    /// 创建 NativeAudioSource → RtcAudioTrack → add_track 到 PeerConnection。
    /// 之后可通过 `pushP2pAudioFrame` 推送 PCM 音频数据，内部自动进行 Opus 编码和 RTP 发送。
    #[napi]
    pub fn attach_p2p_audio(&self, handle: i64) -> Result<()> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = self.runtime.enter();
            let mut engine = self.inner.lock();
            engine
                .attach_p2p_audio(PeerHandle::from(handle as u64))
                .map_err(map_err)
        }))
        .map_err(|e| {
            let msg = if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else {
                "Unknown panic".to_string()
            };
            log::error!("attach_p2p_audio panicked: {}", msg);
            Error::from_reason(format!("attach_p2p_audio panicked: {}", msg))
        })?
    }

    /// 推送 PCM 音频帧到 P2P 音频源。
    ///
    /// `data` - Uint8Array，内部自动按小端字节序解析为 i16 PCM 采样。
    /// `sample_rate` - 采样率（需与 attachP2pAudio 时的参数一致）
    /// `channels` - 声道数（需与 attachP2pAudio 时的参数一致）
    /// `samples_per_channel` - 每声道采样数
    #[napi]
    pub fn push_p2p_audio_frame(
        &self,
        handle: i64,
        data: Uint8Array,
        sample_rate: u32,
        channels: u32,
        samples_per_channel: u32,
    ) -> Result<()> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = self.runtime.enter();
            // Uint8Array → &[i16] 转换（小端字节序）
            let bytes = data.as_ref();
            let i16_data: Vec<i16> = bytes
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect();
            let engine = self.inner.lock();
            engine
                .push_p2p_audio_frame(
                    PeerHandle::from(handle as u64),
                    &i16_data,
                    sample_rate,
                    channels,
                    samples_per_channel,
                )
                .map_err(map_err)
        }))
        .map_err(|e| {
            let msg = if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else {
                "Unknown panic".to_string()
            };
            log::error!("push_p2p_audio_frame panicked: {}", msg);
            Error::from_reason(format!("push_p2p_audio_frame panicked: {}", msg))
        })?
    }

    /// 推送远端参考帧用于 AEC（回声消除）。
    #[napi]
    pub fn push_p2p_reference_frame(&self, handle: i64, data: Vec<i16>) {
        let engine = self.inner.lock();
        engine.push_p2p_reference_frame(PeerHandle::from(handle as u64), &data);
    }

    /// 从远端 P2P audio track 创建音频流（用于播放远端音频）。
    ///
    /// 调用时机：在 pollEvents() 返回 P2pRemoteTrack (kind=Audio) 事件后调用。
    ///
    /// `handle` - P2P 连接句柄
    /// `track_id` - 远端 audio track ID（来自 P2pRemoteTrack 事件）
    ///
    /// 返回 [`LkAudioStream`]，可通过 `nextFrame()` 读取解码后的 PCM 帧，
    /// 然后写入 OHOS AudioRenderer 播放。
    #[napi]
    pub fn create_p2p_audio_stream(&self, handle: i64, track_id: String) -> Result<LkAudioStream> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = self.runtime.enter();
            let engine = self.inner.lock();
            let rtc_track: RtcAudioTrack = engine
                .take_p2p_remote_audio_track(
                    PeerHandle::from(handle as u64),
                    &track_id,
                )
                .ok_or_else(|| {
                    Error::from_reason(format!(
                        "remote audio track '{}' not found for handle {}",
                        track_id, handle
                    ))
                })?;

            let native = NativeAudioStream::new(rtc_track, 48_000, 1);
            log::info!(
                "[NAPI] create_p2p_audio_stream: handle={}, track_id={}",
                handle,
                track_id,
            );
            Ok(LkAudioStream::from_native(native))
        }))
        .map_err(|e| {
            let msg = if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else {
                "Unknown panic".to_string()
            };
            log::error!("create_p2p_audio_stream panicked: {}", msg);
            Error::from_reason(format!("create_p2p_audio_stream panicked: {}", msg))
        })?
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
        let _guard = self.runtime.enter();
        let config: SfuConfig = serde_json::from_str(&config_json).map_err(json_err)?;
        let mut engine = self.inner.lock();
        let handle = engine.create_sfu_session(config);
        Ok(handle.as_u64() as i64)
    }

    /// 连接 SFU 会话到 LiveKit 服务器。
    #[napi]
    pub fn connect_sfu(&self, handle: i64) -> Result<()> {
        let _guard = self.runtime.enter();
        let mut engine = self.inner.lock();
        engine
            .connect_sfu(SfuHandle::from(handle as u64))
            .map_err(map_err)
    }

    /// 断开 SFU 连接。
    #[napi]
    pub fn disconnect_sfu(&self, handle: i64) -> Result<()> {
        let _guard = self.runtime.enter();
        let mut engine = self.inner.lock();
        engine.disconnect_sfu(SfuHandle::from(handle as u64));
        Ok(())
    }

    /// 关闭（销毁）SFU 会话。
    #[napi]
    pub fn close_sfu(&self, handle: i64) -> Result<()> {
        let _guard = self.runtime.enter();
        let mut engine = self.inner.lock();
        engine.close_sfu(SfuHandle::from(handle as u64));
        Ok(())
    }

    /// SFU 是否可用（至少有一个已连接的会话）。
    #[napi]
    pub fn is_sfu_available(&self) -> Result<bool> {
        let _guard = self.runtime.enter();
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
        let _guard = self.runtime.enter();
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
        let _guard = self.runtime.enter();
        let mut engine = self.inner.lock();
        engine.shutdown();
        Ok(())
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
}
