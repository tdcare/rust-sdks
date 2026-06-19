//! OHOS hardware codec implementation - encoder + decoder.
//!
//! Compiled only for `target_env = "ohos"`. Provides the unsafe FFI plumbing
//! for [`super::VideoEncoder`] and [`super::VideoDecoder`].

use std::collections::VecDeque;
use std::ffi::{c_void, CString};
use std::os::raw::{c_char, c_int, c_uint};
use std::ptr;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc, Mutex,
};

use super::ffi::*;
use super::{
    CodecError, DecodedFrame, EncodedFrame, Result, VideoDecoderConfig, VideoEncoderConfig,
};

struct InputBufferInfo {
    index: u32,
    buffer_ptr: *mut OH_AVBuffer,
    capacity: i32,
}
// SAFETY: pointer is owned by the native codec; we only forward it back into
// FFI calls that expect it on any thread.
unsafe impl Send for InputBufferInfo {}

struct PendingFrame {
    data: Vec<u8>,
    timestamp_us: i64,
    flags: u32,
}

fn check(op: &'static str, code: c_int) -> Result<()> {
    if code == AV_ERR_OK {
        Ok(())
    } else {
        Err(CodecError::Native { op, code })
    }
}

// ============================================================
// Encoder
// ============================================================

struct EncoderUserData {
    input_queue: Mutex<VecDeque<InputBufferInfo>>,
    pending: Mutex<VecDeque<PendingFrame>>,
    output_queue: Mutex<VecDeque<EncodedFrame>>,
    last_error: Mutex<Option<i32>>,
}

impl EncoderUserData {
    fn new() -> Self {
        Self {
            input_queue: Mutex::new(VecDeque::new()),
            pending: Mutex::new(VecDeque::new()),
            output_queue: Mutex::new(VecDeque::new()),
            last_error: Mutex::new(None),
        }
    }
}

/// Hardware H.264 video encoder (NV12 input, Annex-B H.264 output).
pub(crate) struct VideoEncoder {
    codec: *mut OH_AVCodec,
    config: VideoEncoderConfig,
    user_data: Arc<EncoderUserData>,
    is_running: AtomicBool,
    is_initialized: AtomicBool,
}
// SAFETY: All FFI calls take an opaque codec handle; concurrent access is
// also gated by the outer wrapper's `Mutex`.
unsafe impl Send for VideoEncoder {}
unsafe impl Sync for VideoEncoder {}

impl VideoEncoder {
    pub(crate) fn new_h264(config: VideoEncoderConfig) -> Result<Self> {
        let mime = CString::new("video/avc")
            .map_err(|_| CodecError::CreateFailed("invalid mime".into()))?;
        // SAFETY: `mime` is valid for the call.
        let codec = unsafe { OH_VideoEncoder_CreateByMime(mime.as_ptr()) };
        if codec.is_null() {
            return Err(CodecError::CreateFailed(
                "OH_VideoEncoder_CreateByMime returned null".into(),
            ));
        }
        Ok(Self {
            codec,
            config,
            user_data: Arc::new(EncoderUserData::new()),
            is_running: AtomicBool::new(false),
            is_initialized: AtomicBool::new(false),
        })
    }

