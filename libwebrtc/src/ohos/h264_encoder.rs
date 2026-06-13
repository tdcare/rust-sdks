//! H.264 hardware encoder using OH_AVCodec.
//!
//! Uses the OpenHarmony `OH_VideoEncoder` API with MIME `"video/avc"` to
//! leverage device hardware H.264 encoding. Falls back to software encoder
//! (`c2.android.avc.encoder`) if hardware is unavailable.
//!
//! The encoder operates in async callback mode:
//! - `encode()` pushes NV12 frames into a pending queue
//! - `on_need_input_buffer` callback copies frame data into encoder buffers
//! - `on_new_output_buffer` callback collects encoded H.264 NALUs
//! - `poll_output()` retrieves the next encoded packet

use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_uint, c_void};
use std::ptr::{self, null_mut};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;

use crate::{RtcError, RtcErrorType};

fn err(msg: String) -> RtcError {
    RtcError { error_type: RtcErrorType::Internal, message: msg }
}

// ---------------------------------------------------------------------------
// OH_AVCodec FFI declarations
// ---------------------------------------------------------------------------

const AV_ERR_OK: c_int = 0;
const AV_PIXEL_FORMAT_NV12: c_int = 2;
const AVCODEC_BUFFER_FLAGS_NONE: u32 = 0;
const AVCODEC_BUFFER_FLAGS_SYNC_FRAME: u32 = 2;
const AVCODEC_BUFFER_FLAGS_CODEC_DATA: u32 = 8;

#[repr(C)]
struct OH_AVCodecBufferAttr {
    pts: i64,
    size: c_int,
    offset: c_int,
    flags: u32,
}

#[repr(C)]
struct OH_AVCodec { _opaque: [u8; 0] }
#[repr(C)]
struct OH_AVFormat { _opaque: [u8; 0] }
#[repr(C)]
struct OH_AVBuffer { _opaque: [u8; 0] }

#[allow(non_camel_case_types)]
type OH_AVCodecOnError =
    Option<unsafe extern "C" fn(*mut OH_AVCodec, c_int, *mut c_void)>;
#[allow(non_camel_case_types)]
type OH_AVCodecOnStreamChanged =
    Option<unsafe extern "C" fn(*mut OH_AVCodec, *mut OH_AVFormat, *mut c_void)>;
#[allow(non_camel_case_types)]
type OH_AVCodecOnNeedInputBuffer =
    Option<unsafe extern "C" fn(*mut OH_AVCodec, c_uint, *mut OH_AVBuffer, *mut c_void)>;
#[allow(non_camel_case_types)]
type OH_AVCodecOnNewOutputBuffer =
    Option<unsafe extern "C" fn(*mut OH_AVCodec, c_uint, *mut OH_AVBuffer, *mut c_void)>;

#[repr(C)]
struct OH_AVCodecCallback {
    on_error: OH_AVCodecOnError,
    on_stream_changed: OH_AVCodecOnStreamChanged,
    on_need_input_buffer: OH_AVCodecOnNeedInputBuffer,
    on_new_output_buffer: OH_AVCodecOnNewOutputBuffer,
}

#[link(name = "native_media_core")]
extern "C" {
    fn OH_AVFormat_Create() -> *mut OH_AVFormat;
    fn OH_AVFormat_Destroy(format: *mut OH_AVFormat);
    fn OH_AVFormat_SetIntValue(format: *mut OH_AVFormat, key: *const c_char, value: i32);
    fn OH_AVFormat_SetLongValue(format: *mut OH_AVFormat, key: *const c_char, value: i64);
    fn OH_AVFormat_SetDoubleValue(format: *mut OH_AVFormat, key: *const c_char, value: f64);
    fn OH_AVBuffer_GetAddr(buffer: *mut OH_AVBuffer) -> *mut u8;
    fn OH_AVBuffer_GetCapacity(buffer: *mut OH_AVBuffer) -> i32;
    fn OH_AVBuffer_GetBufferAttr(buffer: *mut OH_AVBuffer, attr: *mut OH_AVCodecBufferAttr) -> c_int;
    fn OH_AVBuffer_SetBufferAttr(buffer: *mut OH_AVBuffer, attr: *const OH_AVCodecBufferAttr) -> c_int;
}

