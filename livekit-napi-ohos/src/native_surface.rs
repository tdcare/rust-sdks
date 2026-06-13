//! NativeWindow Surface rendering for OHOS.
//!
//! Provides a [`YuvRenderer`] that writes I420 video frames into an
//! `OHNativeWindow` obtained from a Surface ID (XComponent). The renderer
//! converts I420 to RGBA_8888 in CPU and flushes the result to the display
//! compositor via the `OH_NativeWindow_*` API family (loaded at runtime via
//! `dlopen`).
//!
//! Buffer mapping strategy (API 12+):
//!   1. Primary: `OH_NativeBuffer_Map` / `OH_NativeBuffer_Unmap`
//!   2. Fallback: `BufferHandle.vir_addr` (pre-mapped)
//!   3. Last resort: `mmap(BufferHandle.fd)`

use std::collections::VecDeque;
use std::ffi::c_void;
use std::os::raw::{c_char, c_int};
use std::ptr;
use imgproc::colorcvt;
use std::sync::Once;

// ============================================================
// Opaque handle types
// ============================================================

/// Opaque `OHNativeWindow` handle.
#[repr(C)]
pub(crate) struct OHNativeWindow {
    _private: [u8; 0],
}

/// Opaque `OHNativeWindowBuffer` handle.
#[repr(C)]
struct OHNativeWindowBuffer {
    _private: [u8; 0],
}

/// Opaque `OH_NativeBuffer` handle (API 12+).
#[repr(C)]
struct OHNativeBuffer {
    _private: [u8; 0],
}

/// Buffer handle exposing the DMA-BUF fd and virtual address.
#[repr(C)]
struct BufferHandle {
    fd: i32,
    width: i32,
    height: i32,
    stride: i32,
    size: i32,
    format: i32,
    usage: u64,
    vir_addr: *mut c_void,
    phy_addr: u64,
}

/// Region passed to FlushBuffer (empty = full buffer).
#[repr(C)]
struct NwRegion {
    rect_count: c_int,
    rects: *const c_void,
}

// ============================================================
// POSIX helpers
// ============================================================

