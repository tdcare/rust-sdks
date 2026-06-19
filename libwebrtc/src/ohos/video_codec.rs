//! OHOS H.264 hardware video encoder/decoder bindings.
//!
//! Thin FFI bridge over the OH_AVCodec API. The native codec performs the
//! work; this module marshals YUV/H.264 buffers in and out.
//!
//! FFI symbols are gated by `#[cfg(target_env = "ohos")]`; on other targets a
//! stub set of types is exposed so the rest of the crate keeps compiling.

#![allow(non_camel_case_types)]
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

/// Errors produced by the OHOS hardware video codec wrappers.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// The native codec instance could not be created.
    #[error("failed to create codec: {0}")]
    CreateFailed(String),
    /// The codec has not been initialized.
    #[error("codec not initialized")]
    NotInitialized,
    /// The codec has not been started.
    #[error("codec not running")]
    NotRunning,
    /// A native call returned a non-zero status.
    #[error("native call `{op}` failed with code {code}")]
    Native { op: &'static str, code: i32 },
    /// An interior `Mutex` was poisoned.
    #[error("internal lock poisoned")]
    Poisoned,
}

/// Result alias used throughout this module.
pub type Result<T> = std::result::Result<T, CodecError>;

#[cfg(target_env = "ohos")]
pub(crate) mod ffi {
    use std::os::raw::{c_char, c_int, c_uint, c_void};

    pub const AV_ERR_OK: c_int = 0;
    pub const AV_PIXEL_FORMAT_NV12: c_int = 2;
    pub const AVCODEC_BUFFER_FLAGS_NONE: u32 = 0;
    pub const AVCODEC_BUFFER_FLAGS_EOS: u32 = 1;
    pub const AVCODEC_BUFFER_FLAGS_SYNC_FRAME: u32 = 2;
    pub const AVCODEC_BUFFER_FLAGS_CODEC_DATA: u32 = 8;

    /// Field order must match `native_avcodec_base.h`.
    #[repr(C)]
    pub struct OH_AVCodecBufferAttr {
        pub pts: i64,
        pub size: c_int,
        pub offset: c_int,
        pub flags: u32,
    }

    #[repr(C)]
    pub struct OH_AVCodec {
        _private: [u8; 0],
    }
    #[repr(C)]
    pub struct OH_AVFormat {
        _private: [u8; 0],
    }
    #[repr(C)]
    pub struct OH_AVBuffer {
        _private: [u8; 0],
    }

    pub type OH_OnError = Option<unsafe extern "C" fn(*mut OH_AVCodec, c_int, *mut c_void)>;
    pub type OH_OnStreamChanged =
        Option<unsafe extern "C" fn(*mut OH_AVCodec, *mut OH_AVFormat, *mut c_void)>;
    pub type OH_OnNeedInputBuffer =
        Option<unsafe extern "C" fn(*mut OH_AVCodec, c_uint, *mut OH_AVBuffer, *mut c_void)>;
    pub type OH_OnNewOutputBuffer =
        Option<unsafe extern "C" fn(*mut OH_AVCodec, c_uint, *mut OH_AVBuffer, *mut c_void)>;

    #[repr(C)]
    pub struct OH_AVCodecCallback {
        pub on_error: OH_OnError,
        pub on_stream_changed: OH_OnStreamChanged,
        pub on_need_input_buffer: OH_OnNeedInputBuffer,
        pub on_new_output_buffer: OH_OnNewOutputBuffer,
    }

    #[link(name = "native_media_core")]
    extern "C" {
        pub fn OH_AVFormat_Create() -> *mut OH_AVFormat;
        pub fn OH_AVFormat_Destroy(format: *mut OH_AVFormat);
        pub fn OH_AVFormat_SetIntValue(f: *mut OH_AVFormat, key: *const c_char, v: i32);
        pub fn OH_AVFormat_SetLongValue(f: *mut OH_AVFormat, key: *const c_char, v: i64);
        pub fn OH_AVFormat_SetDoubleValue(f: *mut OH_AVFormat, key: *const c_char, v: f64);
        pub fn OH_AVBuffer_GetAddr(b: *mut OH_AVBuffer) -> *mut u8;
        pub fn OH_AVBuffer_GetCapacity(b: *mut OH_AVBuffer) -> i32;
        pub fn OH_AVBuffer_GetBufferAttr(b: *mut OH_AVBuffer, a: *mut OH_AVCodecBufferAttr) -> c_int;
        pub fn OH_AVBuffer_SetBufferAttr(b: *mut OH_AVBuffer, a: *const OH_AVCodecBufferAttr) -> c_int;
    }

