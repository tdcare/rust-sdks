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

//! OHOS pure-Rust [`NativeAudioSource`].
//!
//! Audio frames are pushed by the application via [`capture_frame`], buffered
//! into a fixed-size queue, and consumed by the encoder/track binding.
//! Buffering matches the native semantics: `queue_size_ms == 0` requires
//! 10 ms frames and bypasses the queue, otherwise frames are split into
//! `queue_size_ms` chunks before being enqueued.

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use parking_lot::Mutex;

use crate::{
    audio_frame::AudioFrame,
    audio_source::AudioSourceOptions,
    RtcError, RtcErrorType,
};

use super::rtp_send_pipeline::RtpSendPipeline;

/// Opus frame size in milliseconds.
const OPUS_FRAME_MS: u32 = 20;

/// Debug counter for [`encode_and_send`]; used to throttle hex-dump logging.
static ENCODE_FRAME_COUNT: AtomicU64 = AtomicU64::new(0);

/// Maximum encoded Opus frame size in bytes (worst-case for high-bitrate stereo).
const MAX_OPUS_FRAME_BYTES: usize = 4000;

/// Maximum PCM samples that a single drain can extract (20 ms @ 48kHz stereo).
const MAX_DRAIN_SAMPLES: usize = (48000 * 20 / 1000) * 2;

/// State shared between clones of [`NativeAudioSource`].
///
/// The Opus encoder is created lazily on first use because the `opus` crate
/// encoder is not `Send + Sync` by default and must be accessed through a
/// mutex.
struct EncoderState {
    encoder: Option<opus::Encoder>,
    /// PCM sample buffer (interleaved i16).
    buffer: VecDeque<i16>,
    /// Reusable scratch buffer for extracting Opus-sized PCM frames
    /// without a per-frame `drain().collect()` allocation.
    pcm_scratch: Vec<i16>,
    /// Reusable output buffer for Opus encoding.
    output: Vec<u8>,
    /// Reusable buffer for collecting encoded byte chunks so that
    /// multiple Opus frames produced in a single `encode_and_send` call
    /// share one allocation instead of per-frame `to_vec()`.
    encoded_chunks: Vec<u8>,
    /// Running timestamp in milliseconds.
    timestamp_ms: u64,
    /// Number of consecutive RTP send failures.
    send_fail_count: u64,
    /// --- sonora AEC (WebRTC AEC3 + NS + AGC) ---
    /// Created lazily when echo_cancellation is enabled.
    apm: Option<sonora::AudioProcessing>,
    /// Far-end render reference buffer (speaker output i16 PCM).
    render_buffer: VecDeque<i16>,
    /// Accumulated 10ms AEC output before reassembly into 20ms Opus frames.
    aec_output: Vec<i16>,
    /// Stored AEC config for periodic filter reset.
    aec_config: Option<sonora::Config>,
}

#[derive(Clone)]
pub struct NativeAudioSource {
    options: Arc<Mutex<AudioSourceOptions>>,
    sample_rate: u32,
    num_channels: u32,
    queue_size_samples: u32,
    /// Opus encoder + PCM buffer + output scratch space.
    encoder_state: Arc<Mutex<EncoderState>>,
    /// Optional pipeline that forwards encoded Opus frames as RTP packets.
    rtp_pipeline: Arc<Mutex<Option<RtpSendPipeline>>>,
}

