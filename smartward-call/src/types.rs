//! 公共类型定义 — 纯 WebRTC 相关，无信令依赖
//!
//! 所有类型设计为 FFI 友好：无 async trait，无生命周期参数。

use serde::{Deserialize, Serialize};

// ============================================================
// 不透明句柄 (Opaque Handles)
// ============================================================

/// P2P 连接句柄
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PeerHandle(pub(crate) u64);

impl PeerHandle {
    /// Get the raw u64 value of this handle.
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl From<u64> for PeerHandle {
    fn from(v: u64) -> Self {
        PeerHandle(v)
    }
}

/// SFU 会话句柄
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SfuHandle(pub(crate) u64);

impl SfuHandle {
    /// Get the raw u64 value of this handle.
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl From<u64> for SfuHandle {
    fn from(v: u64) -> Self {
        SfuHandle(v)
    }
}

// ============================================================
// SDP 与会话描述
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SdpType {
    Offer,
    Answer,
    // PrAnswer 暂不支持
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDescription {
    #[serde(rename = "type")]
    pub sdp_type: SdpType,
    pub sdp: String,
}

// ============================================================
// ICE 候选
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IceCandidate {
    /// 媒体流标识
    pub sdp_mid: String,
    /// 媒体行索引
    pub sdp_m_line_index: i32,
    /// 候选字符串 (如 "candidate:...")
    pub candidate: String,
}

// ============================================================
// ICE 服务器配置
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IceServer {
    /// STUN/TURN 服务器 URL 列表
    ///
    /// 示例: `"stun:stun.xten.com:3478"` / `"turn:10.0.0.1:3478?transport=udp"`
    pub urls: Vec<String>,
    /// TURN 用户名
    pub username: Option<String>,
    /// TURN 密码
    pub credential: Option<String>,
}

// ============================================================
// 配置
// ============================================================

/// P2P 连接配置
///
/// 默认使用国内公开 STUN 服务器做 NAT 穿透，生产环境建议替换为医院自建 TURN/STUN。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2pConfig {
    /// ICE 服务器列表
    pub ice_servers: Vec<IceServer>,
}

impl Default for P2pConfig {
    fn default() -> Self {
        Self {
            ice_servers: vec![
                IceServer {
                    urls: vec!["stun:stun.xten.com:3478".into()],
                    username: None,
                    credential: None,
                },
                IceServer {
                    urls: vec!["stun:stun.miwifi.com:3478".into()],
                    username: None,
                    credential: None,
                },
            ],
        }
    }
}

/// SFU 会话配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SfuConfig {
    /// LiveKit 服务器地址 (如 "ws://10.0.0.1:7880")
    pub url: String,
    /// LiveKit 房间名
    pub room_name: String,
    /// 参与者标识
    pub participant_identity: String,
    /// LiveKit 访问令牌
    pub token: String,
}

// ============================================================
// 媒体
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MediaKind {
    Audio,
    Video,
}

// ============================================================
// 连接状态
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum P2pState {
    New,
    Connecting,
    Connected,
    Disconnected,
    Failed,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SfuState {
    Disconnected,
    Connecting,
    Connected,
    Failed,
}

// ============================================================
// 传输模式
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportMode {
    P2P,
    SFU,
}

// ============================================================
// 错误
// ============================================================

#[derive(Debug, thiserror::Error)]
pub enum RtcError {
    #[error("invalid handle")]
    InvalidHandle,

    #[error("invalid state: current={current}, expected={expected}")]
    InvalidState { current: String, expected: String },

    #[error("SDP error: {0}")]
    Sdp(String),

    #[error("ICE error: {0}")]
    Ice(String),

    #[error("SFU error: {0}")]
    Sfu(String),

    #[error("internal error: {0}")]
    Internal(String),
}

// ============================================================
// 引擎事件 (上层轮询获取)
// ============================================================

/// WebRTC 引擎产生的事件，由上层通过 `poll_events()` 拉取
#[derive(Debug, Clone, Serialize)]
pub enum EngineEvent {
    // ---- P2P 事件 ----

    /// 本地 ICE 候选已生成，上层需通过 MQTT 发送给对端
    P2pIceCandidate {
        handle: PeerHandle,
        candidate: IceCandidate,
    },

    /// P2P 连接状态变更
    P2pStateChange {
        handle: PeerHandle,
        state: P2pState,
    },

    /// P2P 远端 track 到达
    P2pRemoteTrack {
        handle: PeerHandle,
        track_id: String,
        kind: MediaKind,
    },

    // ---- SFU 事件 ----

    /// SFU 连接成功
    SfuConnected {
        handle: SfuHandle,
        room_name: String,
    },

    /// SFU 断开
    SfuDisconnected {
        handle: SfuHandle,
    },

    /// SFU 房间内有新 track
    SfuTrackSubscribed {
        handle: SfuHandle,
        participant_id: String,
        kind: MediaKind,
    },

    /// SFU 错误
    SfuError {
        handle: SfuHandle,
        message: String,
    },
}
