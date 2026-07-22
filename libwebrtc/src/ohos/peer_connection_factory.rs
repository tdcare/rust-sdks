// Copyright 2025 LiveKit, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
// OHOS implementation - pure Rust stub backed by webrtc-rs/rtc.
//
// The OHOS factory is intentionally lightweight: it owns no global state
// because the rtc crate exposes its peer connection builder as a per-call
// API. Audio device management is forwarded to the application layer (the
// OHOS host integrates ADM independently) and therefore returns sensible
// defaults here.

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};

use tokio::net::UdpSocket;
use tokio::sync::mpsc;

use rtc::peer_connection::configuration::media_engine::{
    MediaEngine, MIME_TYPE_H264, MIME_TYPE_OPUS, MIME_TYPE_PCMA, MIME_TYPE_PCMU, MIME_TYPE_VP8,
};
use rtc::peer_connection::configuration::{RTCConfigurationBuilder, RTCIceServer};
use rtc::peer_connection::RTCPeerConnectionBuilder;
use rtc::rtp_transceiver::rtp_sender::{
    RTCPFeedback, RTCRtpCodec, RTCRtpCodecParameters, RtpCodecKind,
};

use super::audio_track::RtcAudioTrack as ImpAudioTrack;
use super::media_stream_track::MediaStreamTrack as ImpMediaStreamTrack;
use super::peer_connection::PeerConnection as ImpPeerConnection;
use super::rtc_io_driver::RtcIoDriver;
use super::video_track::RtcVideoTrack as ImpVideoTrack;
use crate::{
    audio_source::native::NativeAudioSource,
    audio_track::RtcAudioTrack,
    peer_connection::PeerConnection,
    peer_connection_factory::{IceServer, IceTransportsType, RtcConfiguration},
    rtp_parameters::{RtpCapabilities, RtpCodecCapability, RtpHeaderExtensionCapability},
    rtp_transceiver::RtpTransceiverDirection,
    video_source::native::NativeVideoSource,
    video_track::RtcVideoTrack,
    MediaType, RtcError, RtcErrorType,
};

/// OHOS peer connection factory.
///
/// Cheap to clone, default-constructible. Per-connection setup happens
/// in [`PeerConnectionFactory::create_peer_connection`] which spawns an
/// [`RtcIoDriver`] task to drive the rtc-crate state machine.
#[derive(Clone, Default)]
pub struct PeerConnectionFactory {
    // Reserved for future global state such as a shared codec registry or a
    // background ICE agent. Kept private so it can be extended without API
    // churn.
    _private: (),
}

impl PeerConnectionFactory {
    /// Stub: OHOS does not implement zero-playout-delay mode through field trials.
    pub fn with_zero_playout_delay() -> Self {
        Self::default()
    }

    pub fn zero_playout_delay_enabled(&self) -> bool {
        false
    }

