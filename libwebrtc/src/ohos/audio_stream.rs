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

//! OHOS native audio stream.
//!
//! Pulls decrypted RTP packets out of the per-track queue registered with
//! [`super::rtc_io_driver::RtcIoDriver`] and surfaces them as
//! [`AudioFrame`]s. Payload-type aware: we route static PT values (≤95) into
//! the L16/PCMU/PCMA decoder family and dynamic PT values (≥96) into the
//! Opus path. The Opus decoder is created lazily on first dynamic-PT
//! payload (so a stream that only ever carries L16 never spins up libopus)
//! and reused across the lifetime of the stream. RTP sequence-number gaps
//! drive `decode_plc` to emit a concealment frame in place of the missing
//! packet.

use std::{
    borrow::Cow,
    collections::VecDeque,
    pin::Pin,
    task::{Context, Poll},
};

use livekit_runtime::Stream;
use tokio::sync::mpsc;

use crate::{audio_frame::AudioFrame, audio_track::RtcAudioTrack};

use super::{opus_decoder::OpusDecoder, rtc_io_driver::ReceivedRtpPacket};

/// First payload-type value reserved for dynamic codec mappings (RFC 3551).
const DYNAMIC_PT_BASE: u8 = 96;

/// Maximum number of decoded audio frames to buffer before dropping the
/// oldest frames. At 20 ms per Opus frame this bounds end-to-end latency
/// to roughly 2 seconds.  Setting a cap is critical on OHOS because the
/// [`OHAudio`] renderer can fall behind real-time when the HDF audio
/// driver is saturated; without a bound the unbounded
/// [`VecDeque`](std::collections::VecDeque) backing the decoded-frames
/// queue would grow without limit, producing multi-minute audio lag.
const MAX_QUEUE_FRAMES: usize = 100;

pub struct NativeAudioStream {
    audio_track: RtcAudioTrack,
    sample_rate: u32,
    num_channels: u32,
    rx: mpsc::UnboundedReceiver<ReceivedRtpPacket>,
    decoded_frames: VecDeque<AudioFrame<'static>>,
    last_seq: Option<u16>,
    /// Lazily initialised on the first dynamic-PT (Opus) payload. `None`
    /// means we have not yet attempted to construct it; once we have tried
    /// and failed we leave it `None` and fall back to silence frames so
    /// the stream stays alive.
    opus_decoder: Option<OpusDecoder>,
    /// Running count of frames dropped because `decoded_frames` exceeded
    /// [`MAX_QUEUE_FRAMES`].  Reset on close.
    dropped_frames: u64,
    closed: bool,
}

impl NativeAudioStream {
    /// Construct a stream backed by a pre-allocated receive queue. The
    /// receiver is automatically picked up from the track when the peer
    /// connection announced a remote track for it.
    pub fn new(
        audio_track: RtcAudioTrack,
        sample_rate: i32,
        num_channels: i32,
        _queue_size_frames: Option<usize>,
    ) -> Self {
        // Use the pre-allocated RTP receiver that the peer connection
        // stored on the track when the remote track was announced.
        // Falls back to an empty channel for local tracks or when the
        // receiver was already consumed.
        let rx = audio_track.handle.take_rtp_rx().unwrap_or_else(|| {
            let (_tx, rx) = mpsc::unbounded_channel();
            rx
        });
        Self {
            audio_track,
            sample_rate: normalize_rate(sample_rate),
            num_channels: normalize_channels(num_channels),
            rx,
            decoded_frames: VecDeque::new(),
            last_seq: None,
            opus_decoder: None,
            dropped_frames: 0,
            closed: false,
        }
    }

    /// Construct a stream with an explicit receive queue. Kept for
    /// internal/testing use; the public [`Self::new`] now picks up the
    /// receiver from the track automatically.
    pub(crate) fn new_with_receiver(
        audio_track: RtcAudioTrack,
        sample_rate: i32,
        num_channels: i32,
        rx: mpsc::UnboundedReceiver<ReceivedRtpPacket>,
    ) -> Self {
        Self {
            audio_track,
            sample_rate: normalize_rate(sample_rate),
            num_channels: normalize_channels(num_channels),
            rx,
            decoded_frames: VecDeque::new(),
            last_seq: None,
            opus_decoder: None,
            dropped_frames: 0,
            closed: false,
        }
    }