#[link(name = "native_media_venc")]
extern "C" {
    fn OH_VideoEncoder_CreateByMime(mime: *const c_char) -> *mut OH_AVCodec;
    fn OH_VideoEncoder_CreateByName(name: *const c_char) -> *mut OH_AVCodec;
    fn OH_VideoEncoder_Destroy(codec: *mut OH_AVCodec) -> c_int;
    fn OH_VideoEncoder_Configure(codec: *mut OH_AVCodec, format: *mut OH_AVFormat) -> c_int;
    fn OH_VideoEncoder_RegisterCallback(
        codec: *mut OH_AVCodec, callback: OH_AVCodecCallback, user_data: *mut c_void,
    ) -> c_int;
    fn OH_VideoEncoder_Prepare(codec: *mut OH_AVCodec) -> c_int;
    fn OH_VideoEncoder_Start(codec: *mut OH_AVCodec) -> c_int;
    fn OH_VideoEncoder_Stop(codec: *mut OH_AVCodec) -> c_int;
    fn OH_VideoEncoder_PushInputBuffer(codec: *mut OH_AVCodec, index: c_uint) -> c_int;
    fn OH_VideoEncoder_FreeOutputBuffer(codec: *mut OH_AVCodec, index: c_uint) -> c_int;
}

// ---------------------------------------------------------------------------
// Encoder internals
// ---------------------------------------------------------------------------

struct InputBufferInfo {
    index: u32,
    buffer_ptr: *mut OH_AVBuffer,
    capacity: i32,
}
unsafe impl Send for InputBufferInfo {}

struct PendingFrame { data: Vec<u8>, timestamp_us: i64 }

struct EncodedPacket { data: Vec<u8>, timestamp_us: i64, is_key_frame: bool }

struct EncoderUserData {
    input_queue: Mutex<VecDeque<InputBufferInfo>>,
    pending_frames: Mutex<VecDeque<PendingFrame>>,
    output_queue: Mutex<VecDeque<EncodedPacket>>,
    input_cb_count: AtomicU32,
    encode_count: AtomicU32,
    output_count: AtomicU32,
    /// Cached SPS/PPS codec data in Annex-B format, prepended to keyframes
    codec_data: Mutex<Vec<u8>>,
}

impl EncoderUserData {
    fn new() -> Self {
        Self {
            input_queue: Mutex::new(VecDeque::new()),
            pending_frames: Mutex::new(VecDeque::new()),
            output_queue: Mutex::new(VecDeque::new()),
            input_cb_count: AtomicU32::new(0),
            encode_count: AtomicU32::new(0),
            output_count: AtomicU32::new(0),
            codec_data: Mutex::new(Vec::new()),
        }
    }
}

// ---------------------------------------------------------------------------
// I420 → NV12 conversion
// ---------------------------------------------------------------------------