    pub fn create_peer_connection(
        &self,
        config: RtcConfiguration,
    ) -> Result<PeerConnection, RtcError> {
        // Build the rtc-crate configuration from the public layer's
        // `RtcConfiguration`. Most of the fine-grained options (bundle
        // policy, ICE candidate pool, certificates) aren't represented in
        // the public layer yet, so we fall back to defaults.
        let ice_servers: Vec<RTCIceServer> = config
            .ice_servers
            .iter()
            .map(convert_ice_server)
            .collect();
        let rtc_config = RTCConfigurationBuilder::new()
            .with_ice_servers(ice_servers)
            .build();

        // Register only codecs that the OHOS device actually supports.
        // video_capabilities() lists H264 and VP8; audio_capabilities()
        // lists Opus, PCMU, and PCMA.  Avoid registering H265, VP9, AV1,
        // etc. which would leak into SDP offers and never decode properly.
        let mut media_engine = MediaEngine::default();

        // ---- Audio codecs ----
        for codec in [
            RTCRtpCodecParameters {
                rtp_codec: RTCRtpCodec {
                    mime_type: MIME_TYPE_OPUS.to_owned(),
                    clock_rate: 48000,
                    channels: 2,
                    sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                    rtcp_feedback: vec![],
                },
                payload_type: 111,
            },
            RTCRtpCodecParameters {
                rtp_codec: RTCRtpCodec {
                    mime_type: MIME_TYPE_PCMU.to_owned(),
                    clock_rate: 8000,
                    channels: 0,
                    sdp_fmtp_line: String::new(),
                    rtcp_feedback: vec![],
                },
                payload_type: 0,
            },
            RTCRtpCodecParameters {
                rtp_codec: RTCRtpCodec {
                    mime_type: MIME_TYPE_PCMA.to_owned(),
                    clock_rate: 8000,
                    channels: 0,
                    sdp_fmtp_line: String::new(),
                    rtcp_feedback: vec![],
                },
                payload_type: 8,
            },
        ] {
            media_engine.register_codec(codec, RtpCodecKind::Audio).map_err(
                |err| RtcError {
                    error_type: RtcErrorType::Internal,
                    message: format!("failed to register audio codec: {err}"),
                },
            )?;
        }

        let video_rtcp_feedback = vec![
            RTCPFeedback { typ: "goog-remb".to_owned(), parameter: String::new() },
            RTCPFeedback { typ: "ccm".to_owned(), parameter: "fir".to_owned() },
            RTCPFeedback { typ: "nack".to_owned(), parameter: String::new() },
            RTCPFeedback { typ: "nack".to_owned(), parameter: "pli".to_owned() },
        ];

        // ---- Video codecs ----
        // H264 first (preferred for devices with hardware encoder like Mate X5),
        // VP8 second (software fallback for devices without H264 hardware)
        for codec in [
            // H264 @ PT=125 (primary — preferred when hardware encoder available)
            RTCRtpCodecParameters {
                rtp_codec: RTCRtpCodec {
                    mime_type: MIME_TYPE_H264.to_owned(),
                    clock_rate: 90000,
                    channels: 0,
                    sdp_fmtp_line:
                        "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=64001e"
                            .to_owned(),
                    rtcp_feedback: video_rtcp_feedback.clone(),
                },
                payload_type: 125,
            },
            // VP8 @ PT=96 (fallback — used when H264 encoder unavailable)
            RTCRtpCodecParameters {
                rtp_codec: RTCRtpCodec {
                    mime_type: MIME_TYPE_VP8.to_owned(),
                    clock_rate: 90000,
                    channels: 0,
                    sdp_fmtp_line: String::new(),
                    rtcp_feedback: video_rtcp_feedback.clone(),
                },
                payload_type: 96,
            },
        ] {
            media_engine.register_codec(codec, RtpCodecKind::Video).map_err(
                |err| RtcError {
                    error_type: RtcErrorType::Internal,
                    message: format!("failed to register video codec: {err}"),
                },
            )?;
        }

        let pc = RTCPeerConnectionBuilder::new()
            .with_configuration(rtc_config)
            .with_media_engine(media_engine)
            .build()
            .map_err(|err| RtcError {
                error_type: RtcErrorType::Internal,
                message: format!("failed to build RTCPeerConnection: {err}"),
            })?;

        // Bind a UDP socket on an ephemeral port. We bind synchronously
        // via `std::net::UdpSocket` so this method can stay non-async,
        // then convert into a tokio socket for the driver task.
        let std_sock =
            std::net::UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], 0))).map_err(|err| {
                RtcError {
                    error_type: RtcErrorType::Internal,
                    message: format!("failed to bind UDP socket: {err}"),
                }
            })?;
        std_sock.set_nonblocking(true).map_err(|err| RtcError {
            error_type: RtcErrorType::Internal,
            message: format!("failed to put UDP socket into non-blocking mode: {err}"),
        })?;
        let local_addr = std_sock.local_addr().map_err(|err| RtcError {
            error_type: RtcErrorType::Internal,
            message: format!("failed to read UDP local_addr: {err}"),
        })?;

        // Resolve the real local IP address for ICE host candidates.
        // Binding to 0.0.0.0 gives us local_addr.ip() == 0.0.0.0, which
        // the remote peer cannot connect to.  Use a connected UDP socket
        // trick to ask the kernel which interface would be used to reach
        // an external address; the returned IP is the device's actual
        // network-facing address (e.g. 192.168.1.93).
        let real_ip = resolve_local_ip().unwrap_or_else(|| {
            log::warn!(
                "peer_connection_factory: could not resolve local IP, \
                 falling back to {}. ICE connectivity may be impaired.",
                local_addr.ip()
            );
            local_addr.ip()
        });
        let local_addr_for_ice = SocketAddr::new(real_ip, local_addr.port());
        log::info!(
            "peer_connection_factory: bound UDP socket on {local_addr}, \
             using {local_addr_for_ice} for ICE host candidates"
        );

        let socket = Arc::new(UdpSocket::from_std(std_sock).map_err(|err| RtcError {
            error_type: RtcErrorType::Internal,
            message: format!("failed to register UDP socket with tokio: {err}"),
        })?);

        // Build multi-transport manager (UDP + optional TCP).
        let mut transport = super::transport_manager::TransportManager::new(
            socket,
            local_addr_for_ice,
        );

        // Optionally bind a TCP listener if the ICE config allows TCP candidates.
        // We bind on an ephemeral port; the actual port is injected as a TCP
        // host candidate after `set_local_description` (see RtcIoDriver).
        if config.ice_transport_type == IceTransportsType::All {
            log::info!("peer_connection_factory: attempting TCP listener bind...");
            if let Ok(tcp_std) = std::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], 0))) {
                tcp_std.set_nonblocking(true).ok();
                if let Ok(tcp_tokio) = tokio::net::TcpListener::from_std(tcp_std) {
                    let tcp_addr = tcp_tokio.local_addr().unwrap_or(
                        SocketAddr::from(([0, 0, 0, 0], 0)),
                    );
                    // Replace with the real IP for ICE.
                    let tcp_addr_for_ice = SocketAddr::new(real_ip, tcp_addr.port());
                    transport.tcp_listener = Some(tcp_tokio);
                    transport.local_tcp_addr = Some(tcp_addr_for_ice);
                    log::info!(
                        "peer_connection_factory: TCP listener bound on {} (ICE: {})",
                        tcp_addr, tcp_addr_for_ice
                    );
                } else {
                    log::warn!("peer_connection_factory: failed to convert TCP listener to tokio");
                }
            } else {
                log::warn!("peer_connection_factory: failed to bind TCP listener");
            }
        } else {
            log::info!(
                "peer_connection_factory: TCP not enabled (ice_transport_type={:?})",
                config.ice_transport_type
            );
        }

        // Wire up the channel pair and spawn the driver task.
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let driver = RtcIoDriver::new(pc, transport, local_addr_for_ice, cmd_rx, event_tx);
        livekit_runtime::spawn(driver.run());

        let peer_connection = ImpPeerConnection::new(config, cmd_tx);
        peer_connection.spawn_event_consumer(event_rx);
        Ok(PeerConnection { handle: peer_connection })
    }

    pub fn create_video_track(&self, label: &str, source: NativeVideoSource) -> RtcVideoTrack {
        // Construct a fresh OHOS-side track tied to the supplied source.
        // The RTP send pipeline isn't created until the track is added to a
        // peer connection (the factory has no command channel of its own);
        // see [`PeerConnection::add_track`] for the wiring.
        let imp_track = ImpVideoTrack::with_source(
            ImpMediaStreamTrack::new(label.to_owned(), MediaType::Video),
            source.handle.clone(),
        );
        RtcVideoTrack { handle: imp_track }
    }

    pub fn create_audio_track(&self, label: &str, source: NativeAudioSource) -> RtcAudioTrack {
        let imp_track = ImpAudioTrack::with_source(
            ImpMediaStreamTrack::new(label.to_owned(), MediaType::Audio),
            source.handle.clone(),
        );
        RtcAudioTrack { handle: imp_track }
    }

    pub fn create_device_audio_track(&self, _label: &str) -> RtcAudioTrack {
        // TODO(ohos): Wire up Platform ADM-backed capture once available.
        unimplemented!(
            "create_device_audio_track is not yet implemented for the OHOS backend"
        )
    }

    pub fn get_rtp_sender_capabilities(&self, media_type: MediaType) -> RtpCapabilities {
        match media_type {
            MediaType::Audio => audio_capabilities(),
            MediaType::Video => video_capabilities(),
            _ => RtpCapabilities { codecs: Vec::new(), header_extensions: Vec::new() },
        }
    }

    pub fn get_rtp_receiver_capabilities(&self, media_type: MediaType) -> RtpCapabilities {
        // OHOS receiver capabilities mirror the sender side: the same codec
        // set is wired through `rtc-rtp` for both directions.
        self.get_rtp_sender_capabilities(media_type)
    }

    // ===== Device Management Methods =====
    //
    // OHOS audio devices are managed at the application layer (HarmonyOS
    // OH_Audio APIs) rather than by libwebrtc. The methods below preserve
    // the cross-platform API surface but report no devices and accept no
    // configuration changes.

    pub fn playout_devices(&self) -> i16 {
        0
    }

    pub fn recording_devices(&self) -> i16 {
        0
    }

    pub fn playout_device_name(&self, _index: u16) -> String {
        String::new()
    }

    pub fn recording_device_name(&self, _index: u16) -> String {
        String::new()
    }

    pub fn playout_device_guid(&self, _index: u16) -> String {
        String::new()
    }

    pub fn recording_device_guid(&self, _index: u16) -> String {
        String::new()
    }

    pub fn set_playout_device(&self, _index: u16) -> bool {
        false
    }

    pub fn set_recording_device(&self, _index: u16) -> bool {
        false
    }

    pub fn set_playout_device_by_guid(&self, _guid: &str) -> bool {
        false
    }

    pub fn set_recording_device_by_guid(&self, _guid: &str) -> bool {
        false
    }

    pub fn stop_recording(&self) -> bool {
        true
    }

    pub fn init_recording(&self) -> bool {
        true
    }

    pub fn start_recording(&self) -> bool {
        true
    }

    pub fn recording_is_initialized(&self) -> bool {
        false
    }

    pub fn stop_playout(&self) -> bool {
        true
    }

    pub fn init_playout(&self) -> bool {
        true
    }

    pub fn start_playout(&self) -> bool {
        true
    }

    pub fn playout_is_initialized(&self) -> bool {
        false
    }

    // ===== Built-in Audio Processing =====
    //
    // OHOS does not expose hardware AEC/AGC/NS through this API; the
    // application is expected to configure them via OH_AudioCapturer.

    pub fn builtin_aec_is_available(&self) -> bool {
        false
    }

    pub fn builtin_agc_is_available(&self) -> bool {
        false
    }

    pub fn builtin_ns_is_available(&self) -> bool {
        false
    }

    pub fn enable_builtin_aec(&self, _enable: bool) -> bool {
        false
    }

    pub fn enable_builtin_agc(&self, _enable: bool) -> bool {
        false
    }

    pub fn enable_builtin_ns(&self, _enable: bool) -> bool {
        false
    }

    // ===== ADM Lifecycle =====
    //
    // OHOS ships without a libwebrtc AudioDeviceModule. We accept the
    // public API but always report "inactive" so the host application
    // routes audio itself.

    pub fn set_adm_recording_enabled(&self, _enabled: bool) {}

    pub fn adm_recording_enabled(&self) -> bool {
        false
    }

    pub fn set_adm_playout_enabled(&self, _enabled: bool) {}

    pub fn adm_playout_enabled(&self) -> bool {
        false
    }

    pub fn acquire_platform_adm(&self) -> bool {
        false
    }

    pub fn release_platform_adm(&self) {}

    pub fn platform_adm_ref_count(&self) -> i32 {
        0
    }

    pub fn is_platform_adm_active(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// User-provided local IP address, set by the host application (e.g. ArkTS)
/// when the device IP is known externally. Takes priority over `resolve_local_ip()`.
static USER_LOCAL_IP: std::sync::OnceLock<std::net::IpAddr> = std::sync::OnceLock::new();

/// Set the local IP address to be used for ICE host candidates.
/// Call this once during initialization with the device's actual network IP.
pub fn set_user_local_ip(ip: std::net::IpAddr) {
    let _ = USER_LOCAL_IP.set(ip);
}

/// Resolve the device's actual network-facing IPv4 address.
///
/// First checks the user-provided IP set via [`set_user_local_ip`].
/// Falls back to the connected-UDP-socket trick if not set.
///
/// Binding a UDP socket to `0.0.0.0:0` gives us an ephemeral port but
/// `local_addr().ip()` returns `0.0.0.0`, which is unusable in ICE host
/// candidates.  This helper uses a connected-UDP-socket trick: it creates
/// a temporary socket, calls `connect()` to a well-known external address
/// (no packets are actually sent), then reads `local_addr()`.  The kernel
/// resolves the route and returns the interface IP that would be used to
/// reach that destination.
///
/// Falls back to `None` if no network interface is configured, in which
/// case the caller should use the bound address as-is.
fn resolve_local_ip() -> Option<std::net::IpAddr> {
    // 1. Check user-provided IP (from ArkTS via set_user_local_ip).
    if let Some(ip) = USER_LOCAL_IP.get() {
        if ip.is_ipv4() && !ip.is_unspecified() && !ip.is_loopback() {
            return Some(*ip);
        }
    }
    // 2. Try the connected-socket trick with common gateway/DNS addresses.
    for probe in &["8.8.8.8:53", "1.1.1.1:53", "192.168.1.1:53"] {
        if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") {
            if sock.connect(probe).is_ok() {
                if let Ok(addr) = sock.local_addr() {
                    let ip = addr.ip();
                    // Only accept IPv4 — SDP declares "IN IP4" and IPv6
                    // addresses in an IP4 field cause parse errors.
                    if ip.is_ipv4() && !ip.is_unspecified() && !ip.is_loopback() {
                        return Some(ip);
                    }
                }
            }
        }
    }
    None
}

