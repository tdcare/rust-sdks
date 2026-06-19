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

//! OHOS pure-Rust [`NativeVideoSource`].
//!
//! Captured I420 frames are encoded with VP8 (via `libvpx`) and forwarded
//! to the RTP pipeline. If no pipeline is bound yet the frames are silently
//! dropped (the pipeline is attached once the track is added to a peer
//! connection).

use std::sync::Arc;

use parking_lot::Mutex;

use crate::{
    video_frame::{VideoBuffer, VideoFrame},
    video_source::VideoResolution,
    RtcError, RtcErrorType,
};

use super::packet_trailer::PacketTrailerHandler;
use super::rtp_send_pipeline::RtpSendPipeline;
use super::h264_encoder::H264Encoder;
use super::vp8_encoder::Vp8Encoder;

/// Copy a strided plane (width × height, row stride may differ from width)
/// into a contiguous buffer with no padding between rows.
fn copy_with_stride(src: &[u8], stride: u32, width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let s = stride as usize;
    let mut dst = vec![0u8; w * h];
    for row in 0..h {
        let src_off = row * s;
        let dst_off = row * w;
        if src_off + w <= src.len() && dst_off + w <= dst.len() {
            dst[dst_off..dst_off + w].copy_from_slice(&src[src_off..src_off + w]);
        }
    }
    dst
}

/// Rotate I420 data 90° counter-clockwise (270° CW).
///
/// Input dimensions (w, h) become (h, w) after rotation.
/// All three planes (Y, U, V) are rotated independently.
fn rotate_i420_90_ccw(src_y: &[u8], src_u: &[u8], src_v: &[u8], w: u32, h: u32) -> Vec<u8> {
    let new_w = h as usize;
    let new_h = w as usize;
    let half_w = (w + 1) / 2;
    let half_h = (h + 1) / 2;
    let new_half_w = half_h as usize;
    let new_half_h = half_w as usize;

    let y_size = new_w * new_h;
    let uv_size = new_half_w * new_half_h;
    let mut dst = vec![0u8; y_size + 2 * uv_size];

    // Rotate Y plane 90° CCW: W×H → H×W
    let (dst_y, rest) = dst.split_at_mut(y_size);
    for dst_y_idx in 0..new_h {
        for dst_x_idx in 0..new_w {
            let src_x = w as usize - 1 - dst_y_idx;
            let src_row = dst_x_idx;
            let src_idx = src_row * w as usize + src_x;
            let dst_idx = dst_y_idx * new_w + dst_x_idx;
            dst_y[dst_idx] = src_y[src_idx];
        }
    }

    // Rotate U plane
    let (dst_u, dst_v) = rest.split_at_mut(uv_size);
    for dst_y_idx in 0..new_half_h {
        for dst_x_idx in 0..new_half_w {
            let src_x = half_w as usize - 1 - dst_y_idx;
            let src_row = dst_x_idx;
            let src_idx = src_row * half_w as usize + src_x;
            let dst_idx = dst_y_idx * new_half_w + dst_x_idx;
            dst_u[dst_idx] = src_u[src_idx];
        }
    }

    // Rotate V plane
    for dst_y_idx in 0..new_half_h {
        for dst_x_idx in 0..new_half_w {
            let src_x = half_w as usize - 1 - dst_y_idx;
            let src_row = dst_x_idx;
            let src_idx = src_row * half_w as usize + src_x;
            let dst_idx = dst_y_idx * new_half_w + dst_x_idx;
            dst_v[dst_idx] = src_v[src_idx];
        }
    }

    dst
}

/// Wraps either an H.264 hardware encoder or a VP8 software encoder.
enum VideoEncoder {
    H264(H264Encoder),
    Vp8(Vp8Encoder),
}

impl VideoEncoder {
    fn encode(&mut self, i420: &[u8], ts: i64) -> Result<Option<(Vec<u8>, bool)>, RtcError> {
        match self {
            Self::H264(e) => e.encode(i420, ts),
            Self::Vp8(e) => e.encode(i420, ts),
        }
    }

    fn codec_name(&self) -> &'static str {
        match self {
            Self::H264(_) => "H264",
            Self::Vp8(_) => "VP8",
        }
    }
}

/// Shared encoder state. The encoder is created lazily on the first frame.
struct EncoderSlot {
    encoder: Option<VideoEncoder>,
    last_width: u32,
    last_height: u32,
    frames_encoded: u64,
    rtp_packets_sent: u64,
    keyframes_sent: u64,
}

