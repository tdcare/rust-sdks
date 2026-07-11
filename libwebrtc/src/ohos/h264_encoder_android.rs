//! H.264 hardware encoder using Android AMediaCodec NDK API.
//!
//! Uses the Android NDK `AMediaCodec` C API (from `libmediandk.so`) to
//! leverage device hardware H.264 encoding. Falls back to VP8 software
//! encoding at the caller level if codec creation fails.
//!
//! The encoder operates in synchronous polling mode:
//! - `encode()` dequeues an input buffer, fills it with NV12 data,
//!   queues it, then dequeues any available output.
//! - No callbacks are used — the poll loop is driven by each `encode()` call.

use std::ffi::{c_char, c_int, c_void};
use std::ptr::{self, null_mut};
use std::sync::OnceLock;

use crate::{RtcError, RtcErrorType};

fn err(msg: String) -> RtcError {
    RtcError { error_type: RtcErrorType::Internal, message: msg }
}

// ---------------------------------------------------------------------------
// AMediaCodec FFI declarations (libmediandk.so)
// ---------------------------------------------------------------------------

#[repr(C)]
struct AMediaCodec {
    _opaque: [u8; 0],
}
#[repr(C)]
struct AMediaFormat {
    _opaque: [u8; 0],
}

type media_status_t = i32;

const AMEDIA_OK: media_status_t = 0;

/// Returned from `AMediaCodec_dequeueInputBuffer` when no buffer is available.
const AMEDIACODEC_INFO_TRY_AGAIN_LATER: isize = -1;
/// Returned from `AMediaCodec_dequeueOutputBuffer` when the output format changed.
const AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED: isize = -2;

// Flags for AMediaCodec_configure
const AMEDIACODEC_CONFIGURE_FLAG_ENCODE: u32 = 1;

// Flags in AMediaCodecBufferInfo
const AMEDIACODEC_BUFFER_FLAG_KEY_FRAME: u32 = 1;
const AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG: u32 = 2;
const AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM: u32 = 4;

/// Color format: flexible YUV420 (accepts NV12/I420/SemiPlanar)
const COLOR_FORMAT_YUV420_FLEXIBLE: i32 = 0x7F420888;

#[repr(C)]
pub struct AMediaCodecBufferInfo {
    pub offset: i32,
    pub size: i32,
    pub presentation_time_us: u64,
    pub flags: u32,
}

#[link(name = "mediandk")]
extern "C" {
    // ---- Codec lifecycle ----
    fn AMediaCodec_createEncoderByType(mime: *const c_char) -> *mut AMediaCodec;
    fn AMediaCodec_delete(codec: *mut AMediaCodec) -> media_status_t;
    fn AMediaCodec_configure(
        codec: *mut AMediaCodec,
        format: *const AMediaFormat,
        surface: *mut c_void,      // ANativeWindow* → null for encoding
        crypto: *mut c_void,       // AMediaCrypto* → null
        flags: u32,
    ) -> media_status_t;
    fn AMediaCodec_start(codec: *mut AMediaCodec) -> media_status_t;
    fn AMediaCodec_stop(codec: *mut AMediaCodec) -> media_status_t;
    fn AMediaCodec_flush(codec: *mut AMediaCodec) -> media_status_t;

    // ---- Input ----
    fn AMediaCodec_dequeueInputBuffer(
        codec: *mut AMediaCodec,
        timeout_us: i64,
    ) -> isize;

    fn AMediaCodec_getInputBuffer(
        codec: *mut AMediaCodec,
        idx: usize,
        out_size: *mut usize,
    ) -> *mut u8;

    fn AMediaCodec_queueInputBuffer(
        codec: *mut AMediaCodec,
        idx: usize,
        offset: usize,
        size: usize,
        presentation_time_us: u64,
        flags: u32,
    ) -> media_status_t;

    // ---- Output ----
    fn AMediaCodec_dequeueOutputBuffer(
        codec: *mut AMediaCodec,
        info: *mut AMediaCodecBufferInfo,
        timeout_us: i64,
    ) -> isize;

    fn AMediaCodec_getOutputBuffer(
        codec: *mut AMediaCodec,
        idx: usize,
        out_size: *mut usize,
    ) -> *mut u8;

    fn AMediaCodec_releaseOutputBuffer(
        codec: *mut AMediaCodec,
        idx: usize,
        render: bool,
    ) -> media_status_t;

    fn AMediaCodec_getOutputFormat(codec: *mut AMediaCodec) -> *mut AMediaFormat;

    // ---- Format helpers ----
    fn AMediaFormat_new() -> *mut AMediaFormat;
    fn AMediaFormat_delete(format: *mut AMediaFormat);
    fn AMediaFormat_setString(
        format: *mut AMediaFormat,
        key: *const c_char,
        value: *const c_char,
    );
    fn AMediaFormat_setInt32(
        format: *mut AMediaFormat,
        key: *const c_char,
        value: i32,
    );
    fn AMediaFormat_setInt64(
        format: *mut AMediaFormat,
        key: *const c_char,
        value: i64,
    );
    fn AMediaFormat_getBuffer(
        format: *mut AMediaFormat,
        key: *const c_char,
        out_data: *mut *mut c_void,
        out_size: *mut usize,
    ) -> bool;
}

