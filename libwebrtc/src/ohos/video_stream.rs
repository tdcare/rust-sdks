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

//! OHOS native video stream.
//!
//! Pulls inbound RTP packets from the per-track queue registered with
//! [`super::rtc_io_driver::RtcIoDriver`], reassembles H.264 NALUs or VP8 frames
//! via [`rtc_rtp::codec::h264::H264Packet`] or [`rtc_rtp::codec::vp8::Vp8Packet`]
//! and feeds them into the OH_AVCodec hardware decoder (H.264) or libvpx
//! software decoder (VP8). The decoder's output is converted to I420 and
//! surfaced as a [`BoxVideoFrame`].
//!
//! The decoder is lazily initialised the first time we observe a keyframe
//! NAL (SPS/PPS or IDR slice for H.264) or VP8 keyframe so that we don't try
//! to feed dependent frames before a reference frame has arrived.

use std::{
    collections::VecDeque,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use livekit_runtime::Stream;
use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::{
    video_frame::{BoxVideoBuffer, BoxVideoFrame, I420Buffer, VideoFrame, VideoRotation},
    video_track::RtcVideoTrack,
};

use super::{
    packet_trailer::PacketTrailerHandler,
    rtc_io_driver::ReceivedRtpPacket,
    software_vp8::{SoftwareVP8Decoder, SoftwareVP8DecoderConfig},
    video_codec::{DecodedFrame, VideoDecoder, VideoDecoderConfig, AV_PIXEL_FORMAT_NV12},
};

use bytes::Bytes;
use rtc_rtp::{codec::{h264::H264Packet, vp8::Vp8Packet}, packetizer::Depacketizer};

/// Default decoder picture size used until we learn the real one from the
/// SDP/SPS. The OH_AVCodec decoder reconfigures itself when the bitstream
/// dimensions differ, so this is just a starting point.
const DEFAULT_WIDTH: u32 = 1280;
const DEFAULT_HEIGHT: u32 = 720;

/// 90 kHz is the canonical RTP clock rate for video.
const VIDEO_CLOCK_RATE: i64 = 90_000;

/// Codec type for decoding
enum CodecType {
    H264,
    VP8,
}

/// Depacketizer for different codecs
enum DepacketizerType {
    H264(H264Packet),
    VP8(Vp8FrameAssembler),
}

/// VP8 frame assembler: accumulates RTP fragments into complete VP8 frames.
///
/// VP8 frames frequently span multiple RTP packets (keyframes can be 10+ KB while
/// MTU is typically ~1200 bytes).  The `Vp8Packet` depacketizer only strips the
/// per-packet VP8 payload descriptor; this wrapper performs fragment reassembly.
struct Vp8FrameAssembler {
    /// Per-packet VP8 parser.
    parser: Vp8Packet,
    /// Accumulated payload bytes for the current frame.
    buffer: Vec<u8>,
    /// Whether we are currently assembling a fragmented frame.
    assembling: bool,
    /// Timestamp of the current frame (set from the first fragment).
    _current_ts: u32,
    /// After an RTP sequence gap, skip non-S-bit packets until the next
    /// start-of-partition marker to avoid feeding mid-frame fragments to
    /// the decoder as complete frames.
    skip_until_s: bool,
}

impl Default for Vp8FrameAssembler {
    fn default() -> Self {
        Self {
            parser: Vp8Packet::default(),
            buffer: Vec::new(),
            assembling: false,
            _current_ts: 0,
            skip_until_s: false,
        }
    }
}

impl Vp8FrameAssembler {
    /// Reset the assembler state (called on RTP sequence gaps to avoid
    /// feeding corrupted partial frames to the decoder).
    fn reset(&mut self) {
        self.buffer.clear();
        self.assembling = false;
        self.skip_until_s = true;
    }

    /// Feed one RTP payload (already stripped of the RTP fixed header).
    /// Returns the complete assembled VP8 frame data when available, or
    /// empty `Bytes` when waiting for more fragments.
    fn feed(&mut self, payload: &Bytes, marker: bool) -> rtc_shared::error::Result<Bytes> {
        // Parse the VP8 payload descriptor to get the raw frame data.
        let frame_data = self.parser.depacketize(payload)?;

        let has_s_bit = !payload.is_empty() && (payload[0] & 0x10) != 0;

        // After an RTP sequence gap, skip all packets until we see the
        // start of a new partition.  Mid-frame fragments arriving after a
        // gap would otherwise be treated as complete single-packet frames
        // and fed to the decoder, corrupting the reference picture.
        if self.skip_until_s {
            if has_s_bit {
                self.skip_until_s = false;
            } else {
                log::trace!("Vp8FrameAssembler: skipping packet (waiting for S-bit after gap)");
                return Ok(Bytes::new());
            }
        }

        if has_s_bit {
            // Start of a new partition — flush any previously buffered data
            if self.assembling {
                self.buffer.clear();
            }
            self.assembling = true;
            self.buffer.clear();
        }

        if self.assembling || has_s_bit {
            self.buffer.extend_from_slice(&frame_data);
        } else {
            // No S bit and not assembling — treat as a complete single-packet frame.
            return Ok(frame_data);
        }

        if marker {
            // End of frame — return accumulated data.
            let total = self.buffer.len();
            let mut complete = Vec::new();
            std::mem::swap(&mut complete, &mut self.buffer);
            self.assembling = false;
            log::trace!("Vp8FrameAssembler: assembled complete frame total={}bytes", total);
            Ok(Bytes::from(complete))
        } else {
            Ok(Bytes::new())
        }
    }
}

/// Decoder for different codecs
enum DecoderType {
    H264(VideoDecoder),
    VP8(SoftwareVP8Decoder),
}

pub struct NativeVideoStream {
    video_track: RtcVideoTrack,
    rx: mpsc::UnboundedReceiver<ReceivedRtpPacket>,

    // Depacketizer for the detected codec
    depacketizer: DepacketizerType,

    // Decoder for the detected codec
    decoder: Option<DecoderType>,
    decoder_initialized: bool,
    decoder_width: u32,
    decoder_height: u32,
    codec_type: Option<CodecType>,

    // Decoded frames waiting to be drained by the consumer.
    decoded_frames: VecDeque<BoxVideoFrame>,

    // Sequence-number tracking for gap detection.
    last_seq: Option<u16>,

    /// Frame counter for log throttling (only log every N frames).
    output_frame_count: u64,

    closed: bool,
    packet_trailer_handler: Arc<Mutex<Option<PacketTrailerHandler>>>,
}

impl NativeVideoStream {
    /// Construct a stream backed by a pre-allocated receive queue. The
    /// receiver is automatically picked up from the track when the peer
    /// connection announced a remote track for it.
    pub fn new(video_track: RtcVideoTrack, _queue_size_frames: Option<usize>) -> Self {
        let packet_trailer_handler =
            Arc::new(Mutex::new(video_track.handle.packet_trailer_handler()));
        // Use the pre-allocated RTP receiver that the peer connection
        // stored on the track when the remote track was announced.
        // Falls back to an empty channel for local tracks or when the
        // receiver was already consumed.
        let has_rx = video_track.handle.take_rtp_rx();
        let rx = match has_rx {
            Some(rx) => {
                log::info!(
                    "[NativeVideoStream] take_rtp_rx returned Some for track={}",
                    video_track.handle.id()
                );
                rx
            }
            None => {
                log::error!(
                    "[NativeVideoStream] take_rtp_rx returned NONE for track={} — \
                     stream will close immediately! This means rtp_rx was never set \
                     or was already consumed.",
                    video_track.handle.id()
                );
                let (_tx, rx) = mpsc::unbounded_channel();
                rx
            }
        };
        Self::build(video_track, rx, packet_trailer_handler)
    }

    /// Construct a stream with an explicit receive queue. Kept for
    /// internal/testing use; the public [`Self::new`] now picks up the
    /// receiver from the track automatically.
    pub(crate) fn new_with_receiver(
        video_track: RtcVideoTrack,
        rx: mpsc::UnboundedReceiver<ReceivedRtpPacket>,
    ) -> Self {
        let packet_trailer_handler =
            Arc::new(Mutex::new(video_track.handle.packet_trailer_handler()));
        Self::build(video_track, rx, packet_trailer_handler)
    }

    fn build(
        video_track: RtcVideoTrack,
        rx: mpsc::UnboundedReceiver<ReceivedRtpPacket>,
        packet_trailer_handler: Arc<Mutex<Option<PacketTrailerHandler>>>,
    ) -> Self {
        Self {
            video_track,
            rx,
            depacketizer: DepacketizerType::H264(H264Packet::default()),
            decoder: None,
            decoder_initialized: false,
            decoder_width: DEFAULT_WIDTH,
            decoder_height: DEFAULT_HEIGHT,
            codec_type: None,
            decoded_frames: VecDeque::new(),
            last_seq: None,
            output_frame_count: 0,
            closed: false,
            packet_trailer_handler,
        }
    }

    /// Set the packet trailer handler for this stream.
    pub fn set_packet_trailer_handler(&self, handler: PacketTrailerHandler) {
        self.packet_trailer_handler.lock().replace(handler);
    }

    pub fn track(&self) -> RtcVideoTrack {
        self.video_track.clone()
    }

    pub fn close(&mut self) {
        self.closed = true;
        self.rx.close();
        if let Some(decoder) = self.decoder.take() {
            match decoder {
                DecoderType::H264(dec) => {
                    let _ = dec.destroy();
                }
                DecoderType::VP8(_) => {
                    // SoftwareVP8Decoder will be dropped automatically
                }
            }
        }
    }

    fn detect_codec_type(&mut self, payload_type: u8, payload: &[u8]) {
        if self.codec_type.is_some() {
            return;
        }

        // ── Priority 1: SDP negotiation result ──
        // Check if codec_mime was explicitly set via SDP negotiation.
        let explicit_mime = self.video_track.handle.codec_mime_value();
        log::info!(
            "detect_codec_type: explicit_mime={:?}, pt={}, payload_len={}",
            explicit_mime, payload_type, payload.len()
        );

        if let Some(ref mime) = explicit_mime {
            let upper = mime.to_uppercase();
            if upper.contains("VP8") {
                log::info!(
                    "detect_codec_type: codec=VP8 determined via SDP mime='{}', pt={}",
                    mime, payload_type
                );
                self.depacketizer = DepacketizerType::VP8(Vp8FrameAssembler::default());
                self.codec_type = Some(CodecType::VP8);
                return;
            } else if upper.contains("H264") {
                log::info!(
                    "detect_codec_type: codec=H264 determined via SDP mime='{}', pt={}",
                    mime, payload_type
                );
                self.depacketizer = DepacketizerType::H264(H264Packet::default());
                self.codec_type = Some(CodecType::H264);
                return;
            }
            log::warn!(
                "detect_codec_type: unknown codec in mime '{}', pt={}, trying PT mapping",
                mime, payload_type
            );
        }

        // ── Priority 2: PayloadType mapping ──
        // When SDP negotiation did not provide codec info (codec_preferences()
        // returned empty), use the well-known PT mapping consistent with the
        // codecs registered in peer_connection_factory.rs:
        //   PT 96       → VP8
        //   PT 125, 108 → H.264
        if let Some(codec) = Self::codec_from_payload_type(payload_type) {
            match codec {
                CodecType::VP8 => {
                    log::info!(
                        "detect_codec_type: codec=VP8 determined via PT mapping (pt={})",
                        payload_type
                    );
                    self.depacketizer = DepacketizerType::VP8(Vp8FrameAssembler::default());
                    self.codec_type = Some(CodecType::VP8);
                }
                CodecType::H264 => {
                    log::info!(
                        "detect_codec_type: codec=H264 determined via PT mapping (pt={})",
                        payload_type
                    );
                    self.depacketizer = DepacketizerType::H264(H264Packet::default());
                    self.codec_type = Some(CodecType::H264);
                }
            }
            return;
        }

        // ── Priority 3: Payload byte probing (last resort) ──
        // Only reached when both SDP and PT mapping fail.
        log::warn!(
            "detect_codec_type: SDP and PT mapping both failed (pt={}), using payload byte probe",
            payload_type
        );
        if !payload.is_empty() {
            let first = payload[0];
            let nal_type = first & 0x1F;
            let is_h264_like = matches!(nal_type, 1..=5 | 24 | 28);

            if is_h264_like {
                log::warn!(
                    "detect_codec_type: byte probe → H264 (first_byte=0x{:02X}, nal_type={}), pt={}",
                    first, nal_type, payload_type
                );
                self.depacketizer = DepacketizerType::H264(H264Packet::default());
                self.codec_type = Some(CodecType::H264);
            } else {
                log::warn!(
                    "detect_codec_type: byte probe → VP8 (first_byte=0x{:02X}, nal_type={} not H264), pt={}",
                    first, nal_type, payload_type
                );
                self.depacketizer = DepacketizerType::VP8(Vp8FrameAssembler::default());
                self.codec_type = Some(CodecType::VP8);
            }
        } else {
            log::warn!(
                "detect_codec_type: empty payload, cannot probe; defaulting to H264, pt={}",
                payload_type
            );
            self.codec_type = Some(CodecType::H264);
        }
    }

    /// Map a well-known RTP payload type to its codec.
    ///
    /// These values must stay in sync with the codecs registered in
    /// `peer_connection_factory.rs`:
    ///   - PT 96       → VP8   (video/VP8)
    ///   - PT 125, 108 → H.264 (video/H264)
    fn codec_from_payload_type(pt: u8) -> Option<CodecType> {
        match pt {
            96 => Some(CodecType::VP8),
            125 | 108 => Some(CodecType::H264),
            _ => None,
        }
    }

    fn process_rtp_packet(&mut self, pkt: &ReceivedRtpPacket) {
        // Detect codec type on first packet
        self.detect_codec_type(pkt.payload_type, &pkt.payload);

        // Detect sequence-number gaps; we don't try to repair them here, the
        // depacketiser will simply discard partially-reassembled FU-A units.
        // For VP8, we must explicitly reset the frame assembler to avoid
        // feeding corrupted partial-frame data to the decoder, which would
        // cause visual artifacts that propagate through P-frames until the
        // next keyframe arrives.
        if let Some(last) = self.last_seq {
            let expected = last.wrapping_add(1);
            if pkt.sequence_number != expected {
                log::warn!(
                    "video RTP seq gap: {} -> {} (track {}), resetting VP8 assembler",
                    last,
                    pkt.sequence_number,
                    pkt.track_id,
                );
                // Reset VP8 frame assembler — the missing packet likely
                // contained a fragment of the current frame, so the
                // accumulated buffer is now corrupt.  Waiting for the
                // next S-bit (start-of-partition) packet is safer than
                // feeding incomplete data to the decoder.
                if let DepacketizerType::VP8(assembler) = &mut self.depacketizer {
                    assembler.reset();
                }
            }
        }
        self.last_seq = Some(pkt.sequence_number);

        if pkt.payload.is_empty() {
            return;
        }

        let payload = Bytes::copy_from_slice(&pkt.payload);
        let depacketized = match &mut self.depacketizer {
            DepacketizerType::H264(dep) => dep.depacketize(&payload),
            DepacketizerType::VP8(dep) => dep.feed(&payload, pkt.marker),
        };

        match depacketized {
            Ok(frame_data) if !frame_data.is_empty() => {
                self.feed_decoder(&frame_data, pkt.timestamp);
            }
            Ok(_) => {
                // Mid-fragment; nothing to feed yet.
            }
            Err(e) => {
                // If the H264 depacketizer errors on what looks like VP8,
                // switch codec and retry the SAME packet with VP8.
                if matches!(&self.codec_type, Some(CodecType::H264))
                    && !pkt.payload.is_empty()
                {
                    let first = pkt.payload[0];
                    let nal_type = first & 0x1F;
                    if !matches!(nal_type, 1..=5 | 24 | 28) {
                        log::warn!(
                            "process_rtp_packet: H264 depacketizer failed (err='{}'), payload \
                             first_byte=0x{:02X} not valid H264 NAL; switching to VP8 (pt={})",
                            e, first, pkt.payload_type
                        );
                        self.depacketizer = DepacketizerType::VP8(Vp8FrameAssembler::default());
                        self.codec_type = Some(CodecType::VP8);
                        self.decoder = None;
                        self.decoder_initialized = false;
                        // Retry the current payload with the VP8 depacketizer
                        let payload2 = Bytes::copy_from_slice(&pkt.payload);
                        if let DepacketizerType::VP8(dep) = &mut self.depacketizer {
                            match dep.feed(&payload2, pkt.marker) {
                                Ok(frame_data) if !frame_data.is_empty() => {
                                    self.feed_decoder(&frame_data, pkt.timestamp);
                                }
                                Ok(_) => {}
                                Err(e2) => log::warn!("VP8 retry depacketize error: {}", e2),
                            }
                        }
                        return;
                    }
                }
                log::warn!("depacketize error: {}", e);
            }
        }
    }

    fn feed_decoder(&mut self, frame_data: &[u8], rtp_timestamp: u32) {
        let codec_type = match &self.codec_type {
            Some(ct) => ct,
            None => return,
        };

        match codec_type {
            CodecType::H264 => self.feed_h264_decoder(frame_data, rtp_timestamp),
            CodecType::VP8 => self.feed_vp8_decoder(frame_data, rtp_timestamp),
        }
    }

    fn feed_h264_decoder(&mut self, annexb: &[u8], rtp_timestamp: u32) {
        // Each NALU emitted by the depacketiser is prefixed with the 4-byte
        // Annex-B start code 0x00 0x00 0x00 0x01, so the NAL header lives at
        // index 4 (or 3 for the rare 3-byte start code).
        let nal_header_idx = if annexb.len() >= 4
            && annexb[0] == 0
            && annexb[1] == 0
            && annexb[2] == 0
            && annexb[3] == 1
        {
            4
        } else if annexb.len() >= 3 && annexb[0] == 0 && annexb[1] == 0 && annexb[2] == 1 {
            3
        } else {
            0
        };

        let first_nal_type = annexb.get(nal_header_idx).map(|b| b & 0x1F).unwrap_or(0);
        let contains_keyframe_indicator =
            // SPS / PPS / IDR — any of these is enough to bootstrap the decoder.
            matches!(first_nal_type, 5 | 7 | 8) || annexb_contains_idr(annexb);
        let is_key = contains_keyframe_indicator;

        if !self.decoder_initialized {
            if !contains_keyframe_indicator {
                // Wait for a keyframe / parameter set before bringing the
                // decoder up; trying to decode dependent P-frames produces
                // garbage on most hardware decoders.
                return;
            }
            let config = VideoDecoderConfig {
                width: self.decoder_width,
                height: self.decoder_height,
                pixel_format: AV_PIXEL_FORMAT_NV12,
            };
            let mut decoder = VideoDecoder::new();
            if let Err(e) = decoder.create_h264(config) {
                log::warn!("H264 decoder create failed: {}", e);
                return;
            }
            if let Err(e) = decoder.start() {
                log::warn!("H264 decoder start failed: {}", e);
                let _ = decoder.destroy();
                return;
            }
            self.decoder = Some(DecoderType::H264(decoder));
            self.decoder_initialized = true;
            log::info!("H264 decoder initialized");
        }

        let timestamp_us = (rtp_timestamp as i64) * 1_000_000 / VIDEO_CLOCK_RATE;
        if let Some(DecoderType::H264(decoder)) = &mut self.decoder {
            if let Err(e) = decoder.decode_frame(annexb, timestamp_us, is_key) {
                log::warn!("H264 decoder decode_frame failed: {}", e);
            }
        }
    }

    fn feed_vp8_decoder(&mut self, vp8_data: &[u8], rtp_timestamp: u32) {
        // VP8 keyframe detection: first byte bit 0 is 0 for keyframes
        let is_key = !vp8_data.is_empty() && (vp8_data[0] & 0x01) == 0;

        if !self.decoder_initialized {
            if !is_key {
                // Wait for a keyframe before bringing the decoder up
                return;
            }
            let config = SoftwareVP8DecoderConfig {
                width: self.decoder_width,
                height: self.decoder_height,
            };
            let mut decoder = SoftwareVP8Decoder::new(config);
            if !decoder.initialize() {
                log::warn!("VP8 decoder initialize failed");
                return;
            }
            self.decoder = Some(DecoderType::VP8(decoder));
            self.decoder_initialized = true;
            log::info!("VP8 decoder initialized");
        }

        let timestamp_us = (rtp_timestamp as i64) * 1_000_000 / VIDEO_CLOCK_RATE;
        if let Some(DecoderType::VP8(decoder)) = &mut self.decoder {
            if !decoder.decode(vp8_data, timestamp_us) {
                // VP8 decoder errors are normal before first keyframe, don't spam logs
            }
        }
    }

    fn poll_decoder_output(&mut self) {
        let codec_type = match &self.codec_type {
            Some(ct) => ct,
            None => return,
        };

        match codec_type {
            CodecType::H264 => self.poll_h264_decoder_output(),
            CodecType::VP8 => self.poll_vp8_decoder_output(),
        }
    }

    fn poll_h264_decoder_output(&mut self) {
        let mut decoded_frames = Vec::new();
        
        if let Some(DecoderType::H264(decoder)) = &mut self.decoder {
            while let Some(decoded) = decoder.poll_output() {
                if decoded.is_eos {
                    break;
                }
                decoded_frames.push(decoded);
            }
        }
        
        // Convert frames outside the mutable borrow
        for decoded in decoded_frames {
            let frame = Self::h264_decoded_frame_to_video_frame_static(&decoded, self.decoder_width, self.decoder_height);
            self.decoded_frames.push_back(frame);
        }
    }

    fn poll_vp8_decoder_output(&mut self) {
        let mut decoded_frames = Vec::new();
        
        if let Some(DecoderType::VP8(decoder)) = &mut self.decoder {
            while let Some(decoded) = decoder.poll_output() {
                decoded_frames.push(decoded);
            }
        }
        
        // Convert frames outside the mutable borrow
        for decoded in decoded_frames {
            log::trace!(
                "poll_vp8_decoder_output: converting frame {}x{}, i420_data_len={}, ts_us={}",
                decoded.width, decoded.height, decoded.i420_data.len(), decoded.timestamp_us,
            );
            let frame = Self::vp8_decoded_frame_to_video_frame_static(&decoded);
            self.decoded_frames.push_back(frame);
        }
    }

    fn h264_decoded_frame_to_video_frame_static(decoded: &DecodedFrame, width: u32, height: u32) -> BoxVideoFrame {
        // NV12 layout: Y plane (width*height) followed by interleaved UV
        // plane (width*height/2). We split the UV plane into separate U and
        // V arrays as expected by I420.
        //
        // OHOS hardware codec may pad rows to alignment boundaries (e.g. 64
        // bytes), so we detect and handle stride-padded NV12.
        let w = width as usize;
        let h = height as usize;
        let cw = (w + 1) / 2;
        let ch = (h + 1) / 2;
        let y_size = w * h;
        let chroma_size = y_size / 4; // = cw * ch

        let data = &decoded.data;
        let compact_nv12 = y_size + y_size / 2; // w*h*3/2

        // Detect stride padding: for NV12, total = stride_y * h + stride_y * ch
        // → stride_y = data.len() * 2 / (3 * h)
        let stride_y = if data.len() > compact_nv12 && h > 0 {
            data.len() * 2 / (3 * h)
        } else {
            w
        };

        if stride_y != w && (stride_y as i64 - w as i64).abs() > 4 {
            log::debug!(
                "[H264->I420] STRIDE_PAD: {}x{}, stride_y={} (width={}), data_len={} vs compact={}",
                w, h, stride_y, w, data.len(), compact_nv12
            );
        }

        let (y_vec, u_vec, v_vec) = if stride_y == w {
            // Fast path: no stride padding
            let y_end = y_size.min(data.len());
            let y_plane = &data[..y_end];
            let uv_plane: &[u8] = if data.len() > y_size { &data[y_size..] } else { &[] };

            let mut u_plane = Vec::with_capacity(chroma_size);
            let mut v_plane = Vec::with_capacity(chroma_size);
            for chunk in uv_plane.chunks_exact(2) {
                u_plane.push(chunk[0]);
                v_plane.push(chunk[1]);
            }
            (y_plane.to_vec(), u_plane, v_plane)
        } else {
            // Stride-padded NV12: extract Y and UV without padding bytes
            let mut y_plane = Vec::with_capacity(y_size);
            for row in 0..h {
                let start = row * stride_y;
                let end = start + w;
                if end <= data.len() {
                    y_plane.extend_from_slice(&data[start..end]);
                }
            }

            let mut u_plane = Vec::with_capacity(chroma_size);
            let mut v_plane = Vec::with_capacity(chroma_size);
            let uv_start = h * stride_y;
            for row in 0..ch {
                let start = uv_start + row * stride_y;
                for col in 0..cw {
                    let idx = start + col * 2;
                    if idx + 1 < data.len() {
                        u_plane.push(data[idx]);
                        v_plane.push(data[idx + 1]);
                    }
                }
            }
            (y_plane, u_plane, v_plane)
        };

        let mut buffer = I420Buffer::new(width, height);
        let (dy, du, dv) = buffer.data_mut();
        let copy_y = y_vec.len().min(dy.len());
        dy[..copy_y].copy_from_slice(&y_vec[..copy_y]);
        let copy_u = u_vec.len().min(du.len());
        du[..copy_u].copy_from_slice(&u_vec[..copy_u]);
        let copy_v = v_vec.len().min(dv.len());
        dv[..copy_v].copy_from_slice(&v_vec[..copy_v]);

        let boxed: BoxVideoBuffer = Box::new(buffer);
        VideoFrame {
            rotation: VideoRotation::VideoRotation0,
            timestamp_us: decoded.timestamp_us,
            frame_metadata: None,
            buffer: boxed,
        }
    }

    fn vp8_decoded_frame_to_video_frame_static(decoded: &super::software_vp8::DecodedFrame) -> BoxVideoFrame {
        let width = decoded.width;
        let height = decoded.height;

        // VP8 decoder already outputs I420 format, so we can use it directly
        let mut buffer = I420Buffer::new(width, height);
        let (dy, du, dv) = buffer.data_mut();
        
        let y_size = (width as usize) * (height as usize);
        let chroma_size = y_size / 4;
        
        let data = &decoded.i420_data;
        let copy_y = y_size.min(data.len()).min(dy.len());
        dy[..copy_y].copy_from_slice(&data[..copy_y]);
        
        if data.len() > y_size {
            let u_start = y_size;
            let copy_u = chroma_size.min(data.len() - y_size).min(du.len());
            du[..copy_u].copy_from_slice(&data[u_start..u_start + copy_u]);
            
            if data.len() > y_size + chroma_size {
                let v_start = y_size + chroma_size;
                let copy_v = chroma_size.min(data.len() - y_size - chroma_size).min(dv.len());
                dv[..copy_v].copy_from_slice(&data[v_start..v_start + copy_v]);
            }
        }

        let boxed: BoxVideoBuffer = Box::new(buffer);
        VideoFrame {
            rotation: VideoRotation::VideoRotation0,
            timestamp_us: decoded.timestamp_us,
            frame_metadata: None,
            buffer: boxed,
        }
    }
}

impl Drop for NativeVideoStream {
    fn drop(&mut self) {
        self.close();
    }
}

impl Stream for NativeVideoStream {
    type Item = BoxVideoFrame;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.closed {
            return Poll::Ready(None);
        }

        let codec_label = match &this.codec_type {
            Some(CodecType::H264) => "H264",
            Some(CodecType::VP8) => "VP8",
            None => "unknown",
        };

        // 1. Drain any frames that the decoder produced asynchronously
        //    since the last poll.
        this.poll_decoder_output();
        if let Some(frame) = this.decoded_frames.pop_front() {
            this.output_frame_count += 1;
            if this.output_frame_count == 1 {
                log::info!(
                    "poll_next: returning frame #{} [{}] {}x{}, ts_us={}, buf_type={:?}",
                    this.output_frame_count, codec_label, frame.buffer.width(), frame.buffer.height(),
                    frame.timestamp_us, frame.buffer.buffer_type(),
                );
            }
            return Poll::Ready(Some(frame));
        }

        // 2. Pull RTP packets and feed them in until either the channel is
        //    exhausted, a frame becomes available, or no more packets are
        //    immediately ready.
        loop {
            match this.rx.poll_recv(cx) {
                Poll::Ready(Some(pkt)) => {
                    this.process_rtp_packet(&pkt);
                    this.poll_decoder_output();
                    if let Some(frame) = this.decoded_frames.pop_front() {
                        this.output_frame_count += 1;
                        if this.output_frame_count == 1 {
                            log::info!(
                                "poll_next: returning frame #{} [{}] {}x{}, ts_us={}, buf_type={:?}",
                                this.output_frame_count, codec_label, frame.buffer.width(), frame.buffer.height(),
                                frame.timestamp_us, frame.buffer.buffer_type(),
                            );
                        }
                        return Poll::Ready(Some(frame));
                    }
                    // continue draining the queue
                }
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => {
                    this.poll_decoder_output();
                    if let Some(frame) = this.decoded_frames.pop_front() {
                        this.output_frame_count += 1;
                        if this.output_frame_count == 1 {
                            log::info!(
                                "poll_next: returning frame #{} [{}] {}x{}, ts_us={}, buf_type={:?}",
                                this.output_frame_count, codec_label, frame.buffer.width(), frame.buffer.height(),
                                frame.timestamp_us, frame.buffer.buffer_type(),
                            );
                        }
                        return Poll::Ready(Some(frame));
                    }
                    return Poll::Pending;
                }
            }
        }
    }
}