#[derive(Clone)]
pub struct NativeVideoSource {
    resolution: Arc<Mutex<VideoResolution>>,
    is_screencast: bool,
    packet_trailer_handler: Arc<Mutex<Option<PacketTrailerHandler>>>,
    captured_frames: Arc<Mutex<u64>>,
    /// VP8 encoder + RTP pipeline.
    encoder_slot: Arc<Mutex<EncoderSlot>>,
    /// Optional pipeline that forwards encoded video frames as RTP packets.
    rtp_pipeline: Arc<Mutex<Option<RtpSendPipeline>>>,
}

impl NativeVideoSource {
    pub fn new(resolution: VideoResolution, is_screencast: bool) -> Self {
        Self {
            resolution: Arc::new(Mutex::new(resolution)),
            is_screencast,
            packet_trailer_handler: Arc::new(Mutex::new(None)),
            captured_frames: Arc::new(Mutex::new(0)),
            encoder_slot: Arc::new(Mutex::new(EncoderSlot {
                encoder: None,
                last_width: 0,
                last_height: 0,
                frames_encoded: 0,
                rtp_packets_sent: 0,
                keyframes_sent: 0,
            })),
            rtp_pipeline: Arc::new(Mutex::new(None)),
        }
    }

    /// Bind an [`RtpSendPipeline`] used to forward encoded video frames.
    pub(crate) fn bind_rtp_pipeline(&self, pipeline: RtpSendPipeline) {
        log::info!("[NativeVideoSource] bind_rtp_pipeline: video RTP pipeline bound successfully");
        *self.rtp_pipeline.lock() = Some(pipeline);
    }

    /// Accept pre-rotated I420 data directly without further rotation.
    pub fn capture_raw_i420(&self, i420_data: &[u8], width: u32, height: u32, timestamp_us: i64) {
        let count = { let mut c = self.captured_frames.lock(); *c += 1; *c };
        if count % 30 == 1 { log::info!("[NativeVideoSource] capture_raw_i420 #{count}: {width}x{height}"); }
        { let mut res = self.resolution.lock(); if res.width != width || res.height != height { res.width = width; res.height = height; } }
        self.encode_and_send(i420_data, width, height, timestamp_us);
    }

    /// Stub: Jetson DMA-buffer capture is not supported on OHOS.
    /// Exists for API compatibility with the native backend.
    pub fn capture_jetson_frame(&self, _dma_buf_fd: std::os::fd::RawFd, _width: u32, _height: u32, _timestamp_us: i64) {
        // no-op on OHOS
    }

    pub fn send_encoded_frame(
        &self,
        data: &[u8],
        timestamp_ms: u64,
        codec: &str,
        is_key_frame: bool,
    ) -> Result<u32, RtcError> {
        let mut guard = self.rtp_pipeline.lock();
        let pipeline = guard.as_mut().ok_or_else(|| RtcError {
            error_type: RtcErrorType::InvalidState,
            message: "video source not bound to an RTP pipeline".into(),
        })?;
        pipeline.send_encoded_video(data, timestamp_ms, codec, is_key_frame)
    }