    pub fn track(&self) -> RtcAudioTrack {
        self.audio_track.clone()
    }

    pub fn close(&mut self) {
        self.closed = true;
        self.rx.close();
    }

    fn process_rtp_packet(&mut self, pkt: &ReceivedRtpPacket) {
        if let Some(last) = self.last_seq {
            let expected = last.wrapping_add(1);
            if pkt.sequence_number != expected {
                log::warn!(
                    "audio RTP seq gap: {} -> {} (track {})",
                    last,
                    pkt.sequence_number,
                    pkt.track_id,
                );
                // Synthesise a PLC frame for each missing packet so the
                // downstream consumer still sees continuous pacing. Cap
                // the gap so a malicious / bogus jump in sequence numbers
                // cannot make us allocate unbounded memory.
                let gap = pkt.sequence_number.wrapping_sub(expected);
                let plc_count = gap.min(8) as usize;
                for _ in 0..plc_count {
                    if let Some(frame) = self.decode_plc_frame() {
                        self.decoded_frames.push_back(frame);
                    } else {
                        break;
                    }
                }
            }
        }
        self.last_seq = Some(pkt.sequence_number);

        if pkt.payload.is_empty() {
            return;
        }

        let frame = if pkt.payload_type < DYNAMIC_PT_BASE {
            decode_pcm_payload(&pkt.payload, self.sample_rate, self.num_channels)
        } else {
            self.decode_opus_packet(&pkt.payload)
        };
        self.decoded_frames.push_back(frame);

        // Cap queue depth so that a slow OHAudio renderer cannot cause
        // unbounded latency growth.  See [`MAX_QUEUE_FRAMES`] doc.
        self.trim_queue();
    }

    /// Drop the oldest decoded frames when the queue exceeds
    /// [`MAX_QUEUE_FRAMES`].  This is the latency safety-valve: every
    /// dropped frame = 20 ms saved.
    fn trim_queue(&mut self) {
        let excess = self.decoded_frames.len().saturating_sub(MAX_QUEUE_FRAMES);
        if excess == 0 {
            return;
        }
        // Fast path: drain a single contiguous range.
        self.decoded_frames.drain(..excess);
        self.dropped_frames += excess as u64;

        // Log the drop event at a bounded rate so the operator can see
        // that latency has been capped.
        if self.dropped_frames == excess as u64 || self.dropped_frames % 200 == 0 {
            log::warn!(
                "[NativeAudioStream] latency cap: dropped {} frames \
                 (queue > {MAX_QUEUE_FRAMES}), total dropped={}",
                excess,
                self.dropped_frames,
            );
        }
    }

    /// Decode a dynamic-PT (Opus) payload, lazily constructing the
    /// decoder on first use.
    fn decode_opus_packet(&mut self, payload: &[u8]) -> AudioFrame<'static> {
        let decoder = self.ensure_opus_decoder();
        if let Some(decoder) = decoder {
            match decoder.decode(payload) {
                Ok(samples) => {
                    let channels = self.num_channels.max(1);
                    let samples_per_channel = (samples.len() as u32) / channels;
                    return AudioFrame {
                        data: Cow::Owned(samples),
                        sample_rate: self.sample_rate,
                        num_channels: channels,
                        samples_per_channel,
                    };
                }
                Err(e) => {
                    log::warn!("opus decode failed: {}", e.message);
                }
            }
        }
        silence_frame(self.sample_rate, self.num_channels)
    }

    /// Run the Opus PLC path for a single missing packet; returns `None`
    /// if no decoder is available (i.e. we have never seen an Opus packet
    /// yet and therefore have no codec state to extrapolate from).
    fn decode_plc_frame(&mut self) -> Option<AudioFrame<'static>> {
        let decoder = self.opus_decoder.as_mut()?;
        match decoder.decode_plc() {
            Ok(samples) => {
                let channels = self.num_channels.max(1);
                let samples_per_channel = (samples.len() as u32) / channels;
                Some(AudioFrame {
                    data: Cow::Owned(samples),
                    sample_rate: self.sample_rate,
                    num_channels: channels,
                    samples_per_channel,
                })
            }
            Err(e) => {
                log::warn!("opus plc failed: {}", e.message);
                None
            }
        }
    }

    fn ensure_opus_decoder(&mut self) -> Option<&mut OpusDecoder> {
        if self.opus_decoder.is_none() {
            match OpusDecoder::new(self.sample_rate, self.num_channels) {
                Ok(decoder) => self.opus_decoder = Some(decoder),
                Err(e) => {
                    log::error!("failed to initialise opus decoder: {}", e.message);
                    return None;
                }
            }
        }
        self.opus_decoder.as_mut()
    }
}

