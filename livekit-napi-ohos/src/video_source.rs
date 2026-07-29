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

//! ArkTS-facing wrapper around the libwebrtc `NativeVideoSource` used to push
//! captured raw video frames into a LiveKit room.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex as StdMutex;

use livekit::webrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::{native::NativeVideoSource, VideoResolution};
use napi_derive_ohos::napi;
use napi_ohos::bindgen_prelude::*;

static FRAME_COUNT: AtomicU64 = AtomicU64::new(0);

/// Convert NV21 raw camera frame to I420, writing into a caller-provided
/// output buffer (amortised allocation).
///
/// OHOS camera via ImageReceiver (JPEG format) returns **standard NV21 (Y-VU)**:
///   Y  plane: stride * height bytes   (row-major luminance, offset 0)
///   VU plane: stride * height / 2     (interleaved V,U pairs per 2x2 block,
///                                      offset = stride * height)
///
/// I420 output:
///   Y plane:  width * height bytes
///   U plane:  ((width+1)/2) * ((height+1)/2)
///   V plane:  ((width+1)/2) * ((height+1)/2)
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

    // ── Y plane: copy row by row, de-padding stride ──
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

    // ── VU interleaved → separate U, V planes ──
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

    #[cfg(debug_assertions)]
    {
        let raw_y_samples: [u8; 5] = if src.len() >= 5 {
            src[..5].try_into().unwrap_or([0; 5])
        } else { [0; 5] };
        let out_y_mid = (h / 2) * w;
        let out_y_samples: [u8; 5] = if out_y_mid + 5 <= dst.len() {
            dst[out_y_mid..out_y_mid + 5].try_into().unwrap_or([0; 5])
        } else { [0; 5] };
        log::debug!(
            "[LkVideoSource] NV21→I420: {}x{} s={} src={}B out_Y={:?}",
            w, h, s, src.len(), out_y_samples
        );
    }
}

