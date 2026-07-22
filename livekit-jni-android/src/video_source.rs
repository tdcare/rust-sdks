//! Video source JNI wrapper — mirrors `livekit-napi-ohos/src/video_source.rs`.
//!
//! Provides `LkVideoSource` for pushing captured camera frames into a LiveKit
//! room. Frames are received from Android Camera2 as NV21 (YUV_420_888),
//! converted to I420 in Rust, and forwarded to the NativeVideoSource.
//!
//! Kotlin class: `cn.tdcare.smartward.rust.sfu.LkVideoSource`

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex as StdMutex;

use jni::objects::{JByteArray, JClass};
use jni::sys::{jboolean, jint, jlong};
use jni::JNIEnv;

use livekit::webrtc::video_source::{native::NativeVideoSource, VideoResolution};

use crate::{catch_panic, throw_runtime_exception};

static FRAME_COUNT: AtomicU64 = AtomicU64::new(0);

// ============================================================
// NV21 → I420 conversion (ported from OHOS video_source.rs)
// ============================================================

/// Convert NV21 raw camera frame to I420 with optional rotation.
///
/// NV21 layout:
///   Y plane:  stride * height bytes (row-major luminance)
///   VU plane: stride * height/2 bytes (interleaved V,U pairs)
///
/// I420 output:
///   Y plane: width * height
///   U plane: ((width+1)/2) * ((height+1)/2)
///   V plane: ((width+1)/2) * ((height+1)/2)
fn nv21_to_i420_rotated(
    src: &[u8],
    width: u32,
    height: u32,
    stride: u32,
    rotation: u32,
    dst: &mut Vec<u8>,
) {
    let w = width as usize;
    let h = height as usize;
    let s = stride as usize;

    if rotation == 0 {
        nv21_to_i420_into(src, width, height, stride, dst);
        return;
    }

    // Support 90° and 270° rotation
    let is_cw90 = rotation == 90;
    let out_w = h;
    let out_h = w;
    let half_w = (w + 1) / 2;
    let half_h = (h + 1) / 2;
    let cw = (out_w + 1) / 2;
    let ch = (out_h + 1) / 2;
    let y_size = out_w * out_h;
    let i420_len = y_size + 2 * cw * ch;

    dst.resize(i420_len, 0u8);

    // Y plane rotation
    for src_y in 0..h {
        for src_x in 0..w {
            let src_off = src_y * s + src_x;
            if src_off < src.len() {
                let (dst_x, dst_y) = if is_cw90 {
                    (h - 1 - src_y, src_x)
                } else {
                    (src_y, w - 1 - src_x)
                };
                let dst_off = dst_y * out_w + dst_x;
                if dst_off < y_size {
                    dst[dst_off] = src[src_off];
                }
            }
        }
    }

    // VU plane: extract and rotate
    let vu_base = h * s;
    let vu_src = if vu_base < src.len() { &src[vu_base..] } else { &[] };

    for src_y in 0..half_h {
        for src_x in 0..half_w {
            let vu_off = src_y * s + src_x * 2;
            if vu_off + 1 < vu_src.len() {
                let v = vu_src[vu_off];
                let u = vu_src[vu_off + 1];
                let (dst_x, dst_y) = if is_cw90 {
                    (half_h - 1 - src_y, src_x)
                } else {
                    (src_y, half_w - 1 - src_x)
                };
                let u_dst = y_size + dst_y * cw + dst_x;
                let v_dst = y_size + cw * ch + dst_y * cw + dst_x;
                if u_dst < dst.len() {
                    dst[u_dst] = u;
                }
                if v_dst < dst.len() {
                    dst[v_dst] = v;
                }
            }
        }
    }
}

/// NV21 → I420 without rotation.
fn nv21_to_i420_into(src: &[u8], width: u32, height: u32, stride: u32, dst: &mut Vec<u8>) {
    let w = width as usize;
    let h = height as usize;
    let s = stride as usize;
    let cw = (w + 1) / 2;
    let ch = (h + 1) / 2;
    let y_size = w * h;
    let i420_len = y_size + 2 * cw * ch;

    dst.resize(i420_len, 0u8);

    let vu_base = h * s;
    let vu_src = if vu_base < src.len() { &src[vu_base..] } else { &[] };

    // Y plane: copy row by row, de-padding stride
    if s == w {
        let copy_len = y_size.min(src.len());
        dst[..copy_len].copy_from_slice(&src[..copy_len]);
    } else {
        for row in 0..h {
            let src_off = row * s;
            let dst_off = row * w;
            if src_off + w <= src.len() {
                dst[dst_off..dst_off + w].copy_from_slice(&src[src_off..src_off + w]);
            }
        }
    }

    // VU interleaved → separate U, V planes
    for row in 0..(h / 2) {
        for col in 0..(w / 2) {
            let vu_off = row * s + col * 2;
            if vu_off + 1 < vu_src.len() {
                let v = vu_src[vu_off];
                let u = vu_src[vu_off + 1];
                let uv_dst = y_size + row * cw + col;
                if uv_dst < dst.len() {
                    dst[uv_dst] = u;
                }
                let v_dst = y_size + cw * ch + row * cw + col;
                if v_dst < dst.len() {
                    dst[v_dst] = v;
                }
            }
        }
    }
}