impl Drop for NativeAudioStream {
    fn drop(&mut self) {
        self.close();
    }
}

impl Stream for NativeAudioStream {
    type Item = AudioFrame<'static>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.closed {
            return Poll::Ready(None);
        }

        // Fast path: deliver a buffered frame immediately.
        if let Some(frame) = this.decoded_frames.pop_front() {
            this.log_queue_depth();
            return Poll::Ready(Some(frame));
        }

        // Drain the RTP channel until we have at least one decoded frame
        // or the channel is empty.  Track how many packets we processed
        // so we can diagnose when the consumer is falling behind the
        // producer.
        let mut drained = 0u32;
        loop {
            match this.rx.poll_recv(cx) {
                Poll::Ready(Some(pkt)) => {
                    drained += 1;
                    this.process_rtp_packet(&pkt);
                    if let Some(frame) = this.decoded_frames.pop_front() {
                        if drained > 10 {
                            log::warn!(
                                "[NativeAudioStream] consumer behind: \
                                 drained {drained} RTP pkts in one poll, \
                                 decoded_frames depth={} (dropped={})",
                                this.decoded_frames.len(),
                                this.dropped_frames,
                            );
                        }
                        this.log_queue_depth();
                        return Poll::Ready(Some(frame));
                    }
                    // continue draining – producer is ahead of consumer
                }
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl NativeAudioStream {
    /// Periodic diagnostic: log the queue depth so we can see whether
    /// the OHAudio renderer is keeping up with the network.
    fn log_queue_depth(&self) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static CALL_COUNT: AtomicU64 = AtomicU64::new(0);
        let count = CALL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        let depth = self.decoded_frames.len();
        // Log every 200th call, or immediately when depth exceeds 50
        // frames (1 s at 20 ms/frame) – the latter indicates the
        // renderer is starting to fall behind.
        if depth > 50 || count == 1 || count % 200 == 0 {
            log::info!(
                "[NativeAudioStream] poll #{count}: \
                 decoded_frames depth={depth}, dropped={}",
                self.dropped_frames,
            );
        }
    }
}

fn normalize_rate(sample_rate: i32) -> u32 {
    if sample_rate <= 0 {
        // 48 kHz matches the Opus default that the rest of the OHOS pipeline
        // assumes when no rate is supplied.
        48_000
    } else {
        sample_rate as u32
    }
}

fn normalize_channels(num_channels: i32) -> u32 {
    if num_channels <= 0 {
        1
    } else {
        num_channels as u32
    }
}

/// Decode a static-PT PCM payload. Per RFC 3551 the L16 family transports
/// 16-bit signed PCM in big-endian network byte order.
fn decode_pcm_payload(payload: &[u8], sample_rate: u32, num_channels: u32) -> AudioFrame<'static> {
    let samples: Vec<i16> =
        payload.chunks_exact(2).map(|c| i16::from_be_bytes([c[0], c[1]])).collect();
    let channels = num_channels.max(1);
    let samples_per_channel = (samples.len() as u32) / channels;
    AudioFrame {
        data: Cow::Owned(samples),
        sample_rate,
        num_channels: channels,
        samples_per_channel,
    }
}

/// 20 ms of silence at the negotiated rate. Used as a last-resort fallback
/// when the Opus decoder is unavailable or returns an error.
fn silence_frame(sample_rate: u32, num_channels: u32) -> AudioFrame<'static> {
    let channels = num_channels.max(1);
    let samples_per_channel = sample_rate / 50; // 20 ms
    let total_samples = (samples_per_channel * channels) as usize;
    AudioFrame {
        data: Cow::Owned(vec![0i16; total_samples]),
        sample_rate,
        num_channels: channels,
        samples_per_channel,
    }
}
