//! Multi-transport (UDP + TCP) IO abstraction for the RTC sans-IO driver.
//!
//! OHOS platforms behind NAT routers often experience UDP port-mapping
//! expiry on data channels, causing "dtls timeout" disconnections even
//! while audio/video RTP streams are flowing.  Adding TCP transport
//! alongside UDP lets the ICE agent fall back to TCP (RFC 6544) when
//! UDP becomes unreliable — TCP connections are NAT-friendly because
//! they maintain their own kernel-level keepalive.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use rtc::shared::TransportProtocol;
use tokio::net::{TcpListener, TcpStream, UdpSocket};

/// Maximum size of a receive buffer.  DTLS handshake packets (with
/// certificate chains) can reach ~60 KiB, so we use 64 KiB to match
/// the existing UDP buffer.
const RECV_BUF_SIZE: usize = 65536;

/// Result of a single [`TransportManager::recv`] call.
#[derive(Debug)]
pub(crate) struct RecvResult {
    pub n: usize,
    pub peer_addr: SocketAddr,
    pub protocol: TransportProtocol,
}

/// Owns a UDP socket plus an optional set of TCP streams, exposing a
/// unified send/recv interface that hides transport selection from the
/// driver loop.
pub(crate) struct TransportManager {
    /// Primary UDP socket (always present — ICE connectivity checks rely on it).
    pub(crate) udp_socket: Arc<UdpSocket>,
    pub(crate) local_udp_addr: SocketAddr,

    /// TCP listener for passive ICE candidates.  Created when at least one
    /// TCP host candidate is configured.
    pub(crate) tcp_listener: Option<TcpListener>,
    pub(crate) local_tcp_addr: Option<SocketAddr>,

    /// Active outgoing TCP connections, keyed by peer address.
    tcp_streams: HashMap<SocketAddr, TcpStream>,

    /// Accumulator buffer for partial TCP reads (OpenHarmony TCP can
    /// fragment the DTLS record across multiple read calls).
    tcp_bufs: HashMap<SocketAddr, Vec<u8>>,
}

impl TransportManager {
    /// Create a manager with only UDP transport.  Call [`Self::bind_tcp`]
    /// afterwards if TCP ICE candidates are desired.
    pub(crate) fn new(
        udp_socket: Arc<UdpSocket>,
        local_udp_addr: SocketAddr,
    ) -> Self {
        Self {
            udp_socket,
            local_udp_addr,
            tcp_listener: None,
            local_tcp_addr: None,
            tcp_streams: HashMap::new(),
            tcp_bufs: HashMap::new(),
        }
    }

    /// Bind a TCP listener on `addr` (typically `0.0.0.0:0` for an
    /// ephemeral port).  Returns the bound address.
    pub(crate) async fn bind_tcp(
        &mut self,
        addr: SocketAddr,
    ) -> std::io::Result<SocketAddr> {
        let listener = TcpListener::bind(addr).await?;
        let local = listener.local_addr()?;
        self.local_tcp_addr = Some(local);
        self.tcp_listener = Some(listener);
        log::info!("[TransportManager] TCP listener bound on {}", local);
        Ok(local)
    }

    /// Connect to a remote TCP endpoint for an active ICE candidate.
    pub(crate) async fn connect_tcp(
        &mut self,
        peer: SocketAddr,
    ) -> std::io::Result<()> {
        let stream = TcpStream::connect(peer).await?;
        stream.set_nodelay(true)?;  // low-latency for DTLS
        log::info!("[TransportManager] TCP connected to {}", peer);
        self.tcp_streams.insert(peer, stream);
        Ok(())
    }

    /// Send `data` to `peer` using the transport specified by `protocol`.
    pub(crate) async fn send_to(
        &mut self,
        peer: SocketAddr,
        data: &[u8],
        protocol: TransportProtocol,
    ) -> std::io::Result<()> {
        match protocol {
            TransportProtocol::UDP => {
                self.udp_socket.send_to(data, peer).await.map(|_| ())
            }
            TransportProtocol::TCP => {
                use tokio::io::AsyncWriteExt;
                let stream = self.tcp_streams.get_mut(&peer).ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        format!("no TCP stream to {peer}"),
                    )
                })?;
                // Prefix each DTLS record with a 2-byte big-endian length
                // (RFC 4571 / RFC 6544 framing).
                let len = data.len() as u16;
                let framed = [len.to_be_bytes().as_slice(), data].concat();
                stream.write_all(&framed).await
            }
        }
    }

    /// Block until data arrives on ANY transport, or the TCP listener
    /// accepts a new connection.  Returns `None` if the UDP socket was
    /// closed (poison pill — the driver should exit).
    pub(crate) async fn recv(&mut self, buf: &mut [u8]) -> Option<RecvResult> {
        use tokio::io::AsyncReadExt;

        // Poll the UDP socket first (most ICE/DTLS/RTP traffic stays on UDP).
        if let Ok((n, peer)) = self.udp_socket.recv_from(buf).await {
            return Some(RecvResult {
                n,
                peer_addr: peer,
                protocol: TransportProtocol::UDP,
            });
        }

        // Accept new TCP connections.
        if let Some(ref listener) = self.tcp_listener {
            if let Ok((stream, peer)) = listener.accept().await {
                stream.set_nodelay(true).ok();
                log::info!("[TransportManager] TCP accepted from {}", peer);
                self.tcp_streams.insert(peer, stream);
                // Return a zero-length "event" to wake the select loop;
                // the real data will arrive on the next recv.
                return Some(RecvResult {
                    n: 0,
                    peer_addr: peer,
                    protocol: TransportProtocol::TCP,
                });
            }
        }

        // Read from existing TCP streams.
        //
        // OHOS TCP can fragment DTLS records; we read into a per-peer
        // accumulation buffer and yield the first complete framed record.
        let mut closed_peers: Vec<SocketAddr> = Vec::new();
        let mut yielded = None;
        for (&peer, stream) in &mut self.tcp_streams {
            let mut tmp = [0u8; RECV_BUF_SIZE];
            match stream.try_read(&mut tmp) {
                Ok(0) => {
                    closed_peers.push(peer);
                    log::warn!("[TransportManager] TCP EOF from {}", peer);
                    continue;
                }
                Ok(n) => {
                    let acc = self.tcp_bufs.entry(peer).or_default();
                    acc.extend_from_slice(&tmp[..n]);

                    // Parse RFC 4571 length-prefixed records.
                    while acc.len() >= 2 {
                        let record_len =
                            u16::from_be_bytes([acc[0], acc[1]]) as usize;
                        if acc.len() < 2 + record_len {
                            break;
                        }
                        let payload = acc[2..2 + record_len].to_vec();
                        acc.drain(..2 + record_len);
                        let copy_len = payload.len().min(buf.len());
                        buf[..copy_len].copy_from_slice(&payload[..copy_len]);
                        yielded = Some(RecvResult {
                            n: copy_len,
                            peer_addr: peer,
                            protocol: TransportProtocol::TCP,
                        });
                        break;
                    }
                    if yielded.is_some() {
                        break;
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => {
                    log::warn!(
                        "[TransportManager] TCP read error from {}: {}",
                        peer, e
                    );
                    closed_peers.push(peer);
                }
            }
        }
        // Clean up closed connections after the borrow on self.tcp_streams ends.
        for peer in closed_peers {
            self.tcp_streams.remove(&peer);
            self.tcp_bufs.remove(&peer);
        }
        if let Some(result) = yielded {
            return Some(result);
        }

        None
    }
}