    #[link(name = "native_media_venc")]
    extern "C" {
        pub fn OH_VideoEncoder_CreateByMime(mime: *const c_char) -> *mut OH_AVCodec;
        pub fn OH_VideoEncoder_Destroy(codec: *mut OH_AVCodec) -> c_int;
        pub fn OH_VideoEncoder_Configure(c: *mut OH_AVCodec, f: *mut OH_AVFormat) -> c_int;
        pub fn OH_VideoEncoder_RegisterCallback(
            c: *mut OH_AVCodec,
            cb: OH_AVCodecCallback,
            ud: *mut c_void,
        ) -> c_int;
        pub fn OH_VideoEncoder_Prepare(c: *mut OH_AVCodec) -> c_int;
        pub fn OH_VideoEncoder_Start(c: *mut OH_AVCodec) -> c_int;
        pub fn OH_VideoEncoder_Stop(c: *mut OH_AVCodec) -> c_int;
        pub fn OH_VideoEncoder_Flush(c: *mut OH_AVCodec) -> c_int;
        pub fn OH_VideoEncoder_PushInputBuffer(c: *mut OH_AVCodec, idx: c_uint) -> c_int;
        pub fn OH_VideoEncoder_FreeOutputBuffer(c: *mut OH_AVCodec, idx: c_uint) -> c_int;
        pub fn OH_VideoEncoder_SetParameter(c: *mut OH_AVCodec, f: *mut OH_AVFormat) -> c_int;
    }

    #[link(name = "native_media_vdec")]
    extern "C" {
        pub fn OH_VideoDecoder_CreateByMime(mime: *const c_char) -> *mut OH_AVCodec;
        pub fn OH_VideoDecoder_Destroy(codec: *mut OH_AVCodec) -> c_int;
        pub fn OH_VideoDecoder_Configure(c: *mut OH_AVCodec, f: *mut OH_AVFormat) -> c_int;
        pub fn OH_VideoDecoder_RegisterCallback(
            c: *mut OH_AVCodec,
            cb: OH_AVCodecCallback,
            ud: *mut c_void,
        ) -> c_int;
        pub fn OH_VideoDecoder_Prepare(c: *mut OH_AVCodec) -> c_int;
        pub fn OH_VideoDecoder_Start(c: *mut OH_AVCodec) -> c_int;
        pub fn OH_VideoDecoder_Stop(c: *mut OH_AVCodec) -> c_int;
        pub fn OH_VideoDecoder_Flush(c: *mut OH_AVCodec) -> c_int;
        pub fn OH_VideoDecoder_PushInputBuffer(c: *mut OH_AVCodec, idx: c_uint) -> c_int;
        pub fn OH_VideoDecoder_FreeOutputBuffer(c: *mut OH_AVCodec, idx: c_uint) -> c_int;
    }
}

#[cfg(not(target_env = "ohos"))]
mod ffi {
    pub const AV_PIXEL_FORMAT_NV12: i32 = 2;
    pub const AVCODEC_BUFFER_FLAGS_NONE: u32 = 0;
    pub const AVCODEC_BUFFER_FLAGS_EOS: u32 = 1;
    pub const AVCODEC_BUFFER_FLAGS_SYNC_FRAME: u32 = 2;
    pub const AVCODEC_BUFFER_FLAGS_CODEC_DATA: u32 = 8;
}

pub use ffi::{
    AVCODEC_BUFFER_FLAGS_CODEC_DATA, AVCODEC_BUFFER_FLAGS_EOS, AVCODEC_BUFFER_FLAGS_NONE,
    AVCODEC_BUFFER_FLAGS_SYNC_FRAME, AV_PIXEL_FORMAT_NV12,
};

#[cfg(target_env = "ohos")]
mod imp;

/// Configuration for [`VideoEncoder`].
#[derive(Debug, Clone)]
pub struct VideoEncoderConfig {
    pub width: u32,
    pub height: u32,
    pub frame_rate: u32,
    pub bit_rate: u64,
    /// Pixel format value as defined in `native_avcodec_base.h`.
    pub pixel_format: i32,
    /// I-frame interval in milliseconds.
    pub i_frame_interval_ms: i32,
}

impl Default for VideoEncoderConfig {
    fn default() -> Self {
        Self {
            width: 640,
            height: 480,
            frame_rate: 30,
            bit_rate: 2_000_000,
            pixel_format: ffi::AV_PIXEL_FORMAT_NV12,
            i_frame_interval_ms: 2000,
        }
    }
}

/// Configuration for [`VideoDecoder`].
#[derive(Debug, Clone)]
pub struct VideoDecoderConfig {
    pub width: u32,
    pub height: u32,
    pub pixel_format: i32,
}