/// Convert I420 (planar YUV 4:2:0) to NV12 (semi-planar) with stride alignment.
///
/// Hardware video encoders on OHOS allocate input buffers with row-stride
/// aligned to e.g. 64 bytes.  Writing compact NV12 (stride == width) into a
/// larger stride-aligned buffer causes the encoder to read data at wrong
/// offsets, producing visible stripe artefacts.  This helper pads every row
/// to `stride` bytes so that the output matches the layout the encoder
/// expects.
fn i420_to_nv12(i420: &[u8], width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let cw = (w + 1) / 2;
    let ch = (h + 1) / 2;

    // Round up to 64-byte boundary (common DMA / hardware alignment).
    let stride = ((w + 63) / 64) * 64;

    let y = &i420[..w * h];
    let u = &i420[w * h..w * h + cw * ch];
    let v = &i420[w * h + cw * ch..w * h + 2 * cw * ch];

    let y_padded = stride * h;
    let uv_stride = stride;                     // NV12 UV stride == Y stride
    let uv_rows = ch;                           // one UV row per 2 Y rows
    let uv_padded = uv_stride * uv_rows;
    let total = y_padded + uv_padded;

    let mut nv12 = vec![0u8; total];

    // ── Y plane: copy row by row & pad ──
    for row in 0..h {
        let src = &y[row * w..(row + 1) * w];
        let dst_off = row * stride;
        nv12[dst_off..dst_off + w].copy_from_slice(src);
        // remaining bytes in the row are already zero (vec initialised).
    }

    // ── UV plane: interleave U/V row by row & pad ──
    let uv_base = y_padded;
    for row in 0..ch {
        let u_src_off = row * cw;
        let v_src_off = row * cw;
        let dst_row_off = uv_base + row * uv_stride;
        for col in 0..cw {
            nv12[dst_row_off + col * 2] = u[u_src_off + col];
            nv12[dst_row_off + col * 2 + 1] = v[v_src_off + col];
        }
        // padding at row end is already zero.
    }

    nv12
}

// ---------------------------------------------------------------------------
// AVCC → Annex-B conversion
// ---------------------------------------------------------------------------

/// Check if data starts with an Annex-B start code (0x000001 or 0x00000001).
fn is_annexb_format(data: &[u8]) -> bool {
    if data.len() >= 4 && data[0] == 0 && data[1] == 0 && data[2] == 0 && data[3] == 1 {
        return true;
    }
    if data.len() >= 3 && data[0] == 0 && data[1] == 0 && data[2] == 1 {
        return true;
    }
    false
}

/// Convert AVCC format (4-byte length prefix) to Annex-B (start codes).
///
/// AVCC encodes each NALU as: [4-byte big-endian length][NALU data].
/// Annex-B encodes each NALU as: [0x00 0x00 0x00 0x01][NALU data].
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

/// Parse AVCDecoderConfigurationRecord (codec_data) and extract SPS/PPS as Annex-B.
///
/// The record format is:
///   [0] version (always 1)
///   [1] profile
///   [2] profile_compat
///   [3] level
///   [4] 0xFC | (lengthSizeMinusOne & 0x03)  — typically 0xFF (4-byte lengths)
///   [5] 0xE0 | numSPS
///   For each SPS: [2-byte length][SPS data]
///   [n] numPPS
///   For each PPS: [2-byte length][PPS data]
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

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// H.264 hardware encoder backed by OH_AVCodec.
pub struct H264Encoder {
    codec: *mut OH_AVCodec,
    user_data: Arc<EncoderUserData>,
    width: u32,
    height: u32,
    is_running: AtomicBool,
    frame_count: u64,
}

unsafe impl Send for H264Encoder {}

impl H264Encoder {
    /// Lightweight probe: check if any H.264 encoder exists on this device.
    ///
    /// Result is cached after the first call so subsequent invocations are
    /// essentially free.  Used by the SDP layer to decide whether H.264 or
    /// VP8 should be listed first in the offer.
    pub fn is_available() -> bool {
        static CACHED: OnceLock<bool> = OnceLock::new();
        *CACHED.get_or_init(|| {
            let mime = std::ffi::CString::new("video/avc").expect("valid cstr");
            let mut codec = unsafe { OH_VideoEncoder_CreateByMime(mime.as_ptr()) };
            if codec.is_null() {
                for name in &[
                    "OMX.hisi.video.encoder.avc",
                    "OMX.qcom.video.encoder.avc",
                    "OMX.mtk.video.encoder.avc",
                    "c2.android.avc.encoder",
                    "OMX.google.h264.encoder",
                    "OH.Media.Codec.Encoder.Video.Avc",
                ] {
                    let cstr = std::ffi::CString::new(*name).expect("valid cstr");
                    codec = unsafe { OH_VideoEncoder_CreateByName(cstr.as_ptr()) };
                    if !codec.is_null() {
                        break;
                    }
                }
            }
            let available = !codec.is_null();
            if !codec.is_null() {
                unsafe { OH_VideoEncoder_Destroy(codec) };
            }
            log::info!("[H264Encoder] is_available probe: {available}");
            available
        })
    }

