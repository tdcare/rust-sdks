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

//! OHOS YUV conversion stubs.
//!
//! These mirror the public surface of [`native::yuv_helper`](crate::native::yuv_helper)
//! so call-sites compile, but conversions aren't implemented yet. Each
//! function leaves the destination buffer untouched (zero-fill from the
//! caller's allocation) and returns. Wire up libyuv-style conversions before
//! shipping anything that depends on these.

#![allow(clippy::too_many_arguments)]

fn unimplemented_stub(_who: &'static str) {
    // TODO(OHOS): integrate libyuv (or pure-Rust equivalent) for YUV/ARGB
    // conversion. For now we silently no-op so the rest of the pipeline can
    // compile and the encoder fast-path (which does I420 -> codec directly)
    // keeps working without conversions.
}

pub fn argb_to_rgb24(
    _src: &[u8],
    _src_stride: u32,
    _dst: &mut [u8],
    _dst_stride: u32,
    _width: i32,
    _height: i32,
) {
    unimplemented_stub("argb_to_rgb24");
}

pub fn i420_to_nv12(
    _src_y: &[u8],
    _src_stride_y: u32,
    _src_u: &[u8],
    _src_stride_u: u32,
    _src_v: &[u8],
    _src_stride_v: u32,
    _dst_y: &mut [u8],
    _dst_stride_y: u32,
    _dst_uv: &mut [u8],
    _dst_stride_uv: u32,
    _width: i32,
    _height: i32,
) {
    unimplemented_stub("i420_to_nv12");
}

pub fn nv12_to_i420(
    _src_y: &[u8],
    _src_stride_y: u32,
    _src_uv: &[u8],
    _src_stride_uv: u32,
    _dst_y: &mut [u8],
    _dst_stride_y: u32,
    _dst_u: &mut [u8],
    _dst_stride_u: u32,
    _dst_v: &mut [u8],
    _dst_stride_v: u32,
    _width: i32,
    _height: i32,
) {
    unimplemented_stub("nv12_to_i420");
}

pub fn i444_to_i420(
    _src_y: &[u8],
    _src_stride_y: u32,
    _src_u: &[u8],
    _src_stride_u: u32,
    _src_v: &[u8],
    _src_stride_v: u32,
    _dst_y: &mut [u8],
    _dst_stride_y: u32,
    _dst_u: &mut [u8],
    _dst_stride_u: u32,
    _dst_v: &mut [u8],
    _dst_stride_v: u32,
    _width: i32,
    _height: i32,
) {
    unimplemented_stub("i444_to_i420");
}

pub fn i422_to_i420(
    _src_y: &[u8],
    _src_stride_y: u32,
    _src_u: &[u8],
    _src_stride_u: u32,
    _src_v: &[u8],
    _src_stride_v: u32,
    _dst_y: &mut [u8],
    _dst_stride_y: u32,
    _dst_u: &mut [u8],
    _dst_stride_u: u32,
    _dst_v: &mut [u8],
    _dst_stride_v: u32,
    _width: i32,
    _height: i32,
) {
    unimplemented_stub("i422_to_i420");
}

pub fn i010_to_i420(
    _src_y: &[u16],
    _src_stride_y: u32,
    _src_u: &[u16],
    _src_stride_u: u32,
    _src_v: &[u16],
    _src_stride_v: u32,
    _dst_y: &mut [u8],
    _dst_stride_y: u32,
    _dst_u: &mut [u8],
    _dst_stride_u: u32,
    _dst_v: &mut [u8],
    _dst_stride_v: u32,
    _width: i32,
    _height: i32,
) {
    unimplemented_stub("i010_to_i420");
}

pub fn abgr_to_nv12(
    _src: &[u8],
    _src_stride: u32,
    _dst_y: &mut [u8],
    _dst_stride_y: u32,
    _dst_uv: &mut [u8],
    _dst_stride_uv: u32,
    _width: i32,
    _height: i32,
) {
    unimplemented_stub("abgr_to_nv12");
}

pub fn argb_to_nv12(
    _src: &[u8],
    _src_stride: u32,
    _dst_y: &mut [u8],
    _dst_stride_y: u32,
    _dst_uv: &mut [u8],
    _dst_stride_uv: u32,
    _width: i32,
    _height: i32,
) {
    unimplemented_stub("argb_to_nv12");
}
