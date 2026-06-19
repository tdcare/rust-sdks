//! SFU 引擎 — 基于 LiveKit 的跨科室/广播通信
//!
//! 不包含信令逻辑。上层负责：
//! 1. 获取 LiveKit token 后调用 `create_session()`
//! 2. 调用 `connect()` 连接到 LiveKit 房间
//! 3. 从 `poll_events()` 获取连接状态和 track 事件
//!
//! # 运行时要求
//!
//! 调用方必须已进入 Tokio runtime，因为内部需要 spawn 后台任务来监听
//! LiveKit 房间事件。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::block_on;
use crate::types::*;

// ============================================================
// 内部会话
// ============================================================

struct SfuSession {
    config: SfuConfig,
    state: SfuState,
    /// LiveKit Room 实例，连接后持有
    room: Option<livekit::Room>,
    /// 待处理事件队列，后台任务通过 Arc 写入
    events: Arc<Mutex<Vec<EngineEvent>>>,
}

impl SfuSession {
    fn new(config: SfuConfig) -> Self {
        Self {
            config,
            state: SfuState::Disconnected,
            room: None,
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn transition(&mut self, new_state: SfuState, handle: SfuHandle) {
        if self.state != new_state {
            log::info!("[SFU] {:?} {:?} → {:?}", handle, self.state, new_state);
            self.state = new_state;
            match new_state {
                SfuState::Connected => {
                    self.events.lock().unwrap().push(EngineEvent::SfuConnected {
                        handle,
                        room_name: self.config.room_name.clone(),
                    });
                }
                SfuState::Disconnected => {
                    self.events.lock().unwrap().push(EngineEvent::SfuDisconnected { handle });
                }
                _ => {}
            }
        }
    }

    fn is_connected(&self) -> bool {
        matches!(self.state, SfuState::Connected)
    }
}

// ============================================================
// SFU 管理器
// ============================================================

pub(crate) struct SfuManager {
    sessions: HashMap<SfuHandle, SfuSession>,
    next_handle: u64,
}

impl SfuManager {
    pub fn new() -> Self {
        Self { sessions: HashMap::new(), next_handle: 1 }
    }

    // ---- 会话生命周期 ----

    /// 创建 SFU 会话，返回句柄
    pub fn create_session(&mut self, config: SfuConfig) -> SfuHandle {
        let handle = SfuHandle(self.next_handle);
        self.next_handle += 1;
        self.sessions.insert(handle, SfuSession::new(config));
        log::info!("[SFU] {:?} session created", handle);
        handle
    }

    /// 连接到 LiveKit 房间
    pub fn connect(&mut self, handle: SfuHandle) -> Result<(), RtcError> {
        if !self.sessions.contains_key(&handle) {
            return Err(RtcError::InvalidHandle);
        }

        match self.do_connect(handle) {
            Ok(()) => Ok(()),
            Err(e) => {
                if let Some(session) = self.sessions.get_mut(&handle) {
                    session.transition(SfuState::Failed, handle);
                }
                Err(e)
            }
        }
    }

    fn do_connect(&mut self, handle: SfuHandle) -> Result<(), RtcError> {
        // 标记为连接中
        {
            let session = self.sessions.get_mut(&handle).ok_or(RtcError::InvalidHandle)?;
            session.transition(SfuState::Connecting, handle);
        }

        let session = self.sessions.get(&handle).ok_or(RtcError::InvalidHandle)?;
        let url = session.config.url.clone();
        let token = session.config.token.clone();
        let events = session.events.clone();

        let (room, mut rx) =
            block_on(livekit::Room::connect(&url, &token, livekit::RoomOptions::default()))
                .map_err(|e| RtcError::Sfu(e.to_string()))?;

        // 存储 room 并切换到 Connected 状态
        let session = self.sessions.get_mut(&handle).unwrap();
        session.room = Some(room);
        session.transition(SfuState::Connected, handle);

        // 后台任务：将 LiveKit 房间事件转发为 EngineEvent
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    livekit::RoomEvent::TrackSubscribed { publication, participant, .. } => {
                        if let Ok(mut evts) = events.lock() {
                            evts.push(EngineEvent::SfuTrackSubscribed {
                                handle,
                                participant_id: participant.identity().to_string(),
                                kind: match publication.kind() {
                                    livekit::TrackKind::Audio => MediaKind::Audio,
                                    livekit::TrackKind::Video => MediaKind::Video,
                                },
                            });
                        }
                    }
                    _ => {}
                }
            }
            // rx 关闭 → 房间已断开，推送断开事件
            if let Ok(mut evts) = events.lock() {
                evts.push(EngineEvent::SfuDisconnected { handle });
            }
        });

        Ok(())
    }

    /// 断开 LiveKit 连接
    pub fn disconnect(&mut self, handle: SfuHandle) {
        if let Some(session) = self.sessions.get_mut(&handle) {
            session.transition(SfuState::Disconnected, handle);
            if let Some(room) = session.room.take() {
                let _ = block_on(room.close());
            }
        }
    }

    /// 关闭 SFU 会话
    pub fn close(&mut self, handle: SfuHandle) {
        self.disconnect(handle);
        self.sessions.remove(&handle);
        log::info!("[SFU] {:?} closed", handle);
    }

    /// 关闭所有会话
    pub fn close_all(&mut self) {
        let handles: Vec<SfuHandle> = self.sessions.keys().copied().collect();
        for h in handles {
            self.close(h);
        }
    }

    // ---- 状态查询 ----

    /// 检查 SFU 是否可用（已连接到 LiveKit 服务器）
    pub fn is_available(&self) -> bool {
        self.sessions.values().any(|s| s.is_connected())
    }

    /// 获取会话状态
    pub fn state(&self, handle: SfuHandle) -> Option<SfuState> {
        self.sessions.get(&handle).map(|s| s.state)
    }

    // ---- 事件 ----

    /// 获取 SFU 管理器产生的待处理事件并清空队列
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

    fn test_config() -> SfuConfig {
        SfuConfig {
            url: "ws://127.0.0.1:7880".into(),
            room_name: "ward-a-consult".into(),
            participant_identity: "nurse-ns001".into(),
            token: "test-token".into(),
        }
    }

    #[test]
    fn test_create_and_close() {
        let mut mgr = SfuManager::new();
        let h = mgr.create_session(test_config());
        assert!(mgr.sessions.contains_key(&h));
        mgr.close(h);
        assert!(!mgr.sessions.contains_key(&h));
    }

    #[test]
    fn test_connect_no_server() {
        let mut mgr = SfuManager::new();
        let h = mgr.create_session(test_config());
        // 本地无 LiveKit 服务器，connect 应失败
        assert!(mgr.connect(h).is_err());
        assert_eq!(mgr.state(h), Some(SfuState::Failed));
    }

    #[test]
    fn test_disconnect_idempotent() {
        let mut mgr = SfuManager::new();
        let h = mgr.create_session(test_config());
        // 未连接时 disconnect 应不崩溃
        mgr.disconnect(h);
        assert_eq!(mgr.state(h), Some(SfuState::Disconnected));
    }

    #[test]
    fn test_invalid_handle() {
        let mut mgr = SfuManager::new();
        assert!(mgr.connect(SfuHandle(999)).is_err());
    }
}