    pub fn new(width: u32, height: u32, bitrate_kbps: u32) -> Result<Self, RtcError> {
        if width == 0 || height == 0 {
            return Err(err(format!("invalid H264 dimensions: {width}x{height}")));
        }

        let mime = std::ffi::CString::new("video/avc")
            .map_err(|_| err("invalid MIME string".into()))?;
        let mut codec = unsafe { OH_VideoEncoder_CreateByMime(mime.as_ptr()) };

        if codec.is_null() {
            log::warn!("[H264Encoder] hardware H.264 (OH_VideoEncoder_CreateByMime) returned null, trying named codecs");
            // Try OHOS-specific codec names first, then Android fallbacks
            for name in &[
                // OHOS native codec names (vendor-specific)
                "OMX.hisi.video.encoder.avc",       // HiSilicon (Huawei/Honor)
                "OMX.qcom.video.encoder.avc",        // Qualcomm
                "OMX.mtk.video.encoder.avc",         // MediaTek
                // Android fallbacks (may exist on some OHOS builds)
                "c2.android.avc.encoder",
                "OMX.google.h264.encoder",
                // OHOS generic software encoder
                "OH.Media.Codec.Encoder.Video.Avc",
            ] {
                let cstr = std::ffi::CString::new(*name).expect("valid cstr");
                codec = unsafe { OH_VideoEncoder_CreateByName(cstr.as_ptr()) };
                if !codec.is_null() {
                    log::info!("[H264Encoder] using codec by name: {name}");
                    break;
                }
            }
        }
        if codec.is_null() {
            log::error!("[H264Encoder] CRITICAL: no H.264 encoder available on this device. All CreateByMime + CreateByName attempts returned null.");
            return Err(err("no H.264 encoder available on this OHOS device".into()));
        }

        let user_data = Arc::new(EncoderUserData::new());
        let ud_ptr = Arc::as_ptr(&user_data) as *mut c_void;
        let callback = OH_AVCodecCallback {
            on_error: Some(Self::on_error),
            on_stream_changed: Some(Self::on_stream_changed),
            on_need_input_buffer: Some(Self::on_need_input_buffer),
            on_new_output_buffer: Some(Self::on_new_output_buffer),
        };

        let ret = unsafe { OH_VideoEncoder_RegisterCallback(codec, callback, ud_ptr) };
        if ret != AV_ERR_OK {
            unsafe { OH_VideoEncoder_Destroy(codec) };
            return Err(err(format!("RegisterCallback failed: {ret}")));
        }

        let format = unsafe { OH_AVFormat_Create() };
        if format.is_null() {
            unsafe { OH_VideoEncoder_Destroy(codec) };
            return Err(err("OH_AVFormat_Create failed".into()));
        }
        unsafe {
            OH_AVFormat_SetIntValue(format, b"width\0".as_ptr() as _, width as i32);
            OH_AVFormat_SetIntValue(format, b"height\0".as_ptr() as _, height as i32);
            OH_AVFormat_SetIntValue(format, b"pixel_format\0".as_ptr() as _, AV_PIXEL_FORMAT_NV12);
            OH_AVFormat_SetDoubleValue(format, b"frame_rate\0".as_ptr() as _, 30.0);
            OH_AVFormat_SetLongValue(format, b"bitrate\0".as_ptr() as _, (bitrate_kbps as i64) * 1000);
            OH_AVFormat_SetIntValue(format, b"i_frame_interval\0".as_ptr() as _, 5);
            // Set H264 profile to Baseline and level to 3.1 for maximum browser compatibility
            // OH_AVCodec profile values (based on Android MediaCodec): 1=Baseline, 2=Main, 8=High
            OH_AVFormat_SetIntValue(format, b"profile\0".as_ptr() as _, 1);  // Baseline Profile
            OH_AVFormat_SetIntValue(format, b"level\0".as_ptr() as _, 31);   // Level 3.1
        }

        let ret = unsafe { OH_VideoEncoder_Configure(codec, format) };
        unsafe { OH_AVFormat_Destroy(format) };
        if ret != AV_ERR_OK { unsafe { OH_VideoEncoder_Destroy(codec) }; return Err(err(format!("Configure failed: {ret}"))); }

        let ret = unsafe { OH_VideoEncoder_Prepare(codec) };
        if ret != AV_ERR_OK { unsafe { OH_VideoEncoder_Destroy(codec) }; return Err(err(format!("Prepare failed: {ret}"))); }

        let ret = unsafe { OH_VideoEncoder_Start(codec) };
        if ret != AV_ERR_OK { unsafe { OH_VideoEncoder_Destroy(codec) }; return Err(err(format!("Start failed: {ret}"))); }

        log::info!("[H264Encoder] initialised {width}x{height} @ {bitrate_kbps}kbps (OH_AVCodec)");
        Ok(Self { codec, user_data, width, height, is_running: AtomicBool::new(true),
            frame_count: 0 })
    }

