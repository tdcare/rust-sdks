//! OHOS / Android platform WebRTC implementation
//! Based on webrtc-rs/rtc pure Rust implementation with platform-specific hardware codec integration.

use std::sync::atomic::AtomicBool;

/// Debug flag: when true, bypass H.264 hardware encoder entirely and force
/// VP8 software encoding for both the actual encoder AND the SDP negotiation.
/// This ensures the SDP offer and the actual codec stay in sync.
pub static FORCE_VP8: AtomicBool = AtomicBool::new(false);

pub mod peer_connection;
pub mod peer_connection_factory;
pub mod rtc_io_driver;
pub mod transport_manager;
pub mod audio_source;
pub mod audio_track;
pub mod video_source;
pub mod video_track;
pub mod video_frame;
pub mod data_channel;
pub mod media_stream;
pub mod media_stream_track;
pub mod rtp_parameters;
pub mod rtp_sender;
pub mod rtp_receiver;
pub mod rtp_transceiver;
pub mod session_description;
pub mod ice_candidate;
pub mod audio_resampler;
pub mod audio_mixer;
pub mod audio_stream;
pub mod video_stream;
pub mod frame_cryptor;
pub mod packet_trailer;
pub mod yuv_helper;
pub mod apm;
pub mod video_codec;
pub mod image_processing;
pub mod rtp_packetizer;
pub mod rtp_send_pipeline;
pub mod opus_decoder;
pub mod vp8_encoder;
#[cfg(target_env = "ohos")]
pub mod h264_encoder;
#[cfg(all(target_os = "android", not(target_env = "ohos")))]
#[path = "h264_encoder_android.rs"]
pub mod h264_encoder;
#[allow(non_camel_case_types, non_upper_case_globals, dead_code)]
pub mod libvpx_ffi;
#[allow(dead_code)]
pub mod software_vp8;