    /// Encode an I420 frame and send it via the RTP pipeline.
    ///
    /// Tries H.264 hardware encoder first; falls back to VP8 software
    /// (libvpx) when hardware H.264 is unavailable.
    fn encode_and_send(&self, i420_data: &[u8], width: u32, height: u32, timestamp_us: i64) {
        // Phase 1: encode while holding encoder_slot lock.
        let encoded: Option<(Vec<u8>, bool)>;
        let codec_name: &'static str;
        {
            let mut slot = self.encoder_slot.lock();

            // (Re)create encoder if resolution changed.
            if slot.last_width != width || slot.last_height != height {
                eprintln!("[encode_and_send] resolution changed: {}x{} -> {}x{}, replacing encoder...", slot.last_width, slot.last_height, width, height);
                let kbps = ((width * height * 2) / 1000).max(200);

                // Auto-select codec: H.264 hardware encoder preferred;
                // fall back to VP8 software encoder when H.264 is unavailable.
                let encoder =
                    match H264Encoder::new(width, height, kbps) {
                        Ok(enc) => {
                            log::info!("[NativeVideoSource] using H264 hw encoder: {width}x{height} @ {kbps}kbps");
                            eprintln!("[encode_and_send] H264 encoder created, replacing old encoder...");
                            VideoEncoder::H264(enc)
                        }
                        Err(e) => {
                            log::warn!("[NativeVideoSource] H264 unavailable ({}), falling back to VP8", e.message);
                            match Vp8Encoder::new(width, height, kbps) {
                                Ok(enc) => {
                                    log::info!("[NativeVideoSource] using VP8 sw encoder: {width}x{height} @ {kbps}kbps");
                                    VideoEncoder::Vp8(enc)
                                }
                                Err(e2) => {
                                    log::error!("[NativeVideoSource] CRITICAL: Both H264 and VP8 encoder init failed!");
                                    log::error!("[NativeVideoSource] H264 error: {}", e.message);
                                    log::error!("[NativeVideoSource] VP8 error: {}", e2.message);
                                    return;
                                }
                            }
                        }
                    };
                eprintln!("[encode_and_send] about to replace slot.encoder (old encoder will Drop now)...");
                slot.encoder = Some(encoder);
                eprintln!("[encode_and_send] old encoder dropped, new encoder in place");
                slot.last_width = width;
                slot.last_height = height;
            }

            let encoder = match slot.encoder.as_mut() {
                Some(e) => e,
                None => return,
            };
            codec_name = encoder.codec_name();

            match encoder.encode(i420_data, timestamp_us) {
                Ok(Some((data, is_key))) => {
                    slot.frames_encoded += 1;
                    if is_key {
                        slot.keyframes_sent += 1;
                    }
                    encoded = Some((data, is_key));
                }
                Ok(None) => encoded = None,
                Err(e) => {
                    log::warn!("[NativeVideoSource] encode error: {}", e.message);
                    encoded = None;
                }
            }
        } // encoder_slot lock dropped

        // Phase 2: send via RTP pipeline (separate lock).
        // NOTE: we hold the rtp_pipeline lock while sending so that
        // sequence-number state is preserved between successive frames.
        // send_encoded_video is non-blocking (it just pushes to an mpsc
        // channel), so the lock is never held for a meaningful duration.
        let (data, is_key) = match encoded {
            Some(e) => e,
            None => return,
        };
        let timestamp_ms = (timestamp_us / 1000) as u64;
        let mut pipeline = self.rtp_pipeline.lock();
        if let Some(p) = pipeline.as_mut() {
            let fc = *self.captured_frames.lock();
            match p.send_encoded_video(&data, timestamp_ms, codec_name, is_key) {
                Ok(count) => {
                    // Update statistics
                    {
                        let mut slot = self.encoder_slot.lock();
                        slot.rtp_packets_sent += count as u64;
                        
                        // Periodic heartbeat every 100 frames
                        if fc % 100 == 0 {
                            log::info!(
                                "[NativeVideoSource] HEARTBEAT: captured={}, encoded={}, rtp_pkts={}, keyframes={}, codec={}, ssrc={}",
                                fc, slot.frames_encoded, slot.rtp_packets_sent, slot.keyframes_sent, codec_name, p.ssrc()
                            );
                        }
                    }
                    
                    // Log first 10 frames and every 100th frame
                    if fc <= 10 || fc % 100 == 0 {
                        let head: Vec<String> = data.iter().take(8).map(|b| format!("{:02x}", b)).collect();
                        log::info!(
                            "[NativeVideoSource] RTP sent: {} bytes, {} pkts, codec={}, key={}, ssrc={}, head=[{}], #{}",
                            data.len(), count, codec_name, is_key, p.ssrc(), head.join(" "), fc
                        );
                    }
                }
                Err(e) => {
                    let fc = *self.captured_frames.lock();
                    // Log first failure and then every 60th failure to avoid spam
                    if fc <= 5 || fc % 60 == 0 {
                        log::error!(
                            "[NativeVideoSource] send_encoded_video FAILED (frame #{}): {}",
                            fc, e.message
                        );
                    }
                }
            }
        } else {
            let fc = *self.captured_frames.lock();
            if fc % 60 == 1 {
                log::warn!("[NativeVideoSource] RTP pipeline not bound (#{fc})");
            }
        }
    }