    /// Push an I420 frame and return any available encoded H.264 packet.
    pub fn encode(&mut self, i420_data: &[u8], timestamp_us: i64) -> Result<Option<(Vec<u8>, bool)>, RtcError> {
        let expected = (self.width * self.height * 3 / 2) as usize;
        if i420_data.len() < expected {
            return Err(err(format!("I420 buffer too small: {} < {}", i420_data.len(), expected)));
        }

        let nv12 = i420_to_nv12(i420_data, self.width, self.height);
        {
            let mut pq = self.user_data.pending_frames.lock();
            if pq.len() >= 8 { pq.pop_front(); }
            pq.push_back(PendingFrame { data: nv12, timestamp_us });

        }
        self.frame_count += 1;
        if self.frame_count % 30 == 1 {
            let out = self.user_data.output_queue.lock().len();
            log::info!("[H264Encoder] frame #{}: {}x{} pending={} out={}",
                self.frame_count, self.width, self.height,
                self.user_data.pending_frames.lock().len(), out);
        }
        Ok(self.poll_output())
    }

    fn drain_pending(&self) {
        loop {
            let info = match self.user_data.input_queue.lock().pop_front() {
                Some(i) => i,
                None => break,
            };
            let frame = match self.user_data.pending_frames.lock().pop_front() {
                Some(f) => f,
                None => { self.user_data.input_queue.lock().push_front(info); break; }
            };
            unsafe {
                let addr = OH_AVBuffer_GetAddr(info.buffer_ptr);
                let cap = OH_AVBuffer_GetCapacity(info.buffer_ptr);
                if addr.is_null() || cap < 0 || frame.data.len() > cap as usize {
                    OH_VideoEncoder_PushInputBuffer(self.codec, info.index);
                    continue;
                }
                // Zero-fill entire buffer to prevent stale data in stride-padded regions
                ptr::write_bytes(addr, 0u8, cap as usize);
                ptr::copy_nonoverlapping(frame.data.as_ptr(), addr, frame.data.len());
                let attr = OH_AVCodecBufferAttr {
                    pts: frame.timestamp_us, size: frame.data.len() as i32,
                    offset: 0, flags: AVCODEC_BUFFER_FLAGS_NONE,
                };
                if OH_AVBuffer_SetBufferAttr(info.buffer_ptr, &attr) != AV_ERR_OK {
                    OH_VideoEncoder_PushInputBuffer(self.codec, info.index);
                    continue;
                }
                if OH_VideoEncoder_PushInputBuffer(self.codec, info.index) == AV_ERR_OK {
                    let n = self.user_data.encode_count.fetch_add(1, Ordering::Relaxed) + 1;
                    if n <= 5 || n % 100 == 0 {
                        log::info!("[H264Encoder] encoded #{n}: {} bytes (buf_cap={cap}, gap={})",
                            frame.data.len(), cap as usize - frame.data.len());
                    }
                }
            }
        }
    }