/// Scan an Annex-B byte stream for an IDR NAL (type 5). Used to decide
/// whether a STAP-A aggregated unit qualifies as a keyframe even when the
/// first NAL is SPS/PPS.
fn annexb_contains_idr(data: &[u8]) -> bool {
    let len = data.len();
    let mut i = 0;
    while i + 3 < len {
        let three_byte = data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1;
        let four_byte = data[i] == 0
            && data[i + 1] == 0
            && data[i + 2] == 0
            && i + 3 < len
            && data[i + 3] == 1;
        if four_byte {
            if i + 4 < len && (data[i + 4] & 0x1F) == 5 {
                return true;
            }
            i += 4;
        } else if three_byte {
            if i + 3 < len && (data[i + 3] & 0x1F) == 5 {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_idr_nal() {
        // start_code, NAL header byte = 0x65 (forbidden_zero=0, ref_idc=3, type=5)
        let data = [0, 0, 0, 1, 0x65, 0xAB, 0xCD];
        assert!(annexb_contains_idr(&data));
    }

    #[test]
    fn rejects_non_idr_only_stream() {
        // Only contains a P-slice (type 1) and SPS (type 7).
        let data = [0, 0, 0, 1, 0x41, 0xAB, 0, 0, 0, 1, 0x67, 0x42];
        assert!(!annexb_contains_idr(&data));
    }
}