    pub(crate) fn initialize(&self) -> Result<()> {
        if self.codec.is_null() {
            return Err(CodecError::NotInitialized);
        }
        let user_data = Arc::as_ptr(&self.user_data) as *mut c_void;
        let cb = OH_AVCodecCallback {
            on_error: Some(on_error),
            on_stream_changed: Some(on_stream_changed),
            on_need_input_buffer: Some(on_need_input_buffer),
            on_new_output_buffer: Some(on_new_output_buffer),
        };
        // SAFETY: `self.codec` is valid; user_data kept alive by Arc in `self`.
        check("OH_VideoEncoder_RegisterCallback", unsafe {
            OH_VideoEncoder_RegisterCallback(self.codec, cb, user_data)
        })?;

        // SAFETY: returns owned format or null; checked below.
        let format = unsafe { OH_AVFormat_Create() };
        if format.is_null() {
            return Err(CodecError::CreateFailed("OH_AVFormat_Create returned null".into()));
        }
        // SAFETY: keys are static NUL-terminated literals; format is owned.
        unsafe {
            OH_AVFormat_SetIntValue(format, b"width\0".as_ptr() as *const c_char, self.config.width as i32);
            OH_AVFormat_SetIntValue(format, b"height\0".as_ptr() as *const c_char, self.config.height as i32);
            OH_AVFormat_SetIntValue(format, b"pixel_format\0".as_ptr() as *const c_char, self.config.pixel_format);
            OH_AVFormat_SetDoubleValue(format, b"frame_rate\0".as_ptr() as *const c_char, self.config.frame_rate as f64);
            OH_AVFormat_SetLongValue(format, b"bitrate\0".as_ptr() as *const c_char, self.config.bit_rate as i64);
            OH_AVFormat_SetIntValue(format, b"i_frame_interval\0".as_ptr() as *const c_char, self.config.i_frame_interval_ms);
        }
        // SAFETY: codec/format valid for call.
        let cfg = unsafe { OH_VideoEncoder_Configure(self.codec, format) };
        // SAFETY: `format` is owned and must be destroyed regardless of result.
        unsafe { OH_AVFormat_Destroy(format) };
        check("OH_VideoEncoder_Configure", cfg)?;
        // SAFETY: codec valid.
        check("OH_VideoEncoder_Prepare", unsafe { OH_VideoEncoder_Prepare(self.codec) })?;
        self.is_initialized.store(true, Ordering::SeqCst);
        log::info!(
            "[ohos][VideoEncoder] initialized {}x{} @ {} fps {} bps",
            self.config.width, self.config.height, self.config.frame_rate, self.config.bit_rate
        );
        Ok(())
    }

    pub(crate) fn start(&self) -> Result<()> {
        if !self.is_initialized.load(Ordering::SeqCst) {
            return Err(CodecError::NotInitialized);
        }
        // SAFETY: codec valid.
        check("OH_VideoEncoder_Start", unsafe { OH_VideoEncoder_Start(self.codec) })?;
        self.is_running.store(true, Ordering::SeqCst);
        Ok(())
    }

    pub(crate) fn stop(&self) -> Result<()> {
        if self.codec.is_null() {
            return Ok(());
        }
        // SAFETY: codec valid.
        let r = unsafe { OH_VideoEncoder_Stop(self.codec) };
        self.is_running.store(false, Ordering::SeqCst);
        check("OH_VideoEncoder_Stop", r)
    }

    pub(crate) fn encode_frame(&self, yuv: &[u8], pts: i64) -> Result<()> {
        if !self.is_running.load(Ordering::SeqCst) {
            return Err(CodecError::NotRunning);
        }
        let stored = self
            .user_data
            .input_queue
            .lock()
            .map_err(|_| CodecError::Poisoned)?
            .pop_front();
        if let Some(info) = stored {
            if self.fill_and_push(info, yuv, pts, AVCODEC_BUFFER_FLAGS_NONE)? {
                return Ok(());
            }
        }
        let mut q = self.user_data.pending.lock().map_err(|_| CodecError::Poisoned)?;
        if q.len() >= 30 {
            q.pop_front();
        }
        q.push_back(PendingFrame {
            data: yuv.to_vec(),
            timestamp_us: pts,
            flags: AVCODEC_BUFFER_FLAGS_NONE,
        });
        Ok(())
    }