extern "C" {
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn mmap(
        addr: *mut c_void,
        length: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(addr: *mut c_void, length: usize) -> c_int;
}

const RTLD_LAZY: c_int = 0x0001;
const PROT_READ: c_int = 0x1;
const PROT_WRITE: c_int = 0x2;
const MAP_SHARED: c_int = 0x01;

// ============================================================
// Dynamic loading of libnative_window.so & libnative_buffer.so
// ============================================================

static NW_INIT: Once = Once::new();
static mut NW_HANDLE: *mut c_void = ptr::null_mut();

static NB_INIT: Once = Once::new();
static mut NB_HANDLE: *mut c_void = ptr::null_mut();

/// Load `libnative_window.so` (idempotent).
unsafe fn nw_lib() -> *mut c_void {
    NW_INIT.call_once(|| {
        NW_HANDLE = dlopen(
            b"libnative_window.so\0".as_ptr() as *const c_char,
            RTLD_LAZY,
        );
        if NW_HANDLE.is_null() {
            log::warn!("[NativeWindow] dlopen libnative_window.so failed");
        } else {
            log::info!("[NativeWindow] dlopen libnative_window.so OK");
        }
    });
    NW_HANDLE
}

/// Load `libnative_buffer.so` for OH_NativeBuffer APIs (API 12+).
unsafe fn nb_lib() -> *mut c_void {
    NB_INIT.call_once(|| {
        // Try libnative_buffer.so first, then fall back to libnative_window.so
        // (some OHOS versions bundle both in the same library).
        NB_HANDLE = dlopen(
            b"libnative_buffer.so\0".as_ptr() as *const c_char,
            RTLD_LAZY,
        );
        if NB_HANDLE.is_null() {
            // Fall back — symbols may live in libnative_window.so
            NB_HANDLE = nw_lib();
        }
        if NB_HANDLE.is_null() {
            log::info!("[NativeBuffer] OH_NativeBuffer APIs not available");
        } else {
            log::info!("[NativeBuffer] dlopen OK");
        }
    });
    NB_HANDLE
}

/// Look up a symbol in `libnative_window.so`.
unsafe fn nw_sym(name: &[u8]) -> *mut c_void {
    let h = nw_lib();
    if h.is_null() {
        return ptr::null_mut();
    }
    dlsym(h, name.as_ptr() as *const c_char)
}

/// Look up a symbol in the native buffer library.
unsafe fn nb_sym(name: &[u8]) -> *mut c_void {
    let h = nb_lib();
    if h.is_null() {
        return ptr::null_mut();
    }
    dlsym(h, name.as_ptr() as *const c_char)
}

// ============================================================
// NativeWindow runtime wrappers
// ============================================================

/// OHOS API 12: `OH_NativeWindow_CreateNativeWindowFromSurfaceId`
unsafe fn nw_create_from_surface_id(surface_id: u64) -> *mut OHNativeWindow {
    type F = unsafe extern "C" fn(u64, *mut *mut OHNativeWindow) -> c_int;
    let sym = nw_sym(b"OH_NativeWindow_CreateNativeWindowFromSurfaceId\0");
    if sym.is_null() {
        log::error!("[NativeWindow] CreateNativeWindowFromSurfaceId unavailable");
        return ptr::null_mut();
    }
    let func: F = std::mem::transmute(sym);
    let mut window: *mut OHNativeWindow = ptr::null_mut();
    let ret = func(surface_id, &mut window);
    if ret != 0 || window.is_null() {
        log::error!(
            "[NativeWindow] Create failed: surfaceId={}, ret={}",
            surface_id,
            ret
        );
        return ptr::null_mut();
    }
    log::info!("[NativeWindow] Created NativeWindow from surfaceId={}", surface_id);
    window
}

/// Destroy a NativeWindow.
unsafe fn nw_destroy(window: *mut OHNativeWindow) {
    type F = unsafe extern "C" fn(*mut OHNativeWindow);
    let sym = nw_sym(b"OH_NativeWindow_DestroyNativeWindow\0");
    if sym.is_null() {
        return;
    }
    let func: F = std::mem::transmute(sym);
    func(window);
}

/// Request a buffer from the NativeWindow.
unsafe fn nw_request_buffer(
    window: *mut OHNativeWindow,
) -> (*mut OHNativeWindowBuffer, c_int) {
    type F = unsafe extern "C" fn(
        *mut OHNativeWindow,
        *mut *mut OHNativeWindowBuffer,
        *mut c_int,
    ) -> c_int;
    let sym = nw_sym(b"OH_NativeWindow_NativeWindowRequestBuffer\0");
    if sym.is_null() {
        log::warn!("[NativeWindow] RequestBuffer unavailable");
        return (ptr::null_mut(), -1);
    }
    let func: F = std::mem::transmute(sym);
    let mut buf: *mut OHNativeWindowBuffer = ptr::null_mut();
    let mut fence_fd: c_int = -1;
    let ret = func(window, &mut buf, &mut fence_fd);
    if ret != 0 {
        log::warn!("[NativeWindow] RequestBuffer failed: ret={}", ret);
        return (ptr::null_mut(), fence_fd);
    }
    (buf, fence_fd)
}

/// Flush (present) a buffer.
unsafe fn nw_flush_buffer(
    window: *mut OHNativeWindow,
    buffer: *mut OHNativeWindowBuffer,
    fence_fd: c_int,
) -> c_int {
    type F = unsafe extern "C" fn(
        *mut OHNativeWindow,
        *mut OHNativeWindowBuffer,
        c_int,
        NwRegion,
    ) -> c_int;
    let sym = nw_sym(b"OH_NativeWindow_NativeWindowFlushBuffer\0");
    if sym.is_null() {
        log::warn!("[NativeWindow] FlushBuffer unavailable");
        return -1;
    }
    let func: F = std::mem::transmute(sym);
    let region = NwRegion {
        rect_count: 0,
        rects: ptr::null(),
    };
    func(window, buffer, fence_fd, region)
}

/// Get the [`BufferHandle`] from a NativeWindowBuffer.
unsafe fn nw_get_buffer_handle(buffer: *mut OHNativeWindowBuffer) -> *mut BufferHandle {
    type F = unsafe extern "C" fn(*mut OHNativeWindowBuffer) -> *mut BufferHandle;
    let sym = nw_sym(b"OH_NativeWindow_GetBufferHandleFromNative\0");
    if !sym.is_null() {
        let func: F = std::mem::transmute(sym);
        return func(buffer);
    }
    // Alternative name used on older APIs
    let sym = nw_sym(b"GetBufferHandleFromOHNativeWindowBuffer\0");
    if !sym.is_null() {
        let func: F = std::mem::transmute(sym);
        return func(buffer);
    }
    log::warn!("[NativeWindow] GetBufferHandle unavailable");
    ptr::null_mut()
}

/// Set buffer geometry (width × height).
unsafe fn nw_set_buffer_geometry(window: *mut OHNativeWindow, width: i32, height: i32) {
    type F = unsafe extern "C" fn(*mut OHNativeWindow, c_int, i32, i32) -> c_int;
    let sym = nw_sym(b"OH_NativeWindow_NativeWindowHandleOpt\0");
    if sym.is_null() {
        log::warn!("[NativeWindow] HandleOpt unavailable");
        return;
    }
    // operation code 0 = SET_BUFFER_GEOMETRY
    let func: F = std::mem::transmute(sym);
    let ret = func(window, 0, width, height);
    if ret != 0 {
        log::warn!(
            "[NativeWindow] SET_BUFFER_GEOMETRY failed: ret={}, {}x{}",
            ret, width, height
        );
    } else {
        log::info!("[NativeWindow] SET_BUFFER_GEOMETRY: {}x{}", width, height);
    }
}

/// Set pixel format.
unsafe fn nw_set_format(window: *mut OHNativeWindow, format: c_int) {
    type F = unsafe extern "C" fn(*mut OHNativeWindow, c_int, c_int) -> c_int;
    let sym = nw_sym(b"OH_NativeWindow_NativeWindowHandleOpt\0");
    if sym.is_null() {
        return;
    }
    // operation code 3 = SET_FORMAT
    let func: F = std::mem::transmute(sym);
    let ret = func(window, 3, format);
    if ret != 0 {
        log::warn!("[NativeWindow] SET_FORMAT failed: ret={}, format={}", ret, format);
    } else {
        log::info!("[NativeWindow] SET_FORMAT: {}", format);
    }
}

/// Set buffer usage flags.
unsafe fn nw_set_usage(window: *mut OHNativeWindow, usage: u64) {
    type F = unsafe extern "C" fn(*mut OHNativeWindow, c_int, u64) -> c_int;
    let sym = nw_sym(b"OH_NativeWindow_NativeWindowHandleOpt\0");
    if sym.is_null() {
        return;
    }
    // operation code 5 = SET_USAGE
    let func: F = std::mem::transmute(sym);
    let ret = func(window, 5, usage);
    if ret != 0 {
        log::warn!("[NativeWindow] SET_USAGE failed: ret={}, usage={}", ret, usage);
    } else {
        log::info!("[NativeWindow] SET_USAGE: 0x{:X}", usage);
    }
}

/// Abort (return without presenting) a requested buffer.
unsafe fn nw_abort_buffer(window: *mut OHNativeWindow, buffer: *mut OHNativeWindowBuffer) {
    type F = unsafe extern "C" fn(*mut OHNativeWindow, *mut OHNativeWindowBuffer) -> c_int;
    let sym = nw_sym(b"OH_NativeWindow_NativeWindowAbortBuffer\0");
    if sym.is_null() {
        nw_flush_buffer(window, buffer, -1);
        return;
    }
    let func: F = std::mem::transmute(sym);
    let _ = func(window, buffer);
}

// ============================================================
// OH_NativeBuffer API wrappers (API 12+ preferred path)
// ============================================================

/// Get `OH_NativeBuffer*` from an `OHNativeWindowBuffer*`.
unsafe fn nb_get_from_window_buffer(
    window_buffer: *mut OHNativeWindowBuffer,
) -> *mut OHNativeBuffer {
    type F = unsafe extern "C" fn(*mut OHNativeWindowBuffer) -> *mut OHNativeBuffer;
    let sym = nb_sym(b"OH_NativeWindow_GetNativeBufferFromNativeWindowBuffer\0");
    if sym.is_null() {
        // Try in native_window lib directly
        let sym = nw_sym(b"OH_NativeWindow_GetNativeBufferFromNativeWindowBuffer\0");
        if sym.is_null() {
            return ptr::null_mut();
        }
        let func: F = std::mem::transmute(sym);
        return func(window_buffer);
    }
    let func: F = std::mem::transmute(sym);
    func(window_buffer)
}

/// Map an `OH_NativeBuffer` to get a writable virtual address.
/// Returns null on failure.
unsafe fn nb_map(native_buffer: *mut OHNativeBuffer) -> *mut c_void {
    type F = unsafe extern "C" fn(*mut OHNativeBuffer, *mut *mut c_void) -> c_int;
    let sym = nb_sym(b"OH_NativeBuffer_Map\0");
    if sym.is_null() {
        let sym = nw_sym(b"OH_NativeBuffer_Map\0");
        if sym.is_null() {
            return ptr::null_mut();
        }
        let func: F = std::mem::transmute(sym);
        let mut addr: *mut c_void = ptr::null_mut();
        let ret = func(native_buffer, &mut addr);
        if ret != 0 { return ptr::null_mut(); }
        return addr;
    }
    let func: F = std::mem::transmute(sym);
    let mut addr: *mut c_void = ptr::null_mut();
    let ret = func(native_buffer, &mut addr);
    if ret != 0 {
        return ptr::null_mut();
    }
    addr
}

/// Unmap an `OH_NativeBuffer`.
unsafe fn nb_unmap(native_buffer: *mut OHNativeBuffer) {
    type F = unsafe extern "C" fn(*mut OHNativeBuffer) -> c_int;
    let sym = nb_sym(b"OH_NativeBuffer_Unmap\0");
    if sym.is_null() {
        let sym = nw_sym(b"OH_NativeBuffer_Unmap\0");
        if sym.is_null() { return; }
        let func: F = std::mem::transmute(sym);
        let _ = func(native_buffer);
        return;
    }
    let func: F = std::mem::transmute(sym);
    let _ = func(native_buffer);
}

/// Get buffer configuration (width, height, stride, format) from OH_NativeBuffer.
/// Returns (width, height, stride, size) or zeros on failure.
unsafe fn nb_get_config(native_buffer: *mut OHNativeBuffer) -> (i32, i32, i32, i32) {
    // OH_NativeBuffer_Config struct: { int32_t width, height, format, usage, stride }
    // We read it into a local struct.
    #[repr(C)]
    struct NbConfig {
        width: i32,
        height: i32,
        format: i32,
        usage: i32,
        stride: i32,
    }
    type F = unsafe extern "C" fn(*mut OHNativeBuffer, *mut NbConfig);
    let sym = nb_sym(b"OH_NativeBuffer_GetConfig\0");
    if sym.is_null() {
        let sym = nw_sym(b"OH_NativeBuffer_GetConfig\0");
        if sym.is_null() { return (0, 0, 0, 0); }
        let func: F = std::mem::transmute(sym);
        let mut cfg = NbConfig { width: 0, height: 0, format: 0, usage: 0, stride: 0 };
        func(native_buffer, &mut cfg);
        return (cfg.width, cfg.height, cfg.stride, cfg.width * cfg.height * 4);
    }
    let func: F = std::mem::transmute(sym);
    let mut cfg = NbConfig { width: 0, height: 0, format: 0, usage: 0, stride: 0 };
    func(native_buffer, &mut cfg);
    (cfg.width, cfg.height, cfg.stride, cfg.width * cfg.height * 4)
}

// ============================================================
// Constants
// ============================================================

/// RGBA_8888 (OH_NativeBuffer_Format = 12)
const NW_PIXEL_FMT_RGBA_8888: c_int = 12;

/// Usage: CPU_READ | CPU_WRITE | MEM_DMA
const NW_USAGE_CPU_READ_WRITE_DMA: u64 = 0x03;

// ============================================================
// YuvRenderer
// ============================================================

/// Buffer access method detected at first frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BufferAccessMethod {
    /// Not yet determined
    Unknown,
    /// OH_NativeBuffer_Map (API 12+ recommended)
    NativeBufferMap,
    /// BufferHandle.vir_addr (pre-mapped)
    HandleVirAddr,
    /// mmap(BufferHandle.fd)
    HandleMmap,
}