fn convert_ice_server(server: &IceServer) -> RTCIceServer {
    RTCIceServer {
        urls: server.urls.clone(),
        username: server.username.clone(),
        // Public layer calls it `password`; the rtc crate calls it
        // `credential`. Same RFC field, different names.
        credential: server.password.clone(),
    }
}

// ---------------------------------------------------------------------------
// RTP capabilities
// ---------------------------------------------------------------------------
//
// OHOS does not expose a runtime media-engine query, so we report a fixed
// capability set that matches what `rtc-rtp` registers for the platform:
// Opus / PCMU / PCMA on the audio side and H.264 / VP8 on the video side.
// The lists are cached in `OnceLock`s to avoid reallocating on every call.

static AUDIO_CAPS: OnceLock<RtpCapabilities> = OnceLock::new();
static VIDEO_CAPS: OnceLock<RtpCapabilities> = OnceLock::new();

fn audio_capabilities() -> RtpCapabilities {
    AUDIO_CAPS
        .get_or_init(|| RtpCapabilities {
            codecs: vec![
                RtpCodecCapability {
                    mime_type: "audio/opus".to_string(),
                    clock_rate: Some(48000),
                    channels: Some(2),
                    sdp_fmtp_line: Some("minptime=10;useinbandfec=1".to_string()),
                },
                RtpCodecCapability {
                    mime_type: "audio/PCMU".to_string(),
                    clock_rate: Some(8000),
                    channels: Some(1),
                    sdp_fmtp_line: None,
                },
                RtpCodecCapability {
                    mime_type: "audio/PCMA".to_string(),
                    clock_rate: Some(8000),
                    channels: Some(1),
                    sdp_fmtp_line: None,
                },
            ],
            header_extensions: vec![RtpHeaderExtensionCapability {
                uri: "urn:ietf:params:rtp-hdrext:ssrc-audio-level".to_string(),
                direction: RtpTransceiverDirection::SendRecv,
            }],
        })
        .clone()
}

fn video_capabilities() -> RtpCapabilities {
    VIDEO_CAPS
        .get_or_init(|| RtpCapabilities {
            codecs: vec![
                RtpCodecCapability {
                    mime_type: "video/H264".to_string(),
                    clock_rate: Some(90000),
                    channels: None,
                    sdp_fmtp_line: Some(
                        "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=64001e"
                            .to_string(),
                    ),
                },
                RtpCodecCapability {
                    mime_type: "video/VP8".to_string(),
                    clock_rate: Some(90000),
                    channels: None,
                    sdp_fmtp_line: None,
                },
            ],
            header_extensions: vec![
                RtpHeaderExtensionCapability {
                    uri: "urn:ietf:params:rtp-hdrext:toffset".to_string(),
                    direction: RtpTransceiverDirection::SendRecv,
                },
                RtpHeaderExtensionCapability {
                    uri: "http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time".to_string(),
                    direction: RtpTransceiverDirection::SendRecv,
                },
            ],
        })
        .clone()
}