    fn poll_output(&self) -> Option<(Vec<u8>, bool)> {
        self.user_data.output_queue.lock().pop_front().map(|p| (p.data, p.is_key_frame))
    }

    unsafe extern "C" fn on_error(_: *mut OH_AVCodec, code: c_int, _: *mut c_void) {
        log::error!("[H264Encoder] error: {code}");
    }
    unsafe extern "C" fn on_stream_changed(_: *mut OH_AVCodec, _: *mut OH_AVFormat, _: *mut c_void) {
        log::info!("[H264Encoder] stream changed");
    }

    unsafe extern "C" fn on_need_input_buffer(
        codec: *mut OH_AVCodec, index: c_uint, buffer: *mut OH_AVBuffer, user_data: *mut c_void,
    ) {
        let data = &*(user_data as *const EncoderUserData);
        let count = data.input_cb_count.fetch_add(1, Ordering::Relaxed) + 1;

        if let Some(frame) = data.pending_frames.lock().pop_front() {
            let cap = OH_AVBuffer_GetCapacity(buffer);
            let addr = OH_AVBuffer_GetAddr(buffer);
            if !addr.is_null() && cap >= 0 && frame.data.len() <= cap as usize {
                ptr::copy_nonoverlapping(frame.data.as_ptr(), addr, frame.data.len());
                let attr = OH_AVCodecBufferAttr {
                    pts: frame.timestamp_us, size: frame.data.len() as i32,
                    offset: 0, flags: AVCODEC_BUFFER_FLAGS_NONE,
                };
                if OH_AVBuffer_SetBufferAttr(buffer, &attr) == AV_ERR_OK
                    && OH_VideoEncoder_PushInputBuffer(codec, index) == AV_ERR_OK
                {
                    let n = data.encode_count.fetch_add(1, Ordering::Relaxed) + 1;
                    if n <= 5 || n % 100 == 0 {
                        log::info!("[H264Encoder] cb encode #{n}: {} bytes", frame.data.len());
                    }
                    return;
                }
            }
            OH_VideoEncoder_PushInputBuffer(codec, index);
            return;
        }

        let cap = OH_AVBuffer_GetCapacity(buffer);
        let info = InputBufferInfo { index, buffer_ptr: buffer, capacity: cap };
        {
            let mut iq = data.input_queue.lock();
            if iq.len() >= 16 { iq.pop_front(); }
            iq.push_back(info);
        }
        if count <= 3 || count % 100 == 0 {
            log::info!("[H264Encoder] input buffer cached: idx={index} cap={cap} cb=#{count}");
        }
    }