/// Renders I420 video frames to an OHOS NativeWindow Surface.
pub(crate) struct YuvRenderer {
    window: *mut OHNativeWindow,
    width: u32,
    height: u32,
    frame_count: u32,
    access_method: BufferAccessMethod,
    _buffer_queue: VecDeque<*mut OHNativeWindowBuffer>,
}

// SAFETY: OHOS NativeWindow API is thread-safe for single-owner access.
unsafe impl Send for YuvRenderer {}

impl YuvRenderer {
    pub(crate) fn new() -> Self {
        Self {
            window: ptr::null_mut(),
            width: 0,
            height: 0,
            frame_count: 0,
            access_method: BufferAccessMethod::Unknown,
            _buffer_queue: VecDeque::new(),
        }
    }

    /// Bind to a surface via its string surface ID (from XComponent).
    pub(crate) fn set_surface_by_id(
        &mut self,
        surface_id: &str,
        width: u32,
        height: u32,
    ) -> bool {
        // Destroy previous window if any
        if !self.window.is_null() {
            unsafe { nw_destroy(self.window) };
            self.window = ptr::null_mut();
        }

        let surface_id_num: u64 = match surface_id.parse() {
            Ok(n) => n,
            Err(e) => {
                log::error!(
                    "[YuvRenderer] surfaceId parse failed: '{}', err={}",
                    surface_id,
                    e
                );
                return false;
            }
        };

        let window = unsafe { nw_create_from_surface_id(surface_id_num) };
        if window.is_null() {
            log::error!(
                "[YuvRenderer] NativeWindow creation failed for: {}",
                surface_id
            );
            return false;
        }

        self.window = window;
        self.width = width;
        self.height = height;
        self.frame_count = 0;
        self.access_method = BufferAccessMethod::Unknown;

        // Configure the NativeWindow buffer properties
        unsafe {
            nw_set_buffer_geometry(window, width as i32, height as i32);
            nw_set_format(window, NW_PIXEL_FMT_RGBA_8888);
            nw_set_usage(window, NW_USAGE_CPU_READ_WRITE_DMA);
        }

        log::info!(
            "[YuvRenderer] Surface set OK: {}x{}, RGBA_8888, id={}",
            width, height, surface_id
        );
        true
    }

