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

//! OHOS pure-Rust video frame buffers.
//!
//! These types own their pixel data on the heap (`Vec<u8>` / `Vec<u16>`) and
//! provide the same surface as the C++-backed `native` buffers consumed by the
//! `libwebrtc::video_frame` macros. Only [`I420Buffer`] and [`NV12Buffer`] are
//! exercised end-to-end by the OHOS encoder pipeline; the remaining buffer
//! types are stubs that allocate zero-filled storage.
//!
//! All constructors expect strides in **pixels** (matching libwebrtc), not
//! bytes. For 10-bit buffers the underlying storage is `u16` so byte-stride is
//! `stride * 2`.

use crate::video_frame::VideoFormatType;

// -------------------------------------------------------------------------
// Helpers
// -------------------------------------------------------------------------

#[inline]
fn chroma_dim(dim: u32) -> u32 {
    (dim + 1) / 2
}

// -------------------------------------------------------------------------
// I420
// -------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct I420Buffer {
    width: u32,
    height: u32,
    stride_y: u32,
    stride_u: u32,
    stride_v: u32,
    data_y: Vec<u8>,
    data_u: Vec<u8>,
    data_v: Vec<u8>,
}

impl I420Buffer {
    pub fn new(
        width: u32,
        height: u32,
        stride_y: u32,
        stride_u: u32,
        stride_v: u32,
    ) -> crate::video_frame::I420Buffer {
        let chroma_h = chroma_dim(height) as usize;
        let height_us = height as usize;
        let buf = Self {
            width,
            height,
            stride_y,
            stride_u,
            stride_v,
            data_y: vec![0u8; stride_y as usize * height_us],
            data_u: vec![0u8; stride_u as usize * chroma_h],
            data_v: vec![0u8; stride_v as usize * chroma_h],
        };
        crate::video_frame::I420Buffer { handle: buf }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn chroma_width(&self) -> u32 {
        chroma_dim(self.width)
    }

    pub fn chroma_height(&self) -> u32 {
        chroma_dim(self.height)
    }

    pub fn stride_y(&self) -> u32 {
        self.stride_y
    }

    pub fn stride_u(&self) -> u32 {
        self.stride_u
    }

    pub fn stride_v(&self) -> u32 {
        self.stride_v
    }

    pub fn data(&self) -> (&[u8], &[u8], &[u8]) {
        (&self.data_y, &self.data_u, &self.data_v)
    }

    pub fn to_i420(&self) -> Self {
        self.clone()
    }

    pub fn to_argb(
        &self,
        _format: VideoFormatType,
        _dst: &mut [u8],
        _dst_stride: u32,
        _dst_width: i32,
        _dst_height: i32,
    ) {
        // TODO(OHOS): Wire up libyuv-style conversion when the OHOS pipeline
        // needs to render frames outside the encoder.
    }

    pub fn scale(&mut self, scaled_width: i32, scaled_height: i32) -> crate::video_frame::I420Buffer {
        // OHOS doesn't ship a scaler yet; return a freshly allocated buffer of
        // the requested size with default strides. Callers that depend on the
        // pixel data (e.g. simulcast scaling) need libyuv integration.
        Self::new(
            scaled_width as u32,
            scaled_height as u32,
            scaled_width as u32,
            chroma_dim(scaled_width as u32),
            chroma_dim(scaled_width as u32),
        )
    }
}

// -------------------------------------------------------------------------
// I420A (I420 + Alpha)
// -------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct I420ABuffer {
    width: u32,
    height: u32,
    stride_y: u32,
    stride_u: u32,
    stride_v: u32,
    stride_a: u32,
    data_y: Vec<u8>,
    data_u: Vec<u8>,
    data_v: Vec<u8>,
    data_a: Option<Vec<u8>>,
}

impl I420ABuffer {
    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn chroma_width(&self) -> u32 {
        chroma_dim(self.width)
    }

    pub fn chroma_height(&self) -> u32 {
        chroma_dim(self.height)
    }

    pub fn stride_y(&self) -> u32 {
        self.stride_y
    }

    pub fn stride_u(&self) -> u32 {
        self.stride_u
    }

    pub fn stride_v(&self) -> u32 {
        self.stride_v
    }

    pub fn stride_a(&self) -> u32 {
        self.stride_a
    }