impl Default for VideoDecoderConfig {
    fn default() -> Self {
        Self { width: 640, height: 480, pixel_format: ffi::AV_PIXEL_FORMAT_NV12 }
    }
}

/// Encoded H.264 output frame.
#[derive(Clone)]
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub timestamp_us: i64,
    pub is_key_frame: bool,
    pub is_eos: bool,
}

/// Decoded YUV output frame (NV12 by default).
#[derive(Clone)]
pub struct DecodedFrame {
    pub data: Vec<u8>,
    pub timestamp_us: i64,
    pub is_eos: bool,
}

/// Split an Annex-B keyframe payload into a `(SPS+PPS, idr_offset)` pair.
///
/// Returns `None` when no SPS NAL is present in the input.
pub fn extract_sps_pps_from_annexb(data: &[u8]) -> Option<(Vec<u8>, usize)> {
    let len = data.len();
    let mut nal_starts: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i + 3 < len {
        if data[i] == 0 && data[i + 1] == 0 {
            if data[i + 2] == 0 && i + 3 < len && data[i + 3] == 1 {
                nal_starts.push((i, i + 4));
                i += 4;
                continue;
            }
            if data[i + 2] == 1 {
                nal_starts.push((i, i + 3));
                i += 3;
                continue;
            }
        }
        i += 1;
    }

    let mut codec_data = Vec::new();
    let mut idr_offset: usize = len;
    let mut found_sps = false;

    for (idx, &(sc_pos, nal_pos)) in nal_starts.iter().enumerate() {
        if nal_pos >= len {
            continue;
        }
        let nal_type = data[nal_pos] & 0x1F;
        let nal_end = if idx + 1 < nal_starts.len() { nal_starts[idx + 1].0 } else { len };
        if nal_type == 7 || nal_type == 8 {
            codec_data.extend_from_slice(&data[sc_pos..nal_end]);
            if nal_type == 7 {
                found_sps = true;
            }
        } else if idr_offset == len {
            idr_offset = sc_pos;
        }
    }

    if found_sps && !codec_data.is_empty() {
        Some((codec_data, idr_offset))
    } else {
        None
    }
}

// ---------- Public, thread-safe handles ----------

/// Thread-safe handle around a hardware H.264 video encoder.
#[cfg(target_env = "ohos")]
#[derive(Clone)]
pub struct VideoEncoder {
    inner: Arc<Mutex<Option<imp::VideoEncoder>>>,
}

#[cfg(target_env = "ohos")]
impl VideoEncoder {
    pub fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(None)) }
    }
    pub fn create_h264(&self, config: VideoEncoderConfig) -> Result<()> {
        let enc = imp::VideoEncoder::new_h264(config)?;
        enc.initialize()?;
        let mut g = self.inner.lock().map_err(|_| CodecError::Poisoned)?;
        *g = Some(enc);
        Ok(())
    }
    pub fn start(&self) -> Result<()> {
        let g = self.inner.lock().map_err(|_| CodecError::Poisoned)?;
        g.as_ref().ok_or(CodecError::NotInitialized)?.start()
    }
    pub fn stop(&self) -> Result<()> {
        let g = self.inner.lock().map_err(|_| CodecError::Poisoned)?;
        match g.as_ref() {
            Some(e) => e.stop(),
            None => Ok(()),
        }
    }
    pub fn encode_frame(&self, yuv: &[u8], timestamp_us: i64) -> Result<()> {
        let g = self.inner.lock().map_err(|_| CodecError::Poisoned)?;
        g.as_ref().ok_or(CodecError::NotInitialized)?.encode_frame(yuv, timestamp_us)
    }
    pub fn poll_output(&self) -> Option<EncodedFrame> {
        let g = self.inner.lock().ok()?;
        g.as_ref()?.poll_output()
    }
    pub fn request_key_frame(&self) -> Result<()> {
        let g = self.inner.lock().map_err(|_| CodecError::Poisoned)?;
        g.as_ref().ok_or(CodecError::NotInitialized)?.request_key_frame()
    }
    pub fn destroy(&self) -> Result<()> {
        let mut g = self.inner.lock().map_err(|_| CodecError::Poisoned)?;
        *g = None;
        Ok(())
    }
}

#[cfg(target_env = "ohos")]
impl Default for VideoEncoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe handle around a hardware H.264 video decoder.
#[cfg(target_env = "ohos")]
#[derive(Clone)]
pub struct VideoDecoder {
    inner: Arc<Mutex<Option<imp::VideoDecoder>>>,
}