    /// Render an I420 frame to the bound NativeWindow.
    ///
    /// Converts I420 → RGBA_8888 (BT.601) in CPU and presents the buffer.
    pub(crate) fn render_i420(
        &mut self,
        i420_data: &[u8],
        frame_width: u32,
        frame_height: u32,
        _timestamp_us: i64,
    ) -> bool {
        if self.window.is_null() {
            if self.frame_count == 0 {
                log::warn!("[YuvRenderer] No surface bound (window is null)");
            }
            return false;
        }

        let w = frame_width as usize;
        let h = frame_height as usize;
        let uw = w / 2;
        let uh = h / 2;
        let y_size = w * h;
        let uv_size = uw * uh;

        if i420_data.len() < y_size + 2 * uv_size {
            log::warn!(
                "[YuvRenderer] I420 data too small: {} < {}",
                i420_data.len(),
                y_size + 2 * uv_size
            );
            return false;
        }

        // Update geometry if resolution changed
        if frame_width != self.width || frame_height != self.height {
            log::info!(
                "[YuvRenderer] Resolution changed: {}x{} -> {}x{}",
                self.width, self.height, frame_width, frame_height
            );
            self.width = frame_width;
            self.height = frame_height;
            unsafe {
                nw_set_buffer_geometry(self.window, frame_width as i32, frame_height as i32);
            }
        }

        // Request a buffer from the NativeWindow
        let (buffer, fence_fd) = unsafe { nw_request_buffer(self.window) };
        if buffer.is_null() {
            if self.frame_count == 0 {
                log::warn!("[YuvRenderer] RequestBuffer returned null on first frame");
            }
            return false;
        }

        // Try to get a writable buffer address
        let result = unsafe {
            self.map_and_write_rgba(buffer, i420_data, w, h, uw, uh, y_size, uv_size)
        };

        if !result {
            unsafe { nw_abort_buffer(self.window, buffer) };
            return false;
        }

        // Present the buffer
        let flush_ret = unsafe { nw_flush_buffer(self.window, buffer, fence_fd) };
        if self.frame_count == 0 && flush_ret != 0 {
            log::warn!("[YuvRenderer] FlushBuffer failed on first frame: ret={}", flush_ret);
        }

        self.frame_count += 1;
        if self.frame_count == 1 {
            log::info!(
                "[YuvRenderer] *** First frame rendered OK! ***  method={:?}, {}x{}, flush_ret={}",
                self.access_method, w, h, flush_ret
            );
        } else if self.frame_count % 300 == 0 {
            log::info!(
                "[YuvRenderer] Rendered {} frames ({:?})",
                self.frame_count, self.access_method
            );
        }

        // DEBUG: log first 4 pixel values of every 10th frame to verify data
        if self.frame_count <= 5 || self.frame_count % 10 == 0 {
            // Read back the first few pixels from the I420 data
            let y0 = i420_data[0];
            let y1 = i420_data[1];
            let u0 = i420_data[y_size];
            let v0 = i420_data[y_size + uv_size];
            log::info!(
                "[YuvRenderer] DEBUGBUG frame={}, Y[0]={}, Y[1]={}, U[0]={}, V[0]={}, data_len={}",
                self.frame_count, y0, y1, u0, v0, i420_data.len()
            );
        }
        true
    }