// ---------------------------------------------------------------------------
// H264Encoder: public API (matches OHOS version)
// ---------------------------------------------------------------------------

/// H.264 hardware encoder backed by Android AMediaCodec.
pub struct H264Encoder {
    codec: *mut AMediaCodec,
    width: u32,
    height: u32,
    /// Scratch buffer for I420 → NV12 conversion
    nv12_scratch: Vec<u8>,
    /// Cached SPS/PPS codec data in Annex-B format, prepended to keyframes
    codec_data: Vec<u8>,
    /// Whether we've collected the initial SPS/PPS from the codec
    codec_data_collected: bool,
    /// For logging
    frame_count: u64,
    /// Timeout for dequeue operations (microseconds)
    timeout_us: i64,
    /// Stored pending encoded output. At most one frame is buffered.
    pending_output: Option<(Vec<u8>, bool)>,
}

unsafe impl Send for H264Encoder {}

impl H264Encoder {
    /// Check if an H.264 encoder is available on this device.
    ///
    /// Tries to create a short-lived encoder instance as a probe.
    /// Result is cached after the first call.
    pub fn is_available() -> bool {
        static CACHED: OnceLock<bool> = OnceLock::new();
        *CACHED.get_or_init(|| {
            let mime = std::ffi::CString::new("video/avc").unwrap();
            let codec = unsafe { AMediaCodec_createEncoderByType(mime.as_ptr()) };
            let available = !codec.is_null();
            if available {
                unsafe { AMediaCodec_delete(codec) };
            }
            log::info!("[H264Encoder-Android] is_available probe: {available}");
            available
        })
    }