    fn fill_and_push(
        &self,
        info: InputBufferInfo,
        data: &[u8],
        pts: i64,
        flags: u32,
    ) -> Result<bool> {
        // SAFETY: `info.buffer_ptr` was supplied by the codec callback and is
        // valid until pushed back via PushInputBuffer.
        let (addr, cap) = unsafe {
            (OH_AVBuffer_GetAddr(info.buffer_ptr), OH_AVBuffer_GetCapacity(info.buffer_ptr))
        };
        if addr.is_null() || cap < 0 || data.len() > cap as usize {
            // SAFETY: best-effort release to avoid starving the codec.
            unsafe { OH_VideoEncoder_PushInputBuffer(self.codec, info.index) };
            return Ok(false);
        }
        // SAFETY: bounds verified above.
        unsafe { ptr::copy_nonoverlapping(data.as_ptr(), addr, data.len()) };
        let attr = OH_AVCodecBufferAttr { pts, size: data.len() as c_int, offset: 0, flags };
        // SAFETY: buffer/attr valid for the call.
        let set = unsafe { OH_AVBuffer_SetBufferAttr(info.buffer_ptr, &attr) };
        if set != AV_ERR_OK {
            // SAFETY: best-effort release.
            unsafe { OH_VideoEncoder_PushInputBuffer(self.codec, info.index) };
            return Ok(false);
        }
        // SAFETY: codec valid.
        check("OH_VideoEncoder_PushInputBuffer", unsafe {
            OH_VideoEncoder_PushInputBuffer(self.codec, info.index)
        })?;
        Ok(true)
    }

    pub(crate) fn poll_output(&self) -> Option<EncodedFrame> {
        self.user_data.output_queue.lock().ok()?.pop_front()
    }

    pub(crate) fn request_key_frame(&self) -> Result<()> {
        // SAFETY: returns owned format or null.
        let f = unsafe { OH_AVFormat_Create() };
        if f.is_null() {
            return Err(CodecError::CreateFailed("OH_AVFormat_Create returned null".into()));
        }
        // SAFETY: key is a static C string.
        unsafe { OH_AVFormat_SetIntValue(f, b"req_key_frame\0".as_ptr() as *const c_char, 1) };
        // SAFETY: codec/format valid for call.
        let r = unsafe { OH_VideoEncoder_SetParameter(self.codec, f) };
        // SAFETY: own format must be destroyed.
        unsafe { OH_AVFormat_Destroy(f) };
        check("OH_VideoEncoder_SetParameter", r)
    }
}

impl Drop for VideoEncoder {
    fn drop(&mut self) {
        if !self.codec.is_null() {
            let _ = self.stop();
            // SAFETY: codec handle still owned by `self`.
            unsafe { OH_VideoEncoder_Destroy(self.codec) };
            self.codec = ptr::null_mut();
        }
    }
}

unsafe extern "C" fn on_error(_c: *mut OH_AVCodec, code: c_int, ud: *mut c_void) {
    // SAFETY: `ud` is `Arc<EncoderUserData>::as_ptr` kept alive by wrapper.
    let d = unsafe { &*(ud as *const EncoderUserData) };
    if let Ok(mut e) = d.last_error.lock() {
        *e = Some(code);
    }
    log::error!("[ohos][VideoEncoder] error callback: {code}");
}

unsafe extern "C" fn on_stream_changed(
    _c: *mut OH_AVCodec,
    _f: *mut OH_AVFormat,
    _ud: *mut c_void,
) {
    log::debug!("[ohos][VideoEncoder] stream changed");
}

unsafe extern "C" fn on_need_input_buffer(
    codec: *mut OH_AVCodec,
    index: c_uint,
    buffer: *mut OH_AVBuffer,
    ud: *mut c_void,
) {
    // SAFETY: see `on_error`.
    let d = unsafe { &*(ud as *const EncoderUserData) };
    let pending = d.pending.lock().ok().and_then(|mut q| q.pop_front());
    if let Some(frame) = pending {
        // SAFETY: buffer is valid for the duration of this callback.
        let cap = unsafe { OH_AVBuffer_GetCapacity(buffer) };
        let addr = unsafe { OH_AVBuffer_GetAddr(buffer) };
        if !addr.is_null() && (frame.data.len() as i32) <= cap {
            // SAFETY: bounds verified above.
            unsafe { ptr::copy_nonoverlapping(frame.data.as_ptr(), addr, frame.data.len()) };
            let attr = OH_AVCodecBufferAttr {
                pts: frame.timestamp_us,
                size: frame.data.len() as c_int,
                offset: 0,
                flags: frame.flags,
            };
            // SAFETY: buffer/attr valid.
            if unsafe { OH_AVBuffer_SetBufferAttr(buffer, &attr) } == AV_ERR_OK {
                // SAFETY: codec/index valid for callback duration.
                unsafe { OH_VideoEncoder_PushInputBuffer(codec, index) };
                return;
            }
        }
        // SAFETY: best-effort release.
        unsafe { OH_VideoEncoder_PushInputBuffer(codec, index) };
        return;
    }
    // SAFETY: buffer valid.
    let cap = unsafe { OH_AVBuffer_GetCapacity(buffer) };
    if let Ok(mut q) = d.input_queue.lock() {
        if q.len() >= 16 {
            q.pop_front();
        }
        q.push_back(InputBufferInfo { index, buffer_ptr: buffer, capacity: cap });
    }
}

