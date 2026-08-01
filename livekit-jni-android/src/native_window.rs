//! Android ANativeWindow Surface renderer.
//!
//! Mirrors OHOS `native_surface.rs` but uses Android NDK ANativeWindow APIs.
//! Renders I420 video frames to an Android Surface by converting to RGBA
//! and writing via ANativeWindow_lock/unlockAndPost.

use std::ffi::c_void;
use std::os::raw::c_int;
use std::ptr;

// ============================================================
// Android NDK FFI bindings (manual, avoids extra crate deps)
// ============================================================

/// Opaque ANativeWindow handle.
#[repr(C)]
pub struct ANativeWindow {
    _private: [u8; 0],
}

/// ANativeWindow_Buffer — the locked surface buffer.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ANativeWindow_Buffer {
    pub width: i32,
    pub height: i32,
    pub stride: i32,
    pub format: i32,
    pub bits: *mut c_void,
    pub reserved: [u32; 6],
}

/// ARect for specifying lock region.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ARect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

// Android pixel format constants
const WINDOW_FORMAT_RGBA_8888: c_int = 1;

#[link(name = "android")]
extern "C" {
    fn ANativeWindow_fromSurface(
        env: *mut c_void,
        surface: *mut c_void,
    ) -> *mut ANativeWindow;

    fn ANativeWindow_release(window: *mut ANativeWindow);

    fn ANativeWindow_setBuffersGeometry(
        window: *mut ANativeWindow,
        width: c_int,
        height: c_int,
        format: c_int,
    ) -> c_int;

    fn ANativeWindow_lock(
        window: *mut ANativeWindow,
        buffer: *mut ANativeWindow_Buffer,
        inOutDirtyBounds: *mut ARect,
    ) -> c_int;

    fn ANativeWindow_unlockAndPost(window: *mut ANativeWindow) -> c_int;
}

// ============================================================
// I420 → RGBA8888 conversion
// ============================================================

/// Convert I420 planar data to RGBA8888 (CPU, no NEON optimization).
///
/// Input: concatenated Y + U + V planes
/// Output: RGBA pixels (4 bytes per pixel, row-major)
fn i420_to_rgba(i420: &[u8], width: u32, height: u32, rgba: &mut [u8]) {
    let w = width as usize;
    let h = height as usize;
    let y_size = w * h;
    let uv_w = (w + 1) / 2;
    let uv_h = (h + 1) / 2;
    let uv_size = uv_w * uv_h;

    if i420.len() < y_size + 2 * uv_size {
        return;
    }

    let y_plane = &i420[..y_size];
    let u_plane = &i420[y_size..y_size + uv_size];
    let v_plane = &i420[y_size + uv_size..y_size + 2 * uv_size];

    for row in 0..h {
        for col in 0..w {
            let y_idx = row * w + col;
            let uv_idx = (row / 2) * uv_w + (col / 2);

            let y_val = y_plane[y_idx] as i32;
            let u_val = u_plane[uv_idx] as i32 - 128;
            let v_val = v_plane[uv_idx] as i32 - 128;

            // BT.601 YUV→RGB conversion
            let mut r = y_val + ((351 * v_val) >> 8);
            let mut g = y_val - ((179 * v_val + 86 * u_val) >> 8);
            let mut b = y_val + ((443 * u_val) >> 8);

            // Clamp to [0, 255]
            r = r.clamp(0, 255);
            g = g.clamp(0, 255);
            b = b.clamp(0, 255);

            let px_idx = y_idx * 4;
            if px_idx + 3 < rgba.len() {
                rgba[px_idx] = r as u8;
                rgba[px_idx + 1] = g as u8;
                rgba[px_idx + 2] = b as u8;
                rgba[px_idx + 3] = 255;
            }
        }
    }
}

// ============================================================
// AndroidSurfaceRenderer
// ============================================================

/// Renders I420 video frames to an Android Surface via ANativeWindow.
pub struct AndroidSurfaceRenderer {
    window: Option<*mut ANativeWindow>,
    width: u32,
    height: u32,
    rgba_buf: Vec<u8>,
}

// ANativeWindow pointer is Send across threads (single-owner pattern)
unsafe impl Send for AndroidSurfaceRenderer {}

impl AndroidSurfaceRenderer {
    pub fn new() -> Self {
        Self {
            window: None,
            width: 0,
            height: 0,
            rgba_buf: Vec::new(),
        }
    }