    /// Create a new H.264 hardware encoder.
    ///
    /// * `width` / `height` - frame dimensions in pixels.
    /// * `bitrate_kbps` - target bitrate in kbps.
    pub fn new(width: u32, height: u32, bitrate_kbps: u32) -> Result<Self, RtcError> {
        if width == 0 || height == 0 {
            return Err(err(format!("invalid dimensions: {width}x{height}")));
        }
        if width % 2 != 0 || height % 2 != 0 {
            return Err(err(format!(
                "dimensions must be even for YUV420: {width}x{height}"
            )));
        }

        // 1. Create the encoder
        let mime = std::ffi::CString::new("video/avc")
            .map_err(|_| err("invalid MIME string".into()))?;
        let codec = unsafe { AMediaCodec_createEncoderByType(mime.as_ptr()) };
        if codec.is_null() {
            return Err(err("AMediaCodec_createEncoderByType returned null".into()));
        }

        // 2. Create and configure MediaFormat
        let format = unsafe { AMediaFormat_new() };
        if format.is_null() {
            unsafe { AMediaCodec_delete(codec) };
            return Err(err("AMediaFormat_new failed".into()));
        }

        let key_mime = std::ffi::CString::new("mime").unwrap();
        let key_width = std::ffi::CString::new("width").unwrap();
        let key_height = std::ffi::CString::new("height").unwrap();
        let key_bitrate = std::ffi::CString::new("bitrate").unwrap();
        let key_framerate = std::ffi::CString::new("frame-rate").unwrap();
        let key_iframe = std::ffi::CString::new("i-frame-interval").unwrap();
        let key_color = std::ffi::CString::new("color-format").unwrap();
        let val_mime = std::ffi::CString::new("video/avc").unwrap();

        unsafe {
            AMediaFormat_setString(format, key_mime.as_ptr(), val_mime.as_ptr());
            AMediaFormat_setInt32(format, key_width.as_ptr(), width as i32);
            AMediaFormat_setInt32(format, key_height.as_ptr(), height as i32);
            AMediaFormat_setInt32(format, key_bitrate.as_ptr(), (bitrate_kbps * 1000) as i32);
            AMediaFormat_setInt32(format, key_framerate.as_ptr(), 30);
            AMediaFormat_setInt32(format, key_iframe.as_ptr(), 5); // I-frame every 5 seconds
            AMediaFormat_setInt32(format, key_color.as_ptr(), COLOR_FORMAT_YUV420_FLEXIBLE);
        }

        // 3. Configure the codec
        let status = unsafe {
            AMediaCodec_configure(
                codec,
                format,
                null_mut(), // no surface
                null_mut(), // no crypto
                AMEDIACODEC_CONFIGURE_FLAG_ENCODE,
            )
        };
        unsafe { AMediaFormat_delete(format) };
        if status != AMEDIA_OK {
            unsafe { AMediaCodec_delete(codec) };
            return Err(err(format!("AMediaCodec_configure failed: {status}")));
        }

        // 4. Start the codec
        let status = unsafe { AMediaCodec_start(codec) };
        if status != AMEDIA_OK {
            unsafe { AMediaCodec_delete(codec) };
            return Err(err(format!("AMediaCodec_start failed: {status}")));
        }

        // 5. Try to collect SPS/PPS via getOutputFormat immediately after start.
        // On some devices the format is available right away; on others we need
        // to wait for the first OUTPUT_FORMAT_CHANGED event (handled in encode()).
        let mut codec_data = Vec::new();
        let output_format = unsafe { AMediaCodec_getOutputFormat(codec) };
        if !output_format.is_null() {
            codec_data =
                Self::extract_codec_data(output_format).unwrap_or_default();
            unsafe { AMediaFormat_delete(output_format) };
        }

        let cd_collected = !codec_data.is_empty();
        log::info!(
            "[H264Encoder-Android] initialised {width}x{height} @ {bitrate_kbps}kbps \
             (AMediaCodec), codec_data={}B",
            codec_data.len()
        );

        Ok(Self {
            codec,
            width,
            height,
            nv12_scratch: Vec::new(),
            codec_data,
            codec_data_collected: cd_collected,
            frame_count: 0,
            timeout_us: 10_000, // 10ms poll timeout
            pending_output: None,
        })
    }