unsafe extern "C" fn on_new_output_buffer(
    codec: *mut OH_AVCodec,
    index: c_uint,
    buffer: *mut OH_AVBuffer,
    ud: *mut c_void,
) {
    // SAFETY: see `on_error`.
    let d = unsafe { &*(ud as *const EncoderUserData) };
    let mut attr = OH_AVCodecBufferAttr { pts: 0, size: 0, offset: 0, flags: 0 };
    // SAFETY: buffer valid for callback duration.
    if unsafe { OH_AVBuffer_GetBufferAttr(buffer, &mut attr) } != AV_ERR_OK {
        // SAFETY: must release buffer back to codec.
        unsafe { OH_VideoEncoder_FreeOutputBuffer(codec, index) };
        return;
    }
    // SAFETY: buffer valid.
    let addr = unsafe { OH_AVBuffer_GetAddr(buffer) };
    if !addr.is_null() && attr.size > 0 {
        let mut buf = vec![0u8; attr.size as usize];
        // SAFETY: addr+offset is the start of attr.size valid bytes.
        unsafe {
            ptr::copy_nonoverlapping(
                addr.add(attr.offset as usize),
                buf.as_mut_ptr(),
                attr.size as usize,
            )
        };
        let frame = EncodedFrame {
            data: buf,
            timestamp_us: attr.pts,
            is_key_frame: (attr.flags & AVCODEC_BUFFER_FLAGS_SYNC_FRAME) != 0,
            is_eos: (attr.flags & AVCODEC_BUFFER_FLAGS_EOS) != 0,
        };
        if let Ok(mut q) = d.output_queue.lock() {
            if q.len() >= 64 {
                q.pop_front();
            }
            q.push_back(frame);
        }
    }
    // SAFETY: must release buffer back to codec.
    unsafe { OH_VideoEncoder_FreeOutputBuffer(codec, index) };
}

// ============================================================
// Decoder
// ============================================================

struct DecoderUserData {
    input_queue: Mutex<VecDeque<InputBufferInfo>>,
    pending: Mutex<VecDeque<PendingFrame>>,
    output_queue: Mutex<VecDeque<DecodedFrame>>,
    last_error: Mutex<Option<i32>>,
    codec_data_sent: AtomicBool,
    frame_count: AtomicU32,
}

impl DecoderUserData {
    fn new() -> Self {
        Self {
            input_queue: Mutex::new(VecDeque::new()),
            pending: Mutex::new(VecDeque::new()),
            output_queue: Mutex::new(VecDeque::new()),
            last_error: Mutex::new(None),
            codec_data_sent: AtomicBool::new(false),
            frame_count: AtomicU32::new(0),
        }
    }
}

/// Hardware H.264 video decoder (Annex-B H.264 input, NV12 output).
pub(crate) struct VideoDecoder {
    codec: *mut OH_AVCodec,
    config: VideoDecoderConfig,
    user_data: Arc<DecoderUserData>,
    is_running: AtomicBool,
    is_initialized: AtomicBool,
}
// SAFETY: see encoder rationale.
unsafe impl Send for VideoDecoder {}
unsafe impl Sync for VideoDecoder {}