    unsafe extern "C" fn on_new_output_buffer(
        codec: *mut OH_AVCodec, index: c_uint, buffer: *mut OH_AVBuffer, user_data: *mut c_void,
    ) {
        let data = &*(user_data as *const EncoderUserData);
        let mut attr = OH_AVCodecBufferAttr { pts: 0, size: 0, offset: 0, flags: 0 };
        if OH_AVBuffer_GetBufferAttr(buffer, &mut attr) != AV_ERR_OK {
            OH_VideoEncoder_FreeOutputBuffer(codec, index);
            return;
        }
        let addr = OH_AVBuffer_GetAddr(buffer);
        let len = attr.size.max(0) as usize;
        let is_codec_data = (attr.flags & AVCODEC_BUFFER_FLAGS_CODEC_DATA) != 0;
        let is_key = (attr.flags & AVCODEC_BUFFER_FLAGS_SYNC_FRAME) != 0;

        if !addr.is_null() && len > 0 {
            let mut buf = vec![0u8; len];
            ptr::copy_nonoverlapping(addr.add(attr.offset.max(0) as usize), buf.as_mut_ptr(), len);

            let out_num = data.output_count.fetch_add(1, Ordering::Relaxed) + 1;

            if is_codec_data {
                // codec_data may be AVCDecoderConfigurationRecord or Annex-B SPS/PPS.
                // Try to parse as AVCDecoderConfigurationRecord first; if it fails,
                // check if it's already Annex-B; otherwise treat as raw Annex-B.
                let annexb_cd = if is_annexb_format(&buf) {
                    // Already Annex-B format
                    log::info!(
                        "[H264Encoder] codec_data (Annex-B SPS/PPS): {} bytes, head=[{}]",
                        len,
                        buf.iter().take(8).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
                    );
                    buf
                } else if let Some(parsed) = avcc_extradata_to_annexb(&buf) {
                    // AVCDecoderConfigurationRecord → converted to Annex-B
                    log::info!(
                        "[H264Encoder] codec_data (AVCDecoderConfigRecord→Annex-B): orig={} bytes → {} bytes, \
                         head=[{}]",
                        len, parsed.len(),
                        parsed.iter().take(12).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
                    );
                    parsed
                } else {
                    // Unknown format, use as-is and hope for the best
                    log::warn!(
                        "[H264Encoder] codec_data (unknown format): {} bytes, head=[{}]",
                        len,
                        buf.iter().take(12).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
                    );
                    buf
                };
                let mut cd = data.codec_data.lock();
                *cd = annexb_cd;
            } else {
                // Regular frame data: detect AVCC vs Annex-B and convert if needed.
                let frame_is_annexb = is_annexb_format(&buf);
                let converted_buf = if !frame_is_annexb {
                    // AVCC format detected — convert to Annex-B
                    let converted = avcc_to_annexb(&buf);
                    if out_num <= 5 || out_num % 100 == 0 {
                        log::info!(
                            "[H264Encoder] frame #{}: AVCC→Annex-B, orig={} → {} bytes, key={}, \
                             orig_head=[{}], conv_head=[{}]",
                            out_num, buf.len(), converted.len(), is_key,
                            buf.iter().take(8).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "),
                            converted.iter().take(8).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "),
                        );
                    }
                    if converted.is_empty() {
                        // Conversion failed (maybe not actually AVCC), use original
                        log::warn!(
                            "[H264Encoder] frame #{}: AVCC conversion produced empty result, using raw",
                            out_num
                        );
                        buf
                    } else {
                        converted
                    }
                } else {
                    // Already Annex-B
                    if out_num <= 5 || out_num % 100 == 0 {
                        log::info!(
                            "[H264Encoder] frame #{}: Annex-B, {} bytes, key={}, head=[{}]",
                            out_num, buf.len(), is_key,
                            buf.iter().take(8).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "),
                        );
                    }
                    buf
                };

                // Prepend SPS/PPS to keyframes
                let final_buf = if is_key {
                    let cd = data.codec_data.lock();
                    if !cd.is_empty() {
                        let mut combined = Vec::with_capacity(cd.len() + converted_buf.len());
                        combined.extend_from_slice(&cd);
                        combined.extend_from_slice(&converted_buf);
                        if out_num <= 10 || out_num % 100 == 0 {
                            log::info!(
                                "[H264Encoder] keyframe #{}: SPS/PPS({}) + frame({}) = {} bytes",
                                out_num, cd.len(), converted_buf.len(), combined.len(),
                            );
                        }
                        combined
                    } else {
                        converted_buf
                    }
                } else {
                    converted_buf
                };

                let pkt = EncodedPacket {
                    data: final_buf, timestamp_us: attr.pts, is_key_frame: is_key,
                };
                let mut oq = data.output_queue.lock();
                if oq.len() >= 64 { oq.pop_front(); }
                oq.push_back(pkt);
            }
        }
        OH_VideoEncoder_FreeOutputBuffer(codec, index);
    }
}

impl Drop for H264Encoder {
    fn drop(&mut self) {
        self.is_running.store(false, Ordering::SeqCst);
        if !self.codec.is_null() {
            unsafe { OH_VideoEncoder_Stop(self.codec); OH_VideoEncoder_Destroy(self.codec); }
            self.codec = null_mut();
        }
        log::info!("[H264Encoder] destroyed after {} frames", self.frame_count);
    }
}
