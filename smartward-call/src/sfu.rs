//! SFU 引擎 — 基于 LiveKit 的跨科室/广播通信
//!
//! 不包含信令逻辑。上层负责：
//! 1. 获取 LiveKit token 后调用 `create_session()`
//! 2. 调用 `connect()` 连接到 LiveKit 房间
//! 3. 从 `poll_events()` 获取连接状态和 track 事件

use std::collections::HashMap;
use crate::types::*;

// ============================================================
// 内部会话
// ============================================================

struct SfuSession {
    config: SfuConfig,
    state: SfuState,
    events: Vec<EngineEvent>,
}

impl SfuSession {
    fn new(config: SfuConfig) -> Self {
        Self { config, state: SfuState::Disconnected, events: Vec::new() }
    }

    fn transition(&mut self, new_state: SfuState, handle: SfuHandle) {
        if self.state != new_state {
            log::info!("[SFU] {:?} {:?} → {:?}", handle, self.state, new_state);
            self.state = new_state;
            match new_state {
                SfuState::Connected => {
                    self.events.push(EngineEvent::SfuConnected {
                        handle,
                        room_name: self.config.room_name.clone(),
                    });
                }
                SfuState::Disconnected => {
                    self.events.push(EngineEvent::SfuDisconnected { handle });
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
        let session = self.sessions.get_mut(&handle).ok_or(RtcError::InvalidHandle)?;
        session.transition(SfuState::Connecting, handle);
        self.do_connect(handle)
    }

    #[cfg(feature = "call-sfu")]
    fn do_connect(&mut self, handle: SfuHandle) -> Result<(), RtcError> {
        // TODO: 使用 livekit::Room 连接
        // let room = livekit::Room::new();
        // room.connect(&session.config.room_name, &session.config.token).await?;
        let session = self.sessions.get_mut(&handle).unwrap();
        session.transition(SfuState::Connected, handle);
        Ok(())
    }

    #[cfg(not(feature = "call-sfu"))]
    fn do_connect(&mut self, handle: SfuHandle) -> Result<(), RtcError> {
        log::warn!("[SFU] {:?} connect: using stub (enable call-sfu feature)", handle);
        let session = self.sessions.get_mut(&handle).unwrap();
        session.transition(SfuState::Connected, handle);
        Ok(())
    }

    /// 断开 LiveKit 连接
    pub fn disconnect(&mut self, handle: SfuHandle) {
        if let Some(session) = self.sessions.get_mut(&handle) {
            session.transition(SfuState::Disconnected, handle);
            #[cfg(feature = "call-sfu")]
            {
                // TODO: room.disconnect()
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
        for session in self.sessions.values_mut() {
            events.append(&mut session.events);
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> SfuConfig {
        SfuConfig {
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
    fn test_connect_stub() {
        let mut mgr = SfuManager::new();
        let h = mgr.create_session(test_config());
        mgr.connect(h).unwrap();
        assert_eq!(mgr.state(h), Some(SfuState::Connected));
        let events = mgr.drain_events();
        assert!(events.iter().any(|e| matches!(e, EngineEvent::SfuConnected { .. })));
    }

    #[test]
    fn test_disconnect() {
        let mut mgr = SfuManager::new();
        let h = mgr.create_session(test_config());
        mgr.connect(h).unwrap();
        mgr.disconnect(h);
        assert_eq!(mgr.state(h), Some(SfuState::Disconnected));
    }

    #[test]
    fn test_invalid_handle() {
        let mut mgr = SfuManager::new();
        assert!(mgr.connect(SfuHandle(999)).is_err());
    }
}