impl VideoDecoder {
    pub(crate) fn new_h264(config: VideoDecoderConfig) -> Result<Self> {
        let mime = CString::new("video/avc")
            .map_err(|_| CodecError::CreateFailed("invalid mime".into()))?;
        // SAFETY: `mime` is valid for the call.
        let codec = unsafe { OH_VideoDecoder_CreateByMime(mime.as_ptr()) };
        if codec.is_null() {
            return Err(CodecError::CreateFailed(
                "OH_VideoDecoder_CreateByMime returned null".into(),
            ));
        }
        Ok(Self {
            codec,
            config,
            user_data: Arc::new(DecoderUserData::new()),
            is_running: AtomicBool::new(false),
            is_initialized: AtomicBool::new(false),
        })
    }

    /// Create a hardware VP8 decoder.
    pub(crate) fn new_vp8(config: VideoDecoderConfig) -> Result<Self> {
        let mime = CString::new("video/x-vnd.on2.vp8")
            .map_err(|_| CodecError::CreateFailed("invalid mime".into()))?;
        // SAFETY: `mime` is valid for the call.
        let codec = unsafe { OH_VideoDecoder_CreateByMime(mime.as_ptr()) };
        if codec.is_null() {
            return Err(CodecError::CreateFailed(
                "OH_VideoDecoder_CreateByMime VP8 returned null".into(),
            ));
        }
        Ok(Self {
            codec,
            config,
            user_data: Arc::new(DecoderUserData::new()),
            is_running: AtomicBool::new(false),
            is_initialized: AtomicBool::new(false),
        })
    }

    pub(crate) fn initialize(&self) -> Result<()> {
        if self.codec.is_null() {
            return Err(CodecError::NotInitialized);
        }
        let user_data = Arc::as_ptr(&self.user_data) as *mut c_void;
        let cb = OH_AVCodecCallback {
            on_error: Some(dec_on_error),
            on_stream_changed: Some(dec_on_stream_changed),
            on_need_input_buffer: Some(dec_on_need_input_buffer),
            on_new_output_buffer: Some(dec_on_new_output_buffer),
        };
        // SAFETY: codec valid; user_data kept alive by Arc.
        check("OH_VideoDecoder_RegisterCallback", unsafe {
            OH_VideoDecoder_RegisterCallback(self.codec, cb, user_data)
        })?;

        // SAFETY: returns owned format or null.
        let f = unsafe { OH_AVFormat_Create() };
        if f.is_null() {
            return Err(CodecError::CreateFailed("OH_AVFormat_Create returned null".into()));
        }
        // SAFETY: keys are static C strings.
        unsafe {
            OH_AVFormat_SetIntValue(f, b"width\0".as_ptr() as *const c_char, self.config.width as i32);
            OH_AVFormat_SetIntValue(f, b"height\0".as_ptr() as *const c_char, self.config.height as i32);
            OH_AVFormat_SetIntValue(f, b"pixel_format\0".as_ptr() as *const c_char, self.config.pixel_format);
        }
        // SAFETY: codec/format valid.
        let r = unsafe { OH_VideoDecoder_Configure(self.codec, f) };
        // SAFETY: `f` is owned and must be destroyed regardless.
        unsafe { OH_AVFormat_Destroy(f) };
        check("OH_VideoDecoder_Configure", r)?;
        // SAFETY: codec valid.
        check("OH_VideoDecoder_Prepare", unsafe { OH_VideoDecoder_Prepare(self.codec) })?;
        self.is_initialized.store(true, Ordering::SeqCst);
        log::info!(
            "[ohos][VideoDecoder] initialized {}x{}",
            self.config.width, self.config.height
        );
        Ok(())
    }

    pub(crate) fn start(&self) -> Result<()> {
        if !self.is_initialized.load(Ordering::SeqCst) {
            return Err(CodecError::NotInitialized);
        }
        // SAFETY: codec valid.
        check("OH_VideoDecoder_Start", unsafe { OH_VideoDecoder_Start(self.codec) })?;
        self.is_running.store(true, Ordering::SeqCst);
        Ok(())
    }