/// Convert NV21 → rotated I420 in a single pass.
///
/// Performs NV21→I420 conversion, stride de-padding, and rotation
/// in one scan, writing directly into the caller-provided `dst` buffer.
///
/// Supported rotations:
/// - `0`   — no rotation
/// - `90`  — 90° clockwise (used by OHOS back camera)
/// - `270` — 90° counter-clockwise (used by OHOS front camera)
///
/// # Panics
/// Panics if `rotation` is not one of 0, 90, or 270.
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

    assert!(rotation == 90 || rotation == 270, "NV21 rotated only supports 90° or 270° (got {rotation}°)");
    let is_cw90 = rotation == 90;

    // Both 90° and 270° rotations: input W×H → output H×W.
    let out_w = h;
    let out_h = w;
    let half_w = (w + 1) / 2;
    let half_h = (h + 1) / 2;
    let cw = (out_w + 1) / 2;
    let ch = (out_h + 1) / 2;
    let y_size = out_w * out_h;
    let i420_len = y_size + 2 * cw * ch;

    dst.resize(i420_len, 0u8);

    // ── Y plane rotation ──
    for src_y in 0..h {
        for src_x in 0..w {
            let src_off = src_y * s + src_x;
            if src_off < src.len() {
                let (dst_x, dst_y) = if is_cw90 {
                    // 90° CW: src(x,y) → dst(h-1-y, x)
                    (h - 1 - src_y, src_x)
                } else {
                    // 90° CCW (= 270° CW): src(x,y) → dst(y, w-1-x)
                    (src_y, w - 1 - src_x)
                };
                let dst_off = dst_y * out_w + dst_x;
                if dst_off < y_size {
                    dst[dst_off] = src[src_off];
                }
            }
        }
    }

    // ── VU plane: extract and rotate ──
    let vu_base = h * s;
    let vu_src = if vu_base < src.len() { &src[vu_base..] } else { &[] };

    for src_y in 0..half_h {
        for src_x in 0..half_w {
            let vu_off = src_y * s + src_x * 2;
            if vu_off + 1 < vu_src.len() {
                let v = vu_src[vu_off];
                let u = vu_src[vu_off + 1];
                let (dst_x, dst_y) = if is_cw90 {
                    // 90° CW: src(x,y) → dst(half_h-1-y, x)
                    (half_h - 1 - src_y, src_x)
                } else {
                    // 90° CCW: src(x,y) → dst(y, half_w-1-x)
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

    #[cfg(debug_assertions)]
    {
        log::debug!(
            "[LkVideoSource] NV21→I420+rot: {}x{} s={} → {}x{} len={}",
            w, h, s, out_w, out_h, i420_len
        );
    }
}

/// Video source for capturing and sending raw video frames to a LiveKit room.
///
/// Frames are pushed by the application using [`LkVideoSource::capture_frame`]
/// in planar `I420` format. The underlying [`NativeVideoSource`] forwards them
/// to any tracks created from this source.
#[napi]
pub struct LkVideoSource {
    pub(crate) inner: NativeVideoSource,
    /// Pre-allocated I420 buffer reused across frames.
    i420_buf: StdMutex<Vec<u8>>,
}

impl LkVideoSource {
    /// Borrow the underlying [`NativeVideoSource`].
    #[allow(dead_code)]
    pub(crate) fn inner(&self) -> &NativeVideoSource {
        &self.inner
    }
}

#[napi]
impl LkVideoSource {
    /// Create a new video source with the given default resolution.
    ///
    /// `is_screencast` should be `true` when this source represents a
    /// screen-share capture (defaults to `false`).
    #[napi(constructor)]
    pub fn new(width: u32, height: u32, is_screencast: Option<bool>) -> Self {
        let resolution = VideoResolution { width, height };
        Self {
            inner: NativeVideoSource::new(resolution, is_screencast.unwrap_or(false)),
            i420_buf: StdMutex::new(Vec::new()),
        }
    }

    /// Push an `I420`-format video frame.
    ///
    /// `buffer` must contain the three planes concatenated in `Y`, `U`, `V`
    /// order using the default strides for the given `width`/`height`:
    ///
    /// * `Y` plane: `width * height` bytes
    /// * `U` plane: `((width + 1) / 2) * ((height + 1) / 2)` bytes
    /// * `V` plane: `((width + 1) / 2) * ((height + 1) / 2)` bytes
    ///
    /// `timestamp_us` is the capture time in microseconds.
    #[napi]
    pub fn capture_frame(
        &self,
        buffer: Uint8Array,
        width: u32,
        height: u32,
        timestamp_us: i64,
        rotation: Option<u32>,
    ) -> Result<()> {
        let data: &[u8] = buffer.as_ref();

        let mut i420 = I420Buffer::new(width, height);
        let chroma_h = i420.chroma_height();
        let (stride_y, stride_u, stride_v) = i420.strides();

        let y_size = (stride_y as usize) * (height as usize);
        let u_size = (stride_u as usize) * (chroma_h as usize);
        let v_size = (stride_v as usize) * (chroma_h as usize);
        let expected = y_size + u_size + v_size;

        if data.len() != expected {
            return Err(Error::from_reason(format!(
                "video frame buffer size mismatch: got {} bytes, expected {} (y={}, u={}, v={})",
                data.len(), expected, y_size, u_size, v_size,
            )));
        }

        {
            let (dst_y, dst_u, dst_v) = i420.data_mut();
            dst_y.copy_from_slice(&data[..y_size]);
            dst_u.copy_from_slice(&data[y_size..y_size + u_size]);
            dst_v.copy_from_slice(&data[y_size + u_size..y_size + u_size + v_size]);
        }

        let rotation = match rotation.unwrap_or(0) {
            0 => VideoRotation::VideoRotation0,
            90 => VideoRotation::VideoRotation90,
            180 => VideoRotation::VideoRotation180,
            270 => VideoRotation::VideoRotation270,
            other => {
                return Err(Error::from_reason(format!(
                    "invalid rotation {other}, expected 0/90/180/270"
                )))
            }
        };

        let frame =
            VideoFrame { rotation, timestamp_us, frame_metadata: None, buffer: i420 };
        self.inner.capture_frame(&frame);

        let count = FRAME_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if count == 1 {
            log::info!(
                "[LkVideoSource] capture_frame #{}: {}x{} ts={}",
                count, width, height, timestamp_us
            );
        }

        Ok(())
    }

    /// Push a raw NV21 camera frame. Conversion + rotation to I420 happens in
    /// native Rust in a single pass.  The pre-allocated internal I420 buffer
    /// is reused across frames so no per-frame heap allocation occurs.
    ///
    /// `nv21_data` is the raw NV21 byte buffer from OHOS camera `ImageReceiver`.
    /// `stride` is the row stride (may be larger than `width` due to alignment).
    #[napi]
    pub fn capture_frame_nv21(
        &self,
        nv21_data: Uint8Array,
        width: u32,
        height: u32,
        stride: u32,
        timestamp_us: i64,
        rotation: Option<u32>,
    ) -> Result<()> {
        let rot = rotation.unwrap_or(0);

        let (i420_slice_ptr, i420_slice_len, enc_w, enc_h) = {
            let mut buf = self.i420_buf.lock().map_err(|e| {
                Error::from_reason(format!("i420_buf lock poisoned: {e}"))
            })?;
            nv21_to_i420_rotated(
                nv21_data.as_ref(),
                width,
                height,
                stride,
                rot,
                &mut *buf,
            );
            if buf.is_empty() {
                return Err(Error::from_reason("NV21→I420 produced empty buffer"));
            }
            let (ew, eh) = if rot == 0 { (width, height) } else { (height, width) };
            (buf.as_ptr(), buf.len(), ew, eh)
        };

        // SAFETY: i420_slice_ptr and i420_slice_len are derived from i420_buf
        // which is locked exclusively; the slice is valid for the call duration.
        unsafe {
            let i420_slice = std::slice::from_raw_parts(i420_slice_ptr, i420_slice_len);
            self.inner.capture_raw_i420(i420_slice, enc_w, enc_h, timestamp_us);
        }

        let count = FRAME_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if count == 1 {
            log::info!(
                "[LkVideoSource] capture_frame_nv21 #{}: {}x{} ts={}",
                count, width, height, timestamp_us
            );
        }

        Ok(())
    }

    /// Push an RGBA-format video frame. Converts RGBA → I420 via libyuv and
    /// routes into the standard I420 encoding pipeline.  Significantly faster
    /// than ArkTS-side per-pixel RGBA conversion.
    ///
    /// `rgba_data` is the raw RGBA byte buffer (width*height*4 bytes, row-major).
    /// The I420 output is written into the pre-allocated internal buffer.
    #[napi]
    pub fn capture_frame_rgba(
        &self,
        rgba_data: Uint8Array,
        width: u32,
        height: u32,
        timestamp_us: i64,
        _rotation: Option<u32>,
    ) -> Result<()> {
        let rgba: &[u8] = rgba_data.as_ref();
        let w = width as usize;
        let h = height as usize;
        let expected_rgba = w * h * 4;
        if rgba.len() != expected_rgba {
            return Err(Error::from_reason(format!(
                "RGBA buffer size mismatch: got {} expected {} for {}x{}",
                rgba.len(), expected_rgba, w, h
            )));
        }

        let mut buf = self.i420_buf.lock().map_err(|e| {
            Error::from_reason(format!("i420_buf lock poisoned: {e}"))
        })?;

        // RGBA → I420 via libyuv (imgproc crate).
        let cw = (w + 1) / 2;
        let ch = (h + 1) / 2;
        let i420_len = w * h + 2 * cw * ch;
        buf.resize(i420_len, 0u8);

        let (y_slice, rest) = buf.split_at_mut(w * h);
        let (u_slice, v_slice) = rest.split_at_mut(cw * ch);

        // Use the imgproc crate to convert RGBA→I420.
        // NOTE: libyuv names formats by little-endian word order, so memory
        // layout [R,G,B,A] (OHOS RGBA_8888) is libyuv "ABGR". Using
        // rgba_to_i420 here would read alpha as R, turning black into red.
        imgproc::colorcvt::abgr_to_i420(
            rgba,
            (width * 4) as u32,
            y_slice,
            width,
            u_slice,
            (cw as u32),
            v_slice,
            (cw as u32),
            width,
            height,
            false,
        );

        unsafe {
            self.inner.capture_raw_i420(&buf, width, height, timestamp_us);
        }

        let count = FRAME_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if count == 1 {
            log::info!(
                "[LkVideoSource] capture_frame_rgba #{}: {}x{} ts={}",
                count, width, height, timestamp_us
            );
        }

        Ok(())
    }

    /// Configured video width in pixels.
    #[napi(getter)]
    pub fn width(&self) -> u32 {
        self.inner.video_resolution().width
    }

    /// Configured video height in pixels.
    #[napi(getter)]
    pub fn height(&self) -> u32 {
        self.inner.video_resolution().height
    }
}
