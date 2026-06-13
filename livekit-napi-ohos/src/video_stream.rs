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

//! ArkTS-facing wrapper around a remote video track stream.
//!
//! Supports two rendering modes:
//! 1. **PixelMap mode** — call [`LkVideoStream::next_frame`] to receive I420
//!    data in JS, convert to PixelMap, and display via `Image` component.
//! 2. **Surface mode** (recommended) — call [`LkVideoStream::set_surface`]
//!    with an XComponent surface ID, then call
//!    [`LkVideoStream::render_to_surface`] to render decoded frames directly
//!    to the NativeWindow without going through JS pixel conversion.

use std::sync::Arc;

use futures::{FutureExt, StreamExt};
use livekit::webrtc::video_stream::native::NativeVideoStream;
use napi_derive_ohos::napi;
use napi_ohos::bindgen_prelude::*;
use tokio::sync::Mutex;

use crate::native_surface::YuvRenderer;
use crate::track::LkRemoteVideoTrack;

/// Decoded video frame received from a remote participant.
///
/// Pixel data is exposed in I420 layout: the buffer concatenates the Y, U
/// and V planes, with the chroma planes sized at `width/2 * height/2`
/// each (rounded up).
#[napi(object)]
pub struct LkVideoFrame {
    /// Concatenated I420 plane bytes: `[Y .. U .. V]`.
    pub data: Buffer,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Capture timestamp in microseconds.
    pub timestamp_us: i64,
    /// Clockwise rotation applied by the source, in degrees (`0`, `90`,
    /// `180` or `270`).
    pub rotation: u32,
}

/// Stream for receiving decoded video frames from a remote video track.
///
/// Supports both legacy PixelMap rendering (via `nextFrame()`) and high-
/// performance NativeWindow Surface rendering (via `setSurface()` +
/// `renderToSurface()`).
#[napi]
pub struct LkVideoStream {
    stream: Arc<Mutex<Option<NativeVideoStream>>>,
    renderer: Arc<std::sync::Mutex<YuvRenderer>>,
}