    /// Bind an Android Surface for rendering.
    ///
    /// # Safety
    /// `env_ptr` must be a valid JNIEnv pointer, `surface_ptr` must be a valid
    /// jobject referencing an android.view.Surface.
    pub unsafe fn set_surface(
        &mut self,
        env_ptr: *mut c_void,
        surface_ptr: *mut c_void,
        width: u32,
        height: u32,
    ) -> bool {
        // Release previous window
        self.release_window();

        if surface_ptr.is_null() {
            log::warn!("[SurfaceRenderer] set_surface: null surface");
            return false;
        }

        let window = ANativeWindow_fromSurface(env_ptr, surface_ptr);
        if window.is_null() {
            log::error!("[SurfaceRenderer] ANativeWindow_fromSurface returned null");
            return false;
        }

        // Force RGBA_8888 buffer format
        let ret = ANativeWindow_setBuffersGeometry(
            window,
            width as c_int,
            height as c_int,
            WINDOW_FORMAT_RGBA_8888,
        );
        if ret != 0 {
            log::warn!(
                "[SurfaceRenderer] setBuffersGeometry failed: {} ({}x{})",
                ret, width, height
            );
        }

        self.window = Some(window);
        self.width = width;
        self.height = height;
        self.rgba_buf.resize((width * height * 4) as usize, 0);

        log::info!(
            "[SurfaceRenderer] set_surface OK: {}x{}, window={:?}",
            width, height, window
        );
        true
    }

    /// Render an I420 frame to the bound Surface.
    ///
    /// Returns true if the frame was successfully rendered.
    pub fn render_i420(&mut self, i420_data: &[u8], width: u32, height: u32) -> bool {
        let window = match self.window {
            Some(w) => w,
            None => {
                log::warn!("[SurfaceRenderer] render_i420: no surface bound");
                return false;
            }
        };

        // Ensure RGBA buffer is large enough
        let rgba_size = (width * height * 4) as usize;
        if self.rgba_buf.len() < rgba_size {
            self.rgba_buf.resize(rgba_size, 0);
        }

        // Convert I420 → RGBA
        i420_to_rgba(i420_data, width, height, &mut self.rgba_buf[..rgba_size]);

        // Lock the surface buffer
        let mut buffer: ANativeWindow_Buffer = unsafe { std::mem::zeroed() };
        let lock_ret = unsafe { ANativeWindow_lock(window, &mut buffer, ptr::null_mut()) };
        if lock_ret != 0 {
            log::warn!("[SurfaceRenderer] ANativeWindow_lock failed: {}", lock_ret);
            return false;
        }

        // Copy RGBA data to the locked buffer
        let buf_width = buffer.width as usize;
        let buf_height = buffer.height as usize;
        let buf_stride = buffer.stride as usize;

        if buffer.bits.is_null() {
            log::warn!("[SurfaceRenderer] locked buffer has null bits");
            unsafe { ANativeWindow_unlockAndPost(window) };
            return false;
        }

        // Handle stride != width (padding)
        let copy_w = buf_width.min(width as usize);
        let copy_h = buf_height.min(height as usize);

        unsafe {
            let dst = buffer.bits as *mut u8;
            for row in 0..copy_h {
                let src_off = row * (width as usize) * 4;
                let dst_off = row * buf_stride * 4;
                let copy_bytes = copy_w * 4;
                if src_off + copy_bytes <= self.rgba_buf.len() {
                    ptr::copy_nonoverlapping(
                        self.rgba_buf.as_ptr().add(src_off),
                        dst.add(dst_off),
                        copy_bytes,
                    );
                }
            }
        }

        // Unlock and post
        let post_ret = unsafe { ANativeWindow_unlockAndPost(window) };
        if post_ret != 0 {
            log::warn!("[SurfaceRenderer] unlockAndPost failed: {}", post_ret);
            return false;
        }

        true
    }

    /// Whether a surface is currently bound.
    pub fn has_surface(&self) -> bool {
        self.window.is_some()
    }

    /// Release the ANativeWindow reference.
    pub fn release_window(&mut self) {
        if let Some(window) = self.window.take() {
            unsafe {
                ANativeWindow_release(window);
            }
            log::info!("[SurfaceRenderer] window released");
        }
    }
}

impl Drop for AndroidSurfaceRenderer {
    fn drop(&mut self) {
        self.release_window();
    }
}
