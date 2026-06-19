//! VP8 software encoder wrapper around [`super::software_vp8::SoftwareVP8Encoder`].
//!
//! This module provides a thin adapter so that [`super::video_source`] can use
//! the same `encode() -> Option<(Vec<u8>, bool)>` interface for both the H.264
//! hardware encoder and the VP8 software encoder.

use crate::{RtcError, RtcErrorType};
use super::software_vp8::{SoftwareVP8Encoder, SoftwareVP8EncoderConfig};

fn err(msg: String) -> RtcError {
    RtcError { error_type: RtcErrorType::Internal, message: msg }
}

/// VP8 software encoder backed by libvpx.
pub struct Vp8Encoder {
    inner: SoftwareVP8Encoder,
    width: u32,
    height: u32,
}

unsafe impl Send for Vp8Encoder {}

impl Vp8Encoder {
    pub fn new(width: u32, height: u32, bitrate_kbps: u32) -> Result<Self, RtcError> {
        if width == 0 || height == 0 {
            return Err(err(format!("invalid VP8 dimensions: {width}x{height}")));
        }

        let config = SoftwareVP8EncoderConfig {
            width,
            height,
            frame_rate: 30,
            bit_rate: (bitrate_kbps as u64) * 1000,
            keyframe_interval: 30, // ~1s at 30fps — faster recovery if first keyframe is lost
        };

        let mut inner = SoftwareVP8Encoder::new(config);
        if !inner.initialize() {
            return Err(err("SoftwareVP8Encoder::initialize failed".into()));
        }
        log::info!("[Vp8Encoder] initialised {width}x{height} @ {bitrate_kbps}kbps (libvpx sw)");
        Ok(Self { inner, width, height })
    }

    /// Encode an I420 frame and return the VP8 bitstream.
    ///
    /// Returns `Ok(Some((data, is_key_frame)))` on success, or
    /// `Ok(None)` if the encoder produced no output for this frame.
    pub fn encode(&mut self, i420_data: &[u8], timestamp_us: i64) -> Result<Option<(Vec<u8>, bool)>, RtcError> {
        let expected = (self.width * self.height * 3 / 2) as usize;
        if i420_data.len() < expected {
            return Err(err(format!("I420 buffer too small: {} < {}", i420_data.len(), expected)));
        }

        if !self.inner.encode(i420_data, timestamp_us) {
            return Err(err("SoftwareVP8Encoder::encode returned false".into()));
        }

        match self.inner.poll_output() {
            Some(frame) => Ok(Some((frame.data, frame.is_key_frame))),
            None => Ok(None),
        }
    }
}