impl NativeAudioSource {
    /// Create a new audio source.
    pub fn new(
        options: AudioSourceOptions,
        sample_rate: u32,
        num_channels: u32,
        queue_size_ms: u32,
    ) -> Self {
        assert!(queue_size_ms % 10 == 0, "queue_size_ms must be a multiple of 10");
        let queue_size_samples = (queue_size_ms * sample_rate * num_channels) / 1000;
        Self {
            options: Arc::new(Mutex::new(options)),
            sample_rate,
            num_channels,
            queue_size_samples,
            encoder_state: Arc::new(Mutex::new(EncoderState {
                encoder: None,
                buffer: VecDeque::with_capacity(queue_size_samples.max(9600) as usize),
                pcm_scratch: Vec::with_capacity(MAX_DRAIN_SAMPLES),
                output: vec![0u8; MAX_OPUS_FRAME_BYTES],
                encoded_chunks: Vec::with_capacity(MAX_OPUS_FRAME_BYTES * 4),
                timestamp_ms: 0,
                send_fail_count: 0,
                apm: None,
                render_buffer: VecDeque::with_capacity(9600),
                aec_output: Vec::with_capacity(960),
                aec_config: None,
            })),
            rtp_pipeline: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn bind_rtp_pipeline(&self, pipeline: RtpSendPipeline) {
        log::info!("[NativeAudioSource] bind_rtp_pipeline: audio RTP pipeline BOUND, ssrc={}", pipeline.ssrc());
        *self.rtp_pipeline.lock() = Some(pipeline);
    }

    pub fn send_encoded_frame(
        &self,
        data: &[u8],
        timestamp_ms: u64,
    ) -> Result<(), RtcError> {
        let mut guard = self.rtp_pipeline.lock();
        let pipeline = guard.as_mut().ok_or_else(|| RtcError {
            error_type: RtcErrorType::InvalidState,
            message: "audio source not bound to an RTP pipeline".into(),
        })?;
        pipeline.send_encoded_audio(data, timestamp_ms)
    }

    pub fn clear_buffer(&self) {
        self.encoder_state.lock().buffer.clear();
    }

    fn ensure_encoder(state: &mut EncoderState, sample_rate: u32, num_channels: u32) -> Result<(), RtcError> {
        if state.encoder.is_some() {
            return Ok(());
        }
        let channels = match num_channels {
            1 => opus::Channels::Mono,
            2 => opus::Channels::Stereo,
            _ => {
                return Err(RtcError {
                    error_type: RtcErrorType::InvalidState,
                    message: format!("unsupported channel count: {num_channels}"),
                });
            }
        };
        let mut encoder = opus::Encoder::new(sample_rate, channels, opus::Application::Audio)
            .map_err(|e| {
                log::error!("[NativeAudioSource] Opus encoder creation FAILED: {}", e);
                RtcError {
                    error_type: RtcErrorType::Internal,
                    message: format!("failed to create opus encoder: {e}"),
                }
            })?;
        encoder.set_bitrate(opus::Bitrate::Bits(64000)).ok();
        encoder.set_inband_fec(true).ok();
        state.encoder = Some(encoder);
        log::info!("[NativeAudioSource] Opus encoder initialised: rate={} ch={}", sample_rate, num_channels);
        Ok(())
    }

    /// Drain buffered PCM in 20 ms chunks, encode with Opus, and send via RTP.
    fn encode_and_send(&self) {
        // Diagnostic: confirm encode_and_send entry
        {
            static ENCODE_ENTRY_COUNT: AtomicU64 = AtomicU64::new(0);
            let n = ENCODE_ENTRY_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            let buf_len = self.encoder_state.lock().buffer.len();
            if n == 1 || n % 100 == 0 {
                log::trace!("[NativeAudioSource] encode_and_send ENTRY #{}: buffer_len={} pcm_samples",
                    n, buf_len);
            }
        }

        let frame_samples = ((self.sample_rate * OPUS_FRAME_MS) / 1000) as usize * self.num_channels as usize;

        // Phase 1: encode all buffered PCM while holding encoder_state lock.
        let encoded_frames: Vec<(u64, usize)>;  // (timestamp_ms, offset in encoded_chunks)
        let encoded_chunks: Vec<u8>;
        {
            let mut state = self.encoder_state.lock();

            if Self::ensure_encoder(&mut state, self.sample_rate, self.num_channels).is_err() {
                log::error!("[NativeAudioSource] encode_and_send: ensure_encoder FAILED, returning");
                return;
            }

            let mut encoder = state.encoder.take().unwrap();
            let mut frames = Vec::new();

            state.encoded_chunks.clear();

            while state.buffer.len() >= frame_samples {
                // Use pre-allocated scratch buffer to extract PCM frame
                // without `drain().collect()` allocation.
                // Copy via pop_front to avoid split-borrow conflict through MutexGuard.
                state.pcm_scratch.clear();
                let drain_len = frame_samples.min(state.buffer.len());
                for _ in 0..drain_len {
                    let sample = state.buffer.pop_front().unwrap();
                    state.pcm_scratch.push(sample);
                }

                let pcm = std::mem::take(&mut state.pcm_scratch);
                let encoded_len = match encoder.encode(&pcm, &mut state.output) {
                    Ok(n) => n,
                    Err(e) => {
                        log::warn!("[NativeAudioSource] opus encode error: {e}");
                        continue;
                    }
                };

                state.pcm_scratch = pcm;

                if encoded_len > 0 {
                    let cnt = ENCODE_FRAME_COUNT.fetch_add(1, Ordering::Relaxed);
                    if cnt <= 3 {
                        let pcm_min = state.pcm_scratch.iter().copied().min().unwrap_or(0);
                        let pcm_max = state.pcm_scratch.iter().copied().max().unwrap_or(0);
                        let pcm_head: Vec<i16> = state.pcm_scratch.iter().take(4).copied().collect();
                        let opus_head: Vec<u8> = state.output[..encoded_len.min(8)].to_vec();
                        log::info!(
                            "[NativeAudioSource] frame #{} | pcm_head={:?} pcm_range=[{},{}] | \
                             opus_len={} opus_head={:02x?}",
                            cnt, pcm_head, pcm_min, pcm_max,
                            encoded_len, opus_head,
                        );
                    }

                    let offset = state.encoded_chunks.len();
                    let out_copy = state.output[..encoded_len].to_vec();
                    state.encoded_chunks.extend_from_slice(&out_copy);
                    frames.push((state.timestamp_ms, offset));
                }
                state.timestamp_ms += OPUS_FRAME_MS as u64;
            }

            state.encoder = Some(encoder);
            encoded_frames = frames;
            // Move the Vec out without allocation; leaves an empty Vec behind.
            encoded_chunks = std::mem::take(&mut state.encoded_chunks);
        } // encoder_state lock dropped

        // Phase 2: send encoded frames via RTP pipeline (separate lock).
        if encoded_frames.is_empty() {
            return;
        }
        let mut pipeline = self.rtp_pipeline.lock();
        let pipeline = match pipeline.as_mut() {
            Some(p) => p,
            None => {
                // Pipeline not yet bound — encoded audio is being discarded.
                // Log periodically so we can diagnose delayed binding.
                static DISCARD_COUNT: AtomicU64 = AtomicU64::new(0);
                let n = DISCARD_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                if n == 1 || n % 50 == 0 {
                    log::error!(
                        "[NativeAudioSource] audio pipeline NOT bound! discarding {} encoded frames (total discards={})",
                        encoded_frames.len(), n
                    );
                }
                return;
            }
        };
        for (ts, offset) in &encoded_frames {
            let next_offset = encoded_frames
                .iter()
                .find(|(_, o)| o > offset)
                .map(|(_, o)| *o)
                .unwrap_or(encoded_chunks.len());
            let data = &encoded_chunks[*offset..next_offset];
            if let Err(e) = pipeline.send_encoded_audio(data, *ts) {
                let mut state = self.encoder_state.lock();
                state.send_fail_count += 1;
                let fc = state.send_fail_count;
                drop(state);
                if fc == 1 || fc % 50 == 0 {
                    log::warn!("[NativeAudioSource] send_encoded_audio FAILED (#{}): {}", fc, e.message);
                }
            }
        }
    }

    pub async fn capture_frame(&self, frame: &AudioFrame<'_>) -> Result<(), RtcError> {
        // Diagnostic: confirm capture_frame is being called (ERROR level to ensure hilog visibility)
        {
            static CAPTURE_COUNT: AtomicU64 = AtomicU64::new(0);
            let n = CAPTURE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            if n == 1 || n % 100 == 0 {
                log::trace!("[NativeAudioSource] capture_frame ENTRY #{}: PCM samples={} rate={} ch={}",
                    n, frame.data.len(), frame.sample_rate, frame.num_channels);
            }
        }

        if self.sample_rate != frame.sample_rate || self.num_channels != frame.num_channels {
            return Err(RtcError {
                error_type: RtcErrorType::InvalidState,
                message: "sample_rate and num_channels don't match".to_owned(),
            });
        }

        {
            const CHUNK_SAMPLES: usize = 480; // 10ms at 48kHz mono

            // Take APM out of state to avoid borrow conflicts
            let apm = {
                let mut state = self.encoder_state.lock();
                state.apm.take()
            };

            if let Some(mut apm) = apm {
                let mut state = self.encoder_state.lock();
                // Use a local borrow for the render buffer drain
                let render_frames: Vec<Vec<i16>> = {
                    let mut frames = Vec::new();
                    while state.render_buffer.len() >= CHUNK_SAMPLES {
                        frames.push(state.render_buffer.drain(..CHUNK_SAMPLES).collect());
                    }
                    frames
                };
                drop(state); // release lock before processing

                // Feed render frames to APM
                for ref_frame in &render_frames {
                    let mut ref_out = vec![0i16; CHUNK_SAMPLES];
                    if let Err(e) = apm.process_render_i16(ref_frame, &mut ref_out) {
                        log::warn!("[NativeAudioSource] process_render_i16 error: {:?}", e);
                    }
                }

                // Process capture through APM
                let mut processed = Vec::with_capacity(frame.data.len());
                for chunk in frame.data.chunks(CHUNK_SAMPLES) {
                    let mut out = vec![0i16; chunk.len()];
                    if let Err(e) = apm.process_capture_i16(chunk, &mut out) {
                        log::warn!("[NativeAudioSource] process_capture_i16 error: {:?}, bypassing AEC", e);
                        processed.extend_from_slice(chunk);
                    } else {
                        processed.extend_from_slice(&out[..chunk.len()]);
                    }
                }

                // Push processed data to encoder buffer and put APM back
                let mut state = self.encoder_state.lock();
                // Periodic AEC diagnostic: every 50 frames, log ref availability and I/O levels
                {
                    static AEC_FRAME_COUNT: AtomicU64 = AtomicU64::new(0);
                    let n = AEC_FRAME_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                    if n % 200 == 0 {
                        let ref_available = !state.render_buffer.is_empty();
                        let in_max = frame.data.iter().map(|s| s.abs()).max().unwrap_or(0);
                        let out_max = processed.iter().map(|s| s.abs()).max().unwrap_or(0);
                        log::info!(
                            "[NativeAudioSource] AEC frame #{}: ref_avail={}, ref_buf={}, in_max={}, out_max={}, render_frames_consumed={}",
                            n, ref_available, state.render_buffer.len(), in_max, out_max, render_frames.len()
                        );
                    }
                }
                state.buffer.extend(processed);
                state.apm = Some(apm);
            } else {
                // No AEC — pass through directly
                let mut state = self.encoder_state.lock();
                state.buffer.extend(frame.data.iter().copied());
            }
        }

        self.encode_and_send();

        Ok(())
    }

    pub fn set_audio_options(&self, options: AudioSourceOptions) {
        *self.options.lock() = options;
    }

    /// Enable software AEC (sonora WebRTC AEC3).
    /// Must be called before capture_frame to initialize the processing pipeline.
    pub fn init_aec(&self) {
        let mut state = self.encoder_state.lock();
        if state.apm.is_some() {
            return; // Already initialized
        }
        let config = sonora::Config {
            echo_canceller: Some(sonora::config::EchoCanceller::default()),
            noise_suppression: Some(sonora::config::NoiseSuppression::default()),
            gain_controller2: Some(sonora::config::GainController2::default()),
            ..Default::default()
        };
        let mut apm = sonora::AudioProcessing::builder()
            .config(config.clone())
            .capture_config(sonora::StreamConfig::new(self.sample_rate, self.num_channels as u16))
            .render_config(sonora::StreamConfig::new(self.sample_rate, 1u16)) // render is mono
            .build();
        // Light delay hint (10ms) for faster initial convergence; AEC3 will auto-track drift
        let _ = apm.set_stream_delay_ms(10);
        // Attenuate overly sensitive mic (0.5x = -6dB) to prevent echo overload
        apm.set_capture_pre_gain(0.5);
        state.apm = Some(apm);
        state.aec_config = Some(config);
        log::info!("[NativeAudioSource] AEC initialized: sonora AEC3 + NS + AGC, {}Hz, {}ch",
            self.sample_rate, self.num_channels);
    }

    /// Push far-end reference frame for AEC (speaker output audio).
    /// Called from the render side to provide echo reference.
    pub fn push_reference_frame(&self, data: &[i16]) {
        let mut state = self.encoder_state.lock();
        state.render_buffer.extend(data.iter().copied());
        // Diagnostic: log the first reference frame only
        static REF_COUNT: AtomicU64 = AtomicU64::new(0);
        let n = REF_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if n == 1 {
            log::info!("[NativeAudioSource] push_reference_frame #{}: {} samples, head=[{},{}], total_buf={}",
                n, data.len(), data.first().unwrap_or(&0), data.get(1).unwrap_or(&0), state.render_buffer.len());
        }
        // Keep buffer from growing unbounded (max ~500ms)
        let max_samples = (self.sample_rate as usize) / 2;
        while state.render_buffer.len() > max_samples {
            state.render_buffer.pop_front();
        }
    }

    pub fn audio_options(&self) -> AudioSourceOptions {
        let opts = self.options.lock();
        AudioSourceOptions {
            echo_cancellation: opts.echo_cancellation,
            noise_suppression: opts.noise_suppression,
            auto_gain_control: opts.auto_gain_control,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn num_channels(&self) -> u32 {
        self.num_channels
    }
}