    pub(crate) fn stop(&self) -> Result<()> {
        if self.codec.is_null() {
            return Ok(());
        }
        // SAFETY: codec valid.
        let r = unsafe { OH_VideoDecoder_Stop(self.codec) };
        self.is_running.store(false, Ordering::SeqCst);
        check("OH_VideoDecoder_Stop", r)
    }

    /// Submit one Annex-B unit for decoding. On the first key frame, SPS/PPS
    /// NALs are split out and pushed first as `CODEC_DATA`, followed by the
    /// IDR slice marked `SYNC_FRAME`.
    pub(crate) fn decode_frame(&self, nalu: &[u8], pts: i64, is_key: bool) -> Result<()> {
        if !self.is_running.load(Ordering::SeqCst) {
            return Err(CodecError::NotRunning);
        }
        self.user_data.frame_count.fetch_add(1, Ordering::Relaxed);

        if is_key && !self.user_data.codec_data_sent.load(Ordering::Relaxed) {
            if let Some((csd, idr_off)) = super::extract_sps_pps_from_annexb(nalu) {
                self.submit(&csd, pts, AVCODEC_BUFFER_FLAGS_CODEC_DATA)?;
                self.user_data.codec_data_sent.store(true, Ordering::Relaxed);
                if idr_off < nalu.len() {
                    self.submit(&nalu[idr_off..], pts, AVCODEC_BUFFER_FLAGS_SYNC_FRAME)?;
                }
                return Ok(());
            }
        }
        let flags = if is_key {
            AVCODEC_BUFFER_FLAGS_SYNC_FRAME
        } else {
            AVCODEC_BUFFER_FLAGS_NONE
        };
        self.submit(nalu, pts, flags)
    }

    fn submit(&self, data: &[u8], pts: i64, flags: u32) -> Result<()> {
        let stored = self
            .user_data
            .input_queue
            .lock()
            .map_err(|_| CodecError::Poisoned)?
            .pop_front();
        if let Some(info) = stored {
            // SAFETY: `info.buffer_ptr` was supplied by `dec_on_need_input_buffer`
            // and is valid until pushed back.
            let (addr, cap) = unsafe {
                (OH_AVBuffer_GetAddr(info.buffer_ptr), OH_AVBuffer_GetCapacity(info.buffer_ptr))
            };
            if !addr.is_null() && (data.len() as i32) <= cap {
                // SAFETY: bounds verified above.
                unsafe { ptr::copy_nonoverlapping(data.as_ptr(), addr, data.len()) };
                let attr = OH_AVCodecBufferAttr {
                    pts,
                    size: data.len() as c_int,
                    offset: 0,
                    flags,
                };
                // SAFETY: buffer/attr valid.
                if unsafe { OH_AVBuffer_SetBufferAttr(info.buffer_ptr, &attr) } == AV_ERR_OK {
                    // SAFETY: codec valid.
                    return check("OH_VideoDecoder_PushInputBuffer", unsafe {
                        OH_VideoDecoder_PushInputBuffer(self.codec, info.index)
                    });
                }
            }
            // SAFETY: best-effort release.
            unsafe { OH_VideoDecoder_PushInputBuffer(self.codec, info.index) };
        }
        let mut q = self.user_data.pending.lock().map_err(|_| CodecError::Poisoned)?;
        if q.len() >= 120 {
            q.pop_front();
        }
        q.push_back(PendingFrame { data: data.to_vec(), timestamp_us: pts, flags });
        Ok(())
    }

    pub(crate) fn poll_output(&self) -> Option<DecodedFrame> {
        self.user_data.output_queue.lock().ok()?.pop_front()
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        if !self.codec.is_null() {
            let _ = self.stop();
            // SAFETY: codec handle still owned by `self`.
            unsafe { OH_VideoDecoder_Destroy(self.codec) };
            self.codec = ptr::null_mut();
        }
    }
}

unsafe extern "C" fn dec_on_error(_c: *mut OH_AVCodec, code: c_int, ud: *mut c_void) {
    // SAFETY: `ud` is `Arc<DecoderUserData>::as_ptr`.
    let d = unsafe { &*(ud as *const DecoderUserData) };
    if let Ok(mut e) = d.last_error.lock() {
        *e = Some(code);
    }
    log::error!("[ohos][VideoDecoder] error callback: {code}");
}