// ============================================================
// VideoSourceState — internal state stored as opaque pointer
// ============================================================

pub struct VideoSourceState {
    pub(crate) source: NativeVideoSource,
    i420_buf: StdMutex<Vec<u8>>,
}

// ============================================================
// JNI Methods — cn.tdcare.smartward.rust.sfu.LkVideoSource
// ============================================================

/// Create a new video source.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkVideoSource_nativeNew(
    mut env: JNIEnv,
    _class: JClass,
    width: jint,
    height: jint,
    is_screencast: jboolean,
) -> jlong {
    catch_panic(0i64, || {
        let resolution = VideoResolution {
            width: width as u32,
            height: height as u32,
        };
        let source = NativeVideoSource::new(resolution, is_screencast != 0);
        let state = Box::new(VideoSourceState {
            source,
            i420_buf: StdMutex::new(Vec::new()),
        });
        let ptr = Box::into_raw(state) as jlong;
        log::info!("[JNI VideoSource] nativeNew: {}x{}, ptr={}", width, height, ptr);
        ptr
    })
}

/// Push an I420-format video frame directly.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkVideoSource_nativeCaptureFrameI420(
    mut env: JNIEnv,
    _class: JClass,
    ptr: jlong,
    buffer: JByteArray,
    width: jint,
    height: jint,
    timestamp_us: jlong,
    rotation: jint,
) {
    if ptr == 0 {
        throw_runtime_exception(&mut env, "VideoSource not initialized");
        return;
    }
    // Extract byte array before catch_panic to avoid borrow conflict
    let data = match env.convert_byte_array(&buffer) {
        Ok(d) => d,
        Err(e) => {
            throw_runtime_exception(&mut env, &format!("failed to get byte array: {}", e));
            return;
        }
    };

    catch_panic((), || {
        let state = unsafe { &*(ptr as *const VideoSourceState) };

        let w = width as u32;
        let h = height as u32;

        state.source.capture_raw_i420(&data, w, h, timestamp_us);

        let count = FRAME_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if count % 100 == 1 {
            log::info!("[JNI VideoSource] captureFrameI420 #{}: {}x{}", count, w, h);
        }
    });
}

/// Push an NV21-format camera frame. Conversion to I420 happens in Rust.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkVideoSource_nativeCaptureFrameNv21(
    mut env: JNIEnv,
    _class: JClass,
    ptr: jlong,
    nv21_data: JByteArray,
    width: jint,
    height: jint,
    stride: jint,
    timestamp_us: jlong,
    rotation: jint,
) {
    if ptr == 0 {
        throw_runtime_exception(&mut env, "VideoSource not initialized");
        return;
    }
    // Extract byte array before catch_panic to avoid borrow conflict
    let data = match env.convert_byte_array(&nv21_data) {
        Ok(d) => d,
        Err(e) => {
            throw_runtime_exception(&mut env, &format!("failed to get byte array: {}", e));
            return;
        }
    };

    catch_panic((), || {
        let state = unsafe { &*(ptr as *const VideoSourceState) };

        let w = width as u32;
        let h = height as u32;
        let s = stride as u32;
        let rot = rotation as u32;

        // Convert NV21 → I420 (with rotation) using pre-allocated buffer
        let mut buf = match state.i420_buf.lock() {
            Ok(b) => b,
            Err(_) => {
                log::error!("[JNI VideoSource] i420_buf lock poisoned");
                return;
            }
        };

        nv21_to_i420_rotated(&data, w, h, s, rot, &mut buf);

        if buf.is_empty() {
            log::error!("[JNI VideoSource] NV21→I420 produced empty buffer");
            return;
        }

        let (enc_w, enc_h) = if rot == 0 || rot == 180 {
            (w, h)
        } else {
            (h, w) // 90° or 270° swaps dimensions
        };

        state.source.capture_raw_i420(&buf, enc_w, enc_h, timestamp_us);

        let count = FRAME_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if count % 100 == 1 {
            log::info!(
                "[JNI VideoSource] captureFrameNv21 #{}: {}x{} s={} rot={} → {}x{}",
                count, w, h, s, rot, enc_w, enc_h
            );
        }
    });
}

/// Get configured video width.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkVideoSource_nativeGetWidth(
    mut env: JNIEnv,
    _class: JClass,
    ptr: jlong,
) -> jint {
    catch_panic(0, || {
        if ptr == 0 {
            return 0;
        }
        let state = unsafe { &*(ptr as *const VideoSourceState) };
        state.source.video_resolution().width as jint
    })
}

/// Get configured video height.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkVideoSource_nativeGetHeight(
    mut env: JNIEnv,
    _class: JClass,
    ptr: jlong,
) -> jint {
    catch_panic(0, || {
        if ptr == 0 {
            return 0;
        }
        let state = unsafe { &*(ptr as *const VideoSourceState) };
        state.source.video_resolution().height as jint
    })
}

/// Destroy the video source and free resources.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkVideoSource_nativeDestroy(
    mut env: JNIEnv,
    _class: JClass,
    ptr: jlong,
) {
    catch_panic((), || {
        if ptr == 0 {
            return;
        }
        unsafe {
            let _ = Box::from_raw(ptr as *mut VideoSourceState);
        }
        log::info!("[JNI VideoSource] destroyed: ptr={}", ptr);
    });
}
