//! Network interfaces module - OpenHarmony stub implementation
//! 
//! OpenHarmony 不支持 nix crate，此模块提供空实现
//! WebRTC sans-I/O 架构下，用户需要手动提供网络地址

use std::io::Error;
use std::net::SocketAddr;

/// Interface represents a local network interface
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    /// Interface name
    pub name: String,
    /// Interface kind (IPv4, IPv6, etc.)
    pub kind: Kind,
    /// Interface address
    pub addr: Option<SocketAddr>,
    /// Network mask
    pub mask: Option<SocketAddr>,
    /// Next hop information
    pub hop: Option<NextHop>,
}

/// Kind of network interface
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// IPv4 interface
    Ipv4,
    /// IPv6 interface
    Ipv6,
    /// Packet interface (Linux)
    Packet,
    /// Link interface (BSD)
    Link,
}

/// Next hop information
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextHop {
    /// Destination address
    Destination(SocketAddr),
    /// Broadcast address
    Broadcast(SocketAddr),
}

/// Query the local system for all interface addresses.
/// 
/// On OpenHarmony, this returns an empty list.
/// The caller should provide network addresses manually.
pub fn ifaces() -> Result<Vec<Interface>, Error> {
    // OpenHarmony stub: 返回空列表
    // Sans-I/O 架构下，用户需要手动指定本地地址
    Ok(Vec::new())
}