unsafe extern "C" fn dec_on_stream_changed(
    _c: *mut OH_AVCodec,
    _f: *mut OH_AVFormat,
    _ud: *mut c_void,
) {
    log::debug!("[ohos][VideoDecoder] stream changed");
}

unsafe extern "C" fn dec_on_need_input_buffer(
    codec: *mut OH_AVCodec,
    index: c_uint,
    buffer: *mut OH_AVBuffer,
    ud: *mut c_void,
) {
    // SAFETY: see `dec_on_error`.
    let d = unsafe { &*(ud as *const DecoderUserData) };
    let pending = d.pending.lock().ok().and_then(|mut q| q.pop_front());
    if let Some(p) = pending {
        // SAFETY: buffer valid for callback duration.
        let cap = unsafe { OH_AVBuffer_GetCapacity(buffer) };
        let addr = unsafe { OH_AVBuffer_GetAddr(buffer) };
        if !addr.is_null() && (p.data.len() as i32) <= cap {
            // SAFETY: bounds verified above.
            unsafe { ptr::copy_nonoverlapping(p.data.as_ptr(), addr, p.data.len()) };
            let attr = OH_AVCodecBufferAttr {
                pts: p.timestamp_us,
                size: p.data.len() as c_int,
                offset: 0,
                flags: p.flags,
            };
            // SAFETY: buffer/attr valid.
            if unsafe { OH_AVBuffer_SetBufferAttr(buffer, &attr) } == AV_ERR_OK {
                // SAFETY: codec/index valid.
                unsafe { OH_VideoDecoder_PushInputBuffer(codec, index) };
                return;
            }
        }
        // SAFETY: best-effort release.
        unsafe { OH_VideoDecoder_PushInputBuffer(codec, index) };
        return;
    }
    // SAFETY: buffer valid.
    let cap = unsafe { OH_AVBuffer_GetCapacity(buffer) };
    if let Ok(mut q) = d.input_queue.lock() {
        if q.len() >= 16 {
            q.pop_front();
        }
        q.push_back(InputBufferInfo { index, buffer_ptr: buffer, capacity: cap });
    }
}

unsafe extern "C" fn dec_on_new_output_buffer(
    codec: *mut OH_AVCodec,
    index: c_uint,
    buffer: *mut OH_AVBuffer,
    ud: *mut c_void,
) {
    // SAFETY: see `dec_on_error`.
    let d = unsafe { &*(ud as *const DecoderUserData) };
    let mut attr = OH_AVCodecBufferAttr { pts: 0, size: 0, offset: 0, flags: 0 };
    // SAFETY: buffer valid.
    if unsafe { OH_AVBuffer_GetBufferAttr(buffer, &mut attr) } != AV_ERR_OK {
        // SAFETY: must release buffer.
        unsafe { OH_VideoDecoder_FreeOutputBuffer(codec, index) };
        return;
    }
    // SAFETY: buffer valid.
    let addr = unsafe { OH_AVBuffer_GetAddr(buffer) };
    if !addr.is_null() && attr.size > 0 {
        let mut buf = vec![0u8; attr.size as usize];
        // SAFETY: addr+offset points to attr.size valid bytes.
        unsafe {
            ptr::copy_nonoverlapping(
                addr.add(attr.offset as usize),
                buf.as_mut_ptr(),
                attr.size as usize,
            )
        };
        let frame = DecodedFrame {
            data: buf,
            timestamp_us: attr.pts,
            is_eos: (attr.flags & AVCODEC_BUFFER_FLAGS_EOS) != 0,
        };
        if let Ok(mut q) = d.output_queue.lock() {
            if q.len() >= 64 {
                q.pop_front();
            }
            q.push_back(frame);
        }
    }
    // SAFETY: must release buffer.
    unsafe { OH_VideoDecoder_FreeOutputBuffer(codec, index) };
}