#[napi]
impl LkVideoStream {
    /// Placeholder constructor required by napi-ohos. Use
    /// [`Self::from_track`] to obtain a usable instance bound to a remote
    /// track.
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            stream: Arc::new(Mutex::new(None)),
            renderer: Arc::new(std::sync::Mutex::new(YuvRenderer::new())),
        }
    }

    /// Create a video stream from a remote video track.
    #[napi(factory)]
    pub fn from_track(track: &LkRemoteVideoTrack) -> Result<Self> {
        let inner = track
            .inner
            .as_ref()
            .ok_or_else(|| Error::from_reason("track is not initialized"))?;
        let rtc_track = inner.rtc_track();
        log::info!(
            "[LkVideoStream] from_track: track_sid={}, rtc_track_id={}",
            inner.sid(),
            rtc_track.id()
        );
        let native = NativeVideoStream::new(rtc_track);
        Ok(Self {
            stream: Arc::new(Mutex::new(Some(native))),
            renderer: Arc::new(std::sync::Mutex::new(YuvRenderer::new())),
        })
    }

    /// Bind a NativeWindow Surface for direct rendering.
    ///
    /// `surface_id` is the string returned by
    /// `XComponentController.getXComponentSurfaceId()`. `width` and `height`
    /// are the expected frame dimensions (used for initial buffer geometry).
    ///
    /// After calling this, use [`Self::render_to_surface`] to present frames.
    #[napi]
    pub fn set_surface(&self, surface_id: String, width: u32, height: u32) -> bool {
        log::info!(
            "[LkVideoStream] set_surface: id={}, {}x{}",
            surface_id,
            width,
            height
        );
        match self.renderer.lock() {
            Ok(mut renderer) => renderer.set_surface_by_id(&surface_id, width, height),
            Err(e) => {
                log::error!("[LkVideoStream] renderer lock poisoned: {}", e);
                false
            }
        }
    }

    /// Await the next frame and render it directly to the bound Surface.
    ///
    /// Returns:
    /// - `1` if a frame was successfully rendered
    /// - `0` if the stream ended (no more frames — caller should stop the loop)
    /// - `-1` if a frame was received but could not be rendered (no surface
    ///   bound yet, non-I420 format, etc.) — caller should continue looping
    ///
    /// This is the recommended high-performance path — no JS pixel conversion
    /// is involved.
    #[napi]
    pub async fn render_to_surface(&self) -> Result<i32> {
        let mut guard = self.stream.lock().await;
        let stream = match guard.as_mut() {
            Some(s) => s,
            None => return Ok(0), // stream not initialized → ended
        };

        // Wait for at least one frame (blocking)
        let Some(mut frame) = stream.next().await else {
            log::info!("[LkVideoStream] render_to_surface: stream ended");
            return Ok(0);
        };

        // v30 frame-skipping: non-blockingly drain queued frames so we only
        // render the latest one, preventing stale-frame latency buildup.
        let mut drained_count: u32 = 0;
        loop {
            match stream.next().now_or_never() {
                Some(Some(newer_frame)) => {
                    frame = newer_frame;
                    drained_count += 1;
                }
                _ => break, // no more queued frames or stream ended
            }
        }

        if drained_count > 0 {
            log::info!(
                "[LkVideoStream] v30: skipped {} stale frames, rendering latest",
                drained_count
            );
        }

        let width = frame.buffer.width();
        let height = frame.buffer.height();

        let Some(i420) = frame.buffer.as_i420() else {
            log::warn!(
                "[LkVideoStream] render_to_surface: non-I420 frame {}x{}, skipping",
                width,
                height
            );
            return Ok(-1);
        };

        let (y, u, v) = i420.data();
        let mut i420_buf = Vec::with_capacity(y.len() + u.len() + v.len());
        i420_buf.extend_from_slice(y);
        i420_buf.extend_from_slice(u);
        i420_buf.extend_from_slice(v);

        let rendered = match self.renderer.lock() {
            Ok(mut renderer) => {
                renderer.render_i420(&i420_buf, width, height, frame.timestamp_us)
            }
            Err(e) => {
                log::error!("[LkVideoStream] renderer lock poisoned: {}", e);
                false
            }
        };

        if rendered {
            Ok(1)
        } else {
            Ok(-1)
        }
    }

    /// Await the next video frame.
    ///
    /// Returns `null` once the stream has been closed or the underlying
    /// track has ended.
    #[napi]
    pub async fn next_frame(&self) -> Result<Option<LkVideoFrame>> {
        let mut guard = self.stream.lock().await;
        let stream = match guard.as_mut() {
            Some(s) => s,
            None => return Ok(None),
        };

        let Some(frame) = stream.next().await else {
            return Ok(None);
        };

        let width = frame.buffer.width();
        let height = frame.buffer.height();

        // OHOS produces I420 frames directly. If for any reason the buffer
        // is not I420, we fall back to an empty payload rather than fail.
        let bytes = if let Some(i420) = frame.buffer.as_i420() {
            let (y, u, v) = i420.data();
            log::info!(
                "LkVideoStream::next_frame: as_i420=OK, {}x{}, y_len={}, u_len={}, v_len={}, ts_us={}",
                width, height, y.len(), u.len(), v.len(), frame.timestamp_us,
            );
            let mut out = Vec::with_capacity(y.len() + u.len() + v.len());
            out.extend_from_slice(y);
            out.extend_from_slice(u);
            out.extend_from_slice(v);
            out
        } else {
            log::warn!(
                "LkVideoStream::next_frame: as_i420=NONE (dropping frame), {}x{}, ts_us={}",
                width, height, frame.timestamp_us,
            );
            Vec::new()
        };

        Ok(Some(LkVideoFrame {
            data: Buffer::from(bytes),
            width,
            height,
            timestamp_us: frame.timestamp_us,
            rotation: frame.rotation as u32,
        }))
    }

    /// Close the stream and release decoder resources.
    ///
    /// Pending and subsequent calls to [`Self::next_frame`] will resolve
    /// to `null`.
    #[napi]
    pub fn close(&self) {
        let stream = self.stream.clone();
        // Dispatch on the napi-ohos global runtime so this works when called
        // from a synchronous JS context where no tokio runtime is entered.
        napi_ohos::bindgen_prelude::spawn(async move {
            if let Some(mut s) = stream.lock().await.take() {
                s.close();
            }
        });
    }
}