    /// Try to map the buffer and write RGBA data. Returns true on success.
    unsafe fn map_and_write_rgba(
        &mut self,
        buffer: *mut OHNativeWindowBuffer,
        i420_data: &[u8],
        w: usize,
        h: usize,
        uw: usize,
        uh: usize,
        y_size: usize,
        uv_size: usize,
    ) -> bool {
        let min_stride = w * 4;

        // ====== Strategy 1: OH_NativeBuffer_Map (API 12+ preferred) ======
        if self.access_method == BufferAccessMethod::Unknown
            || self.access_method == BufferAccessMethod::NativeBufferMap
        {
            let native_buf = nb_get_from_window_buffer(buffer);
            if !native_buf.is_null() {
                let mapped = nb_map(native_buf);
                if !mapped.is_null() {
                    // Get stride from config
                    let (_cfg_w, _cfg_h, cfg_stride, _cfg_size) = nb_get_config(native_buf);
                    let actual_stride = if cfg_stride > 0 && (cfg_stride as usize) >= min_stride {
                        cfg_stride as usize
                    } else {
                        min_stride
                    };

                    if self.frame_count == 0 {
                        log::info!(
                            "[YuvRenderer] Using NativeBufferMap: {}x{}, cfg_stride={}, actual_stride={}",
                            w, h, cfg_stride, actual_stride
                        );
                    }

                    self.write_i420_to_rgba(
                        mapped as *mut u8,
                        actual_stride,
                        i420_data,
                        w, h, uw, uh, y_size, uv_size,
                    );

                    nb_unmap(native_buf);
                    self.access_method = BufferAccessMethod::NativeBufferMap;
                    return true;
                }
            }
            // NativeBuffer API not available or failed — try next method
            if self.access_method == BufferAccessMethod::NativeBufferMap {
                log::warn!("[YuvRenderer] NativeBufferMap failed after previously working!");
            }
        }

        // ====== Strategy 2 & 3: BufferHandle (legacy) ======
        let handle = nw_get_buffer_handle(buffer);
        if handle.is_null() {
            log::warn!("[YuvRenderer] GetBufferHandle failed, no buffer access available");
            return false;
        }

        let buf_size = (*handle).size as usize;
        let buf_fd = (*handle).fd;

        // Derive actual stride from total buffer size
        let actual_stride = if h > 0 && buf_size >= min_stride * h {
            buf_size / h
        } else {
            min_stride
        };

        let required_size = min_stride * h;
        if buf_size < required_size {
            log::warn!(
                "[YuvRenderer] buffer too small: {} < {} ({}x{}x4)",
                buf_size, required_size, w, h
            );
            return false;
        }

        // Try pre-mapped vir_addr first, then mmap
        let pre_mapped = !(*handle).vir_addr.is_null();
        let vir_addr: *mut u8 = if pre_mapped {
            if self.frame_count == 0 {
                log::info!(
                    "[YuvRenderer] Using HandleVirAddr: {}x{}, buf_size={}, stride={}, actual_stride={}, fd={}",
                    w, h, buf_size, (*handle).stride, actual_stride, buf_fd
                );
            }
            self.access_method = BufferAccessMethod::HandleVirAddr;
            (*handle).vir_addr as *mut u8
        } else {
            // Map the DMA-BUF fd into userspace
            if buf_fd < 0 || buf_size == 0 {
                log::warn!(
                    "[YuvRenderer] Invalid buffer fd={} size={}, cannot map",
                    buf_fd, buf_size
                );
                return false;
            }
            let mapped = mmap(
                ptr::null_mut(),
                buf_size,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                buf_fd,
                0,
            );
            if mapped == (!0usize as *mut c_void) {
                log::warn!(
                    "[YuvRenderer] mmap failed: fd={}, size={}",
                    buf_fd, buf_size
                );
                return false;
            }
            if self.frame_count == 0 {
                log::info!(
                    "[YuvRenderer] Using HandleMmap: {}x{}, buf_size={}, stride={}, actual_stride={}, fd={}",
                    w, h, buf_size, (*handle).stride, actual_stride, buf_fd
                );
            }
            self.access_method = BufferAccessMethod::HandleMmap;
            mapped as *mut u8
        };

        // Write I420 → RGBA
        self.write_i420_to_rgba(
            vir_addr, actual_stride, i420_data, w, h, uw, uh, y_size, uv_size,
        );

        // Unmap if we did mmap
        if !pre_mapped {
            munmap(vir_addr as *mut c_void, buf_size);
        }

        true
    }