    pub fn data(&self) -> (&[u8], &[u8], &[u8], Option<&[u8]>) {
        (&self.data_y, &self.data_u, &self.data_v, self.data_a.as_deref())
    }

    pub fn to_i420(&self) -> I420Buffer {
        I420Buffer {
            width: self.width,
            height: self.height,
            stride_y: self.stride_y,
            stride_u: self.stride_u,
            stride_v: self.stride_v,
            data_y: self.data_y.clone(),
            data_u: self.data_u.clone(),
            data_v: self.data_v.clone(),
        }
    }

    pub fn to_argb(
        &self,
        _format: VideoFormatType,
        _dst: &mut [u8],
        _dst_stride: u32,
        _dst_width: i32,
        _dst_height: i32,
    ) {
        // TODO(OHOS): implement when needed.
    }

    pub fn scale(&mut self, scaled_width: i32, scaled_height: i32) -> crate::video_frame::I420ABuffer {
        let chroma_h = chroma_dim(scaled_height as u32) as usize;
        let h = scaled_height as usize;
        let w = scaled_width as u32;
        crate::video_frame::I420ABuffer {
            handle: I420ABuffer {
                width: w,
                height: scaled_height as u32,
                stride_y: w,
                stride_u: chroma_dim(w),
                stride_v: chroma_dim(w),
                stride_a: w,
                data_y: vec![0u8; w as usize * h],
                data_u: vec![0u8; chroma_dim(w) as usize * chroma_h],
                data_v: vec![0u8; chroma_dim(w) as usize * chroma_h],
                data_a: None,
            },
        }
    }
}

// -------------------------------------------------------------------------
// I422 (4:2:2 - chroma height = height)
// -------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct I422Buffer {
    width: u32,
    height: u32,
    stride_y: u32,
    stride_u: u32,
    stride_v: u32,
    data_y: Vec<u8>,
    data_u: Vec<u8>,
    data_v: Vec<u8>,
}

impl I422Buffer {
    pub fn new(
        width: u32,
        height: u32,
        stride_y: u32,
        stride_u: u32,
        stride_v: u32,
    ) -> crate::video_frame::I422Buffer {
        let h = height as usize;
        let buf = Self {
            width,
            height,
            stride_y,
            stride_u,
            stride_v,
            data_y: vec![0u8; stride_y as usize * h],
            data_u: vec![0u8; stride_u as usize * h],
            data_v: vec![0u8; stride_v as usize * h],
        };
        crate::video_frame::I422Buffer { handle: buf }
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn chroma_width(&self) -> u32 {
        chroma_dim(self.width)
    }
    pub fn chroma_height(&self) -> u32 {
        self.height
    }
    pub fn stride_y(&self) -> u32 {
        self.stride_y
    }
    pub fn stride_u(&self) -> u32 {
        self.stride_u
    }
    pub fn stride_v(&self) -> u32 {
        self.stride_v
    }
    pub fn data(&self) -> (&[u8], &[u8], &[u8]) {
        (&self.data_y, &self.data_u, &self.data_v)
    }

    pub fn to_i420(&self) -> I420Buffer {
        // TODO(OHOS): proper chroma decimation; for now copy luma and zero chroma
        I420Buffer {
            width: self.width,
            height: self.height,
            stride_y: self.stride_y,
            stride_u: chroma_dim(self.width),
            stride_v: chroma_dim(self.width),
            data_y: self.data_y.clone(),
            data_u: vec![0u8; chroma_dim(self.width) as usize * chroma_dim(self.height) as usize],
            data_v: vec![0u8; chroma_dim(self.width) as usize * chroma_dim(self.height) as usize],
        }
    }

    pub fn to_argb(
        &self,
        _format: VideoFormatType,
        _dst: &mut [u8],
        _dst_stride: u32,
        _dst_width: i32,
        _dst_height: i32,
    ) {
        // TODO(OHOS): implement.
    }

    pub fn scale(&mut self, scaled_width: i32, scaled_height: i32) -> crate::video_frame::I422Buffer {
        Self::new(
            scaled_width as u32,
            scaled_height as u32,
            scaled_width as u32,
            chroma_dim(scaled_width as u32),
            chroma_dim(scaled_width as u32),
        )
    }
}

// -------------------------------------------------------------------------
// I444 (4:4:4)
// -------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct I444Buffer {
    width: u32,
    height: u32,
    stride_y: u32,
    stride_u: u32,
    stride_v: u32,
    data_y: Vec<u8>,
    data_u: Vec<u8>,
    data_v: Vec<u8>,
}