    /// Record and encode a captured I420 frame.
    pub fn capture_frame<T: AsRef<dyn VideoBuffer>>(&self, frame: &VideoFrame<T>) {
        let buffer = frame.buffer.as_ref();
        let w = buffer.width();
        let h = buffer.height();

        let count = {
            let mut c = self.captured_frames.lock();
            *c += 1;
            *c
        };

        if count % 30 == 1 {
            log::info!("[NativeVideoSource] capture_frame #{count}: {w}x{h} buffer_type={:?}", buffer.buffer_type());
        }

        // Extract I420 planes, rotate 90° CCW, and encode.
        // After rotation, w×h becomes h×w — update resolution so SDP/browser
        // receive the correct (rotated) dimensions.
        match buffer.as_i420() {
            Some(i420) => {
                let (stride_y, stride_u, stride_v) = i420.strides();
                let (y, u, v) = i420.data();

                if count % 30 == 1 {
                    log::info!(
                        "[NativeVideoSource] capture_frame #{count}: {w}x{h}, strides: Y={stride_y} U={stride_u} V={stride_v}, Y_slice={}, U_slice={}, V_slice={}",
                        y.len(), u.len(), v.len()
                    );
                }

                // Skip rotation flag — set false for normal operation
                let skip_rotate = false;
                let (enc_w, enc_h, contiguous) = if skip_rotate {
                    // Copy I420 without rotation, pack strides if needed
                    let y_packed = copy_with_stride(y, stride_y, w, h);
                    let half_w = (w + 1) / 2;
                    let half_h = (h + 1) / 2;
                    let u_packed = copy_with_stride(u, stride_u, half_w, half_h);
                    let v_packed = copy_with_stride(v, stride_v, half_w, half_h);
                    let mut i420_packed = Vec::with_capacity((w * h * 3 / 2) as usize);
                    i420_packed.extend_from_slice(&y_packed);
                    i420_packed.extend_from_slice(&u_packed);
                    i420_packed.extend_from_slice(&v_packed);
                    if count % 30 == 1 {
                        log::info!("[NativeVideoSource] NO ROTATE (test): {w}x{h}, total={}", i420_packed.len());
                    }
                    (w, h, i420_packed)
                } else {
                    let contiguous = if stride_y != w || stride_u != (w + 1) / 2 || stride_v != (w + 1) / 2 {
                        let y_packed = copy_with_stride(y, stride_y, w, h);
                        let half_w = (w + 1) / 2;
                        let half_h = (h + 1) / 2;
                        let u_packed = copy_with_stride(u, stride_u, half_w, half_h);
                        let v_packed = copy_with_stride(v, stride_v, half_w, half_h);
                        rotate_i420_90_ccw(&y_packed, &u_packed, &v_packed, w, h)
                    } else {
                        rotate_i420_90_ccw(y, u, v, w, h)
                    };
                    if count % 30 == 1 {
                        log::info!("[NativeVideoSource] I420 rotated 90° CCW: {w}x{h} → {h}x{w}, total={}", contiguous.len());
                    }
                    (h, w, contiguous)
                };
                {
                    let mut res = self.resolution.lock();
                    // Only update if resolution actually changed — avoids
                    // spurious SDP renegotiations that can cause the
                    // connection to flap.
                    if res.width != enc_w || res.height != enc_h {
                        log::info!("[NativeVideoSource] resolution changed: {w}x{h} → {enc_w}x{enc_h}");
                        res.width = enc_w;
                        res.height = enc_h;
                    }
                }
                self.encode_and_send(&contiguous, enc_w, enc_h, frame.timestamp_us);
            }
            None => {
                if count % 30 == 1 {
                    log::warn!("[NativeVideoSource] as_i420() returned None! buffer_type={:?}", buffer.buffer_type());
                }
            }
        }

        if let (Some(meta), Some(handler)) =
            (frame.frame_metadata, self.packet_trailer_handler.lock().clone())
        {
            if let (Some(user_ts), Some(frame_id)) = (meta.user_timestamp, meta.frame_id) {
                handler.store_frame_metadata(frame.timestamp_us, user_ts, frame_id);
            }
        }
    }

    pub fn set_packet_trailer_handler(&self, handler: PacketTrailerHandler) {
        self.packet_trailer_handler.lock().replace(handler);
    }

    pub fn video_resolution(&self) -> VideoResolution {
        self.resolution.lock().clone()
    }

    pub fn is_screencast(&self) -> bool {
        self.is_screencast
    }
}