    /// Convert I420 to RGBA_8888 and write to the destination buffer.
    #[inline(never)]
    unsafe fn write_i420_to_rgba(
        &self,
        dst: *mut u8,
        dst_stride: usize,
        i420_data: &[u8],
        w: usize,
        h: usize,
        uw: usize,
        _uh: usize,
        y_size: usize,
        uv_size: usize,
    ) {
        let y_plane = i420_data.as_ptr();
        let u_plane = y_plane.add(y_size);
        let v_plane = u_plane.add(uv_size);

        // I420 → RGBA_8888 conversion (BT.601 studio-range)
        for row in 0..h {
            let dst_row = dst.add(row * dst_stride);
            let y_row = row * w;
            let uv_row = (row / 2) * uw;

            for col in 0..w {
                let y_val = *y_plane.add(y_row + col) as i32;
                let u_val = *u_plane.add(uv_row + col / 2) as i32;
                let v_val = *v_plane.add(uv_row + col / 2) as i32;

                let c = y_val - 16;
                let d = u_val - 128;
                let e = v_val - 128;

                let r = ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
                let g = ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
                let b = ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8;

                let px = dst_row.add(col * 4);
                *px = r;
                *px.add(1) = g;
                *px.add(2) = b;
                *px.add(3) = 255; // alpha
            }
        }
    }

    /// Check if a surface is currently bound.
    #[allow(dead_code)]
    pub(crate) fn has_surface(&self) -> bool {
        !self.window.is_null()
    }
}

impl Drop for YuvRenderer {
    fn drop(&mut self) {
        if !self.window.is_null() {
            unsafe { nw_destroy(self.window) };
            self.window = ptr::null_mut();
            log::info!("[YuvRenderer] Destroyed (rendered {} frames)", self.frame_count);
        }
    }
}