impl I444Buffer {
    pub fn new(
        width: u32,
        height: u32,
        stride_y: u32,
        stride_u: u32,
        stride_v: u32,
    ) -> crate::video_frame::I444Buffer {
        let h = height as usize;
        let buf = Self {
            width,
            height,
            stride_y,
            stride_u,
            stride_v,
            data_y: vec![0u8; stride_y as usize * h],
            data_u: vec![0u8; stride_u as usize * h],
            data_v: vec![0u8; stride_v as usize * h],
        };
        crate::video_frame::I444Buffer { handle: buf }
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn chroma_width(&self) -> u32 {
        self.width
    }
    pub fn chroma_height(&self) -> u32 {
        self.height
    }
    pub fn stride_y(&self) -> u32 {
        self.stride_y
    }
    pub fn stride_u(&self) -> u32 {
        self.stride_u
    }
    pub fn stride_v(&self) -> u32 {
        self.stride_v
    }
    pub fn data(&self) -> (&[u8], &[u8], &[u8]) {
        (&self.data_y, &self.data_u, &self.data_v)
    }

    pub fn to_i420(&self) -> I420Buffer {
        I420Buffer {
            width: self.width,
            height: self.height,
            stride_y: self.stride_y,
            stride_u: chroma_dim(self.width),
            stride_v: chroma_dim(self.width),
            data_y: self.data_y.clone(),
            data_u: vec![0u8; chroma_dim(self.width) as usize * chroma_dim(self.height) as usize],
            data_v: vec![0u8; chroma_dim(self.width) as usize * chroma_dim(self.height) as usize],
        }
    }

    pub fn to_argb(
        &self,
        _format: VideoFormatType,
        _dst: &mut [u8],
        _dst_stride: u32,
        _dst_width: i32,
        _dst_height: i32,
    ) {
        // TODO(OHOS): implement.
    }

    pub fn scale(&mut self, scaled_width: i32, scaled_height: i32) -> crate::video_frame::I444Buffer {
        Self::new(
            scaled_width as u32,
            scaled_height as u32,
            scaled_width as u32,
            scaled_width as u32,
            scaled_width as u32,
        )
    }
}

// -------------------------------------------------------------------------
// I010 (10-bit YUV 4:2:0)
// -------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct I010Buffer {
    width: u32,
    height: u32,
    stride_y: u32,
    stride_u: u32,
    stride_v: u32,
    data_y: Vec<u16>,
    data_u: Vec<u16>,
    data_v: Vec<u16>,
}

impl I010Buffer {
    pub fn new(
        width: u32,
        height: u32,
        stride_y: u32,
        stride_u: u32,
        stride_v: u32,
    ) -> crate::video_frame::I010Buffer {
        let chroma_h = chroma_dim(height) as usize;
        let h = height as usize;
        let buf = Self {
            width,
            height,
            stride_y,
            stride_u,
            stride_v,
            data_y: vec![0u16; stride_y as usize * h],
            data_u: vec![0u16; stride_u as usize * chroma_h],
            data_v: vec![0u16; stride_v as usize * chroma_h],
        };
        crate::video_frame::I010Buffer { handle: buf }
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn chroma_width(&self) -> u32 {
        chroma_dim(self.width)
    }
    pub fn chroma_height(&self) -> u32 {
        chroma_dim(self.height)
    }
    pub fn stride_y(&self) -> u32 {
        self.stride_y
    }
    pub fn stride_u(&self) -> u32 {
        self.stride_u
    }
    pub fn stride_v(&self) -> u32 {
        self.stride_v
    }
    pub fn data(&self) -> (&[u16], &[u16], &[u16]) {
        (&self.data_y, &self.data_u, &self.data_v)
    }

    pub fn to_i420(&self) -> I420Buffer {
        // 10-bit -> 8-bit drops the low 2 bits.
        let down = |v: &[u16]| -> Vec<u8> { v.iter().map(|&p| (p >> 2) as u8).collect() };
        I420Buffer {
            width: self.width,
            height: self.height,
            stride_y: self.stride_y,
            stride_u: self.stride_u,
            stride_v: self.stride_v,
            data_y: down(&self.data_y),
            data_u: down(&self.data_u),
            data_v: down(&self.data_v),
        }
    }

    pub fn to_argb(
        &self,
        _format: VideoFormatType,
        _dst: &mut [u8],
        _dst_stride: u32,
        _dst_width: i32,
        _dst_height: i32,
    ) {
        // TODO(OHOS): implement.
    }

    pub fn scale(&mut self, scaled_width: i32, scaled_height: i32) -> crate::video_frame::I010Buffer {
        Self::new(
            scaled_width as u32,
            scaled_height as u32,
            scaled_width as u32,
            chroma_dim(scaled_width as u32),
            chroma_dim(scaled_width as u32),
        )
    }
}

// -------------------------------------------------------------------------
// NV12 (semi-planar Y + interleaved UV)
// -------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct NV12Buffer {
    width: u32,
    height: u32,
    stride_y: u32,
    stride_uv: u32,
    data_y: Vec<u8>,
    data_uv: Vec<u8>,
}