    /// Push an I420 frame and return any available encoded H.264 packet.
    ///
    /// Returns `Option<(encoded_data, is_key_frame)>`.  When `None` is
    /// returned, no output frame is ready yet (call again after feeding more
    /// input).
    pub fn encode(
        &mut self,
        i420_data: &[u8],
        timestamp_us: i64,
    ) -> Result<Option<(Vec<u8>, bool)>, RtcError> {
        let expected = (self.width * self.height * 3 / 2) as usize;
        if i420_data.len() < expected {
            return Err(err(format!(
                "I420 buffer too small: {} < {expected}",
                i420_data.len()
            )));
        }

        // ---- Step 1: Convert I420 → NV12 ----
        Self::i420_to_nv12_into(i420_data, self.width, self.height, &mut self.nv12_scratch);

        // ---- Step 2: Feed NV12 into the codec (copy to avoid borrow conflict) ----
        let nv12_copy = self.nv12_scratch.clone();
        self.feed_input(&nv12_copy, timestamp_us);

        // ---- Step 3: Drain any available output ----
        self.drain_output();

        // ---- Step 4: Return the next encoded packet (if any) ----
        Ok(self.poll_output())
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Feed one NV12 frame to the codec's input queue.
    fn feed_input(&mut self, nv12: &[u8], timestamp_us: i64) {
        let idx = unsafe {
            AMediaCodec_dequeueInputBuffer(self.codec, self.timeout_us)
        };
        if idx < 0 {
            log::warn!(
                "[H264Encoder-Android] dequeueInputBuffer returned {idx} \
                 (TRY_AGAIN), dropping frame"
            );
            return;
        }

        let mut buf_size: usize = 0;
        let buf = unsafe {
            AMediaCodec_getInputBuffer(self.codec, idx as usize, &mut buf_size)
        };
        if buf.is_null() || nv12.len() > buf_size {
            log::error!(
                "[H264Encoder-Android] input buffer null or too small \
                 (need {} vs {buf_size})",
                nv12.len()
            );
            return;
        }

        unsafe {
            ptr::copy_nonoverlapping(nv12.as_ptr(), buf, nv12.len());
            AMediaCodec_queueInputBuffer(
                self.codec,
                idx as usize,
                0,
                nv12.len(),
                timestamp_us as u64,
                0, // no flags
            );
        }

        self.frame_count += 1;
        if self.frame_count % 30 == 1 {
            log::debug!(
                "[H264Encoder-Android] feed #{frame}: {w}x{h} {n}B @ {ts}us",
                frame = self.frame_count,
                w = self.width,
                h = self.height,
                n = nv12.len(),
                ts = timestamp_us,
            );
        }
    }

    /// Drain available output frames from the codec.
    fn drain_output(&mut self) {
        loop {
            let mut info = AMediaCodecBufferInfo {
                offset: 0,
                size: 0,
                presentation_time_us: 0,
                flags: 0,
            };
            let idx = unsafe {
                AMediaCodec_dequeueOutputBuffer(
                    self.codec,
                    &mut info,
                    self.timeout_us,
                )
            };

            if idx == AMEDIACODEC_INFO_TRY_AGAIN_LATER {
                // No output available right now — that's fine.
                break;
            }

            if idx == AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED {
                // Codec format changed — collect SPS/PPS.
                let fmt = unsafe { AMediaCodec_getOutputFormat(self.codec) };
                if !fmt.is_null() {
                    if let Some(cd) = Self::extract_codec_data(fmt) {
                        log::info!(
                            "[H264Encoder-Android] OUTPUT_FORMAT_CHANGED: \
                             codec_data={}B",
                            cd.len()
                        );
                        self.codec_data = cd;
                        self.codec_data_collected = true;
                    }
                    unsafe { AMediaFormat_delete(fmt) };
                }
                continue;
            }

            if idx < 0 {
                log::warn!("[H264Encoder-Android] unexpected dequeueOutputBuffer result: {idx}");
                break;
            }

            // Read the encoded data.
            let mut buf_size: usize = 0;
            let buf = unsafe {
                AMediaCodec_getOutputBuffer(
                    self.codec,
                    idx as usize,
                    &mut buf_size,
                )
            };

            let is_codec_config =
                (info.flags & AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG) != 0;
            let is_key_frame =
                (info.flags & AMEDIACODEC_BUFFER_FLAG_KEY_FRAME) != 0;

            if !buf.is_null() && info.size > 0 && !is_codec_config {
                let data_slice =
                    unsafe {
                        std::slice::from_raw_parts(
                            buf.add(info.offset as usize),
                            info.size as usize,
                        )
                    };

                // Convert AVCC → Annex-B if needed, prepend SPS/PPS to keyframes.
                let nal_data = if is_annexb_format(data_slice) {
                    data_slice.to_vec()
                } else {
                    avcc_to_annexb(data_slice)
                };

                let final_data = if is_key_frame && !self.codec_data.is_empty() {
                    let mut combined =
                        Vec::with_capacity(self.codec_data.len() + nal_data.len());
                    combined.extend_from_slice(&self.codec_data);
                    combined.extend_from_slice(&nal_data);
                    combined
                } else {
                    nal_data
                };

                // Instead of pushing to a queue, we store a single pending output.
                // The caller polls immediately after encode(), so we only need
                // one slot. If multiple frames arrive between calls we keep the
                // latest.
                self.pending_output = Some((final_data, is_key_frame));
            } else if is_codec_config {
                // Some devices send codec config as output buffer instead of
                // through the format. Cache it if we haven't already.
                if !self.codec_data_collected && !buf.is_null() && info.size > 0 {
                    let raw = unsafe {
                        std::slice::from_raw_parts(
                            buf.add(info.offset as usize),
                            info.size as usize,
                        )
                    };
                    let cd = if is_annexb_format(raw) {
                        raw.to_vec()
                    } else if let Some(parsed) = avcc_extradata_to_annexb(raw) {
                        parsed
                    } else {
                        raw.to_vec()
                    };
                    log::info!(
                        "[H264Encoder-Android] codec_config buffer: {}B",
                        cd.len()
                    );
                    self.codec_data = cd;
                    self.codec_data_collected = true;
                }
            }

            unsafe {
                AMediaCodec_releaseOutputBuffer(self.codec, idx as usize, false);
            }
        }
    }

    /// Return the next encoded packet, if available.
    fn poll_output(&mut self) -> Option<(Vec<u8>, bool)> {
        self.pending_output.take()
    }

    /// Extract SPS/PPS codec data from the output format.
    ///
    /// On Android, csd-0 (SPS) and csd-1 (PPS) are available as byte buffers
    /// in the MediaFormat after the codec starts.
    fn extract_codec_data(format: *mut AMediaFormat) -> Option<Vec<u8>> {
        let key_csd0 = std::ffi::CString::new("csd-0").unwrap();
        let key_csd1 = std::ffi::CString::new("csd-1").unwrap();

        let mut sps_ptr: *mut c_void = null_mut();
        let mut sps_len: usize = 0;
        let mut pps_ptr: *mut c_void = null_mut();
        let mut pps_len: usize = 0;

        let has_sps = unsafe {
            AMediaFormat_getBuffer(format, key_csd0.as_ptr(), &mut sps_ptr, &mut sps_len)
        };
        let has_pps = unsafe {
            AMediaFormat_getBuffer(format, key_csd1.as_ptr(), &mut pps_ptr, &mut pps_len)
        };

        if !has_sps || sps_ptr.is_null() || sps_len == 0 {
            return None;
        }

        let sps_slice = unsafe { std::slice::from_raw_parts(sps_ptr as *const u8, sps_len) };
        let pps_slice = if has_pps && !pps_ptr.is_null() && pps_len > 0 {
            unsafe { std::slice::from_raw_parts(pps_ptr as *const u8, pps_len) }
        } else {
            &[]
        };

        // Build Annex-B SPS+PPS
        let mut result = Vec::with_capacity(sps_len + pps_len + 8);
        result.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        result.extend_from_slice(sps_slice);
        if !pps_slice.is_empty() {
            result.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            result.extend_from_slice(pps_slice);
        } else {
            log::warn!(
                "[H264Encoder-Android] SPS without PPS (csd-1 missing)"
            );
        }

        log::info!(
            "[H264Encoder-Android] extract_codec_data: SPS={sps_len}B PPS={pps_len}B"
        );
        Some(result)
    }

    // -----------------------------------------------------------------------
    // I420 → NV12 conversion (same as OHOS version)
    // -----------------------------------------------------------------------

    fn i420_to_nv12_into(
        i420: &[u8],
        width: u32,
        height: u32,
        dst: &mut Vec<u8>,
    ) {
        let w = width as usize;
        let h = height as usize;
        let cw = (w + 1) / 2;
        let ch = (h + 1) / 2;
        let stride = ((w + 63) / 64) * 64;
        let y_padded = stride * h;
        let uv_stride = stride;
        let uv_rows = ch;
        let uv_padded = uv_stride * uv_rows;
        let total = y_padded + uv_padded;

        if dst.len() != total {
            dst.resize(total, 0u8);
        }

        let y = &i420[..w * h];
        let u = &i420[w * h..w * h + cw * ch];
        let v = &i420[w * h + cw * ch..w * h + 2 * cw * ch];

        for row in 0..h {
            let src = &y[row * w..(row + 1) * w];
            let dst_off = row * stride;
            dst[dst_off..dst_off + w].copy_from_slice(src);
        }

        let uv_base = y_padded;
        for row in 0..ch {
            let u_src_off = row * cw;
            let v_src_off = row * cw;
            let dst_row_off = uv_base + row * uv_stride;
            for col in 0..cw {
                dst[dst_row_off + col * 2] = u[u_src_off + col];
                dst[dst_row_off + col * 2 + 1] = v[v_src_off + col];
            }
        }
    }
}

impl Drop for H264Encoder {
    fn drop(&mut self) {
        log::info!(
            "[H264Encoder-Android] drop: frames={}, codec={:?}",
            self.frame_count,
            self.codec,
        );
        if !self.codec.is_null() {
            unsafe {
                AMediaCodec_stop(self.codec);
                AMediaCodec_delete(self.codec);
            }
            self.codec = null_mut();
        }
        log::info!("[H264Encoder-Android] destroyed");
    }
}

// ---------------------------------------------------------------------------
// AVCC ↔ Annex-B conversion (same as OHOS version)
// ---------------------------------------------------------------------------

fn is_annexb_format(data: &[u8]) -> bool {
    if data.len() >= 4 && data[0] == 0 && data[1] == 0 && data[2] == 0 && data[3] == 1 {
        return true;
    }
    if data.len() >= 3 && data[0] == 0 && data[1] == 0 && data[2] == 1 {
        return true;
    }
    false
}

fn avcc_to_annexb(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len() + 32);
    let mut offset = 0;
    while offset + 4 <= data.len() {
        let nalu_len = u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        offset += 4;
        if nalu_len == 0 || offset + nalu_len > data.len() {
            break;
        }
        result.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        result.extend_from_slice(&data[offset..offset + nalu_len]);
        offset += nalu_len;
    }
    result
}

fn avcc_extradata_to_annexb(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 7 || data[0] != 1 {
        return None;
    }
    let mut result = Vec::with_capacity(data.len() + 16);
    let num_sps = (data[5] & 0x1F) as usize;
    let mut offset = 6;
    for _ in 0..num_sps {
        if offset + 2 > data.len() {
            return None;
        }
        let sps_len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        if offset + sps_len > data.len() {
            return None;
        }
        result.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        result.extend_from_slice(&data[offset..offset + sps_len]);
        offset += sps_len;
    }
    if offset >= data.len() {
        return None;
    }
    let num_pps = data[offset] as usize;
    offset += 1;
    for _ in 0..num_pps {
        if offset + 2 > data.len() {
            return None;
        }
        let pps_len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        if offset + pps_len > data.len() {
            return None;
        }
        result.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        result.extend_from_slice(&data[offset..offset + pps_len]);
        offset += pps_len;
    }
    Some(result)
}
