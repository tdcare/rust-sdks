//! 会话路由器 — 纯函数式 P2P/SFU 模式决策
//!
//! 上层根据业务场景调用 `resolve()` 决定使用哪种传输模式，
//! 然后选择对应的 `WebRtcEngine` API 发起连接。
//!
//! # 决策矩阵
//!
//! | 场景                     | 判定条件                  | 传输  |
//! |--------------------------|---------------------------|-------|
//! | 科内点对点 (床头→护士站)  | local_ward == target_ward | P2P   |
//! | 跨科室会诊               | local_ward != target_ward | SFU   |
//! | 病区广播 / 全院广播      | is_broadcast == true      | SFU   |

use crate::types::TransportMode;

/// 根据病区和场景决定传输模式
///
/// # 参数
/// - `local_ward`: 本机所在病区编号
/// - `target_ward`: 目标病区编号
/// - `is_broadcast`: 是否为一对多广播
///
/// # 示例
/// ```
/// use smartward_call::{SessionRouter, TransportMode};
///
/// // 科内呼叫 → P2P
/// assert_eq!(SessionRouter::resolve("wardA", "wardA", false), TransportMode::P2P);
///
/// // 跨科室 → SFU
/// assert_eq!(SessionRouter::resolve("wardA", "wardB", false), TransportMode::SFU);
///
/// // 广播 → SFU
/// assert_eq!(SessionRouter::resolve("wardA", "wardA", true), TransportMode::SFU);
/// ```
pub struct SessionRouter;

impl SessionRouter {
    pub fn resolve(local_ward: &str, target_ward: &str, is_broadcast: bool) -> TransportMode {
        if is_broadcast || local_ward != target_ward {
            TransportMode::SFU
        } else {
            TransportMode::P2P
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_p2p() {
        assert_eq!(SessionRouter::resolve("wardA", "wardA", false), TransportMode::P2P);
    }

    #[test]
    fn test_cross_ward_sfu() {
        assert_eq!(SessionRouter::resolve("wardA", "wardB", false), TransportMode::SFU);
    }

    #[test]
    fn test_broadcast_sfu() {
        assert_eq!(SessionRouter::resolve("wardA", "wardA", true), TransportMode::SFU);
    }

    #[test]
    fn test_cross_ward_broadcast_sfu() {
        assert_eq!(SessionRouter::resolve("wardA", "wardB", true), TransportMode::SFU);
    }
}