impl NV12Buffer {
    pub fn new(
        width: u32,
        height: u32,
        stride_y: u32,
        stride_uv: u32,
    ) -> crate::video_frame::NV12Buffer {
        let chroma_h = chroma_dim(height) as usize;
        let h = height as usize;
        let buf = Self {
            width,
            height,
            stride_y,
            stride_uv,
            data_y: vec![0u8; stride_y as usize * h],
            data_uv: vec![0u8; stride_uv as usize * chroma_h],
        };
        crate::video_frame::NV12Buffer { handle: buf }
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn chroma_width(&self) -> u32 {
        chroma_dim(self.width)
    }
    pub fn chroma_height(&self) -> u32 {
        chroma_dim(self.height)
    }
    pub fn stride_y(&self) -> u32 {
        self.stride_y
    }
    pub fn stride_uv(&self) -> u32 {
        self.stride_uv
    }

    pub fn data(&self) -> (&[u8], &[u8]) {
        (&self.data_y, &self.data_uv)
    }

    pub fn to_i420(&self) -> I420Buffer {
        let cw = chroma_dim(self.width) as usize;
        let ch = chroma_dim(self.height) as usize;
        let mut data_u = vec![0u8; cw * ch];
        let mut data_v = vec![0u8; cw * ch];
        for row in 0..ch {
            let row_start = row * self.stride_uv as usize;
            for col in 0..cw {
                let idx = row_start + col * 2;
                if idx + 1 < self.data_uv.len() {
                    data_u[row * cw + col] = self.data_uv[idx];
                    data_v[row * cw + col] = self.data_uv[idx + 1];
                }
            }
        }
        I420Buffer {
            width: self.width,
            height: self.height,
            stride_y: self.stride_y,
            stride_u: cw as u32,
            stride_v: cw as u32,
            data_y: self.data_y.clone(),
            data_u,
            data_v,
        }
    }

    pub fn to_argb(
        &self,
        _format: VideoFormatType,
        _dst: &mut [u8],
        _dst_stride: u32,
        _dst_width: i32,
        _dst_height: i32,
    ) {
        // TODO(OHOS): implement.
    }

    pub fn scale(&mut self, scaled_width: i32, scaled_height: i32) -> crate::video_frame::NV12Buffer {
        Self::new(
            scaled_width as u32,
            scaled_height as u32,
            scaled_width as u32,
            scaled_width as u32 + scaled_width as u32 % 2,
        )
    }
}

// -------------------------------------------------------------------------
// NativeBuffer (placeholder - OHOS doesn't expose a platform-native handle)
// -------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct NativeBuffer {
    width: u32,
    height: u32,
}

impl NativeBuffer {
    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn to_i420(&self) -> I420Buffer {
        I420Buffer {
            width: self.width,
            height: self.height,
            stride_y: self.width,
            stride_u: chroma_dim(self.width),
            stride_v: chroma_dim(self.width),
            data_y: vec![0u8; self.width as usize * self.height as usize],
            data_u: vec![
                0u8;
                chroma_dim(self.width) as usize * chroma_dim(self.height) as usize
            ],
            data_v: vec![
                0u8;
                chroma_dim(self.width) as usize * chroma_dim(self.height) as usize
            ],
        }
    }

    pub fn to_argb(
        &self,
        _format: VideoFormatType,
        _dst: &mut [u8],
        _dst_stride: u32,
        _dst_width: i32,
        _dst_height: i32,
    ) {
        // TODO(OHOS): no native buffer surface yet.
    }
}