#[cfg(target_env = "ohos")]
impl VideoDecoder {
    pub fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(None)) }
    }
    pub fn create_h264(&self, config: VideoDecoderConfig) -> Result<()> {
        let dec = imp::VideoDecoder::new_h264(config)?;
        dec.initialize()?;
        let mut g = self.inner.lock().map_err(|_| CodecError::Poisoned)?;
        *g = Some(dec);
        Ok(())
    }

    /// Create a hardware VP8 decoder.
    pub fn create_vp8(&self, config: VideoDecoderConfig) -> Result<()> {
        let dec = imp::VideoDecoder::new_vp8(config)?;
        dec.initialize()?;
        let mut g = self.inner.lock().map_err(|_| CodecError::Poisoned)?;
        *g = Some(dec);
        Ok(())
    }
    pub fn start(&self) -> Result<()> {
        let g = self.inner.lock().map_err(|_| CodecError::Poisoned)?;
        g.as_ref().ok_or(CodecError::NotInitialized)?.start()
    }
    pub fn stop(&self) -> Result<()> {
        let g = self.inner.lock().map_err(|_| CodecError::Poisoned)?;
        match g.as_ref() {
            Some(d) => d.stop(),
            None => Ok(()),
        }
    }
    pub fn decode_frame(&self, nalu: &[u8], timestamp_us: i64, is_key_frame: bool) -> Result<()> {
        let g = self.inner.lock().map_err(|_| CodecError::Poisoned)?;
        g.as_ref()
            .ok_or(CodecError::NotInitialized)?
            .decode_frame(nalu, timestamp_us, is_key_frame)
    }
    pub fn poll_output(&self) -> Option<DecodedFrame> {
        let g = self.inner.lock().ok()?;
        g.as_ref()?.poll_output()
    }
    pub fn destroy(&self) -> Result<()> {
        let mut g = self.inner.lock().map_err(|_| CodecError::Poisoned)?;
        *g = None;
        Ok(())
    }
}

#[cfg(target_env = "ohos")]
impl Default for VideoDecoder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------- Stubs for non-OHOS targets ----------

#[cfg(not(target_env = "ohos"))]
#[derive(Default, Clone)]
pub struct VideoEncoder;

#[cfg(not(target_env = "ohos"))]
impl VideoEncoder {
    pub fn new() -> Self {
        Self
    }
    pub fn create_h264(&self, _c: VideoEncoderConfig) -> Result<()> {
        Err(CodecError::CreateFailed("OH_AVCodec is only available on OHOS".into()))
    }
    pub fn start(&self) -> Result<()> {
        Err(CodecError::NotInitialized)
    }
    pub fn stop(&self) -> Result<()> {
        Ok(())
    }
    pub fn encode_frame(&self, _y: &[u8], _t: i64) -> Result<()> {
        Err(CodecError::NotInitialized)
    }
    pub fn poll_output(&self) -> Option<EncodedFrame> {
        None
    }
    pub fn request_key_frame(&self) -> Result<()> {
        Err(CodecError::NotInitialized)
    }
    pub fn destroy(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(not(target_env = "ohos"))]
#[derive(Default, Clone)]
pub struct VideoDecoder;

#[cfg(not(target_env = "ohos"))]
impl VideoDecoder {
    pub fn new() -> Self {
        Self
    }
    pub fn create_h264(&self, _c: VideoDecoderConfig) -> Result<()> {
        Err(CodecError::CreateFailed("OH_AVCodec is only available on OHOS".into()))
    }
    pub fn start(&self) -> Result<()> {
        Err(CodecError::NotInitialized)
    }
    pub fn stop(&self) -> Result<()> {
        Ok(())
    }
    pub fn decode_frame(&self, _n: &[u8], _t: i64, _k: bool) -> Result<()> {
        Err(CodecError::NotInitialized)
    }
    pub fn poll_output(&self) -> Option<DecodedFrame> {
        None
    }
    pub fn destroy(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_sps_pps_returns_none_without_sps() {
        let data = [0, 0, 0, 1, 0x21, 0x00];
        assert!(extract_sps_pps_from_annexb(&data).is_none());
    }

    #[test]
    fn extract_sps_pps_separates_csd_from_idr() {
        let data = [
            0, 0, 0, 1, 0x67, 0xAA, // SPS
            0, 0, 0, 1, 0x68, 0xBB, // PPS
            0, 0, 0, 1, 0x65, 0xCC, // IDR
        ];
        let (csd, idr) = extract_sps_pps_from_annexb(&data).expect("found SPS");
        assert_eq!(idr, 12);
        assert!(csd.starts_with(&[0, 0, 0, 1, 0x67]));
        assert!(csd.windows(5).any(|w| w == [0, 0, 0, 1, 0x68]));
    }
}
