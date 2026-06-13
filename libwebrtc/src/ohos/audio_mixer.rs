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

//! OHOS audio mixer.
//!
//! Pulls a 10ms frame from each registered [`AudioMixerSource`], resamples
//! and remixes the channel layout when needed, then sums all sources into
//! a single output buffer using i32 intermediate accumulation with
//! saturation clamping to `i16::MIN..=i16::MAX`.

use crate::audio_frame::AudioFrame;

use super::audio_resampler::AudioResampler;

/// Per-frame info matching the native enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFrameInfo {
    Normal,
    Muted,
    Error,
}

pub trait AudioMixerSource: Send + Sync {
    fn ssrc(&self) -> i32;
    fn preferred_sample_rate(&self) -> u32;
    fn get_audio_frame_with_info(&self, target_sample_rate: u32) -> Option<AudioFrame>;
}

pub struct AudioMixer {
    sources: Vec<Box<dyn AudioMixerSource>>,
    resampler: AudioResampler,
    output: Vec<i16>,
    /// Scratch buffer for one resampled source frame. Reused across calls
    /// to avoid per-mix allocations.
    scratch: Vec<i16>,
}

impl Default for AudioMixer {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioMixer {
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
            resampler: AudioResampler::default(),
            output: Vec::new(),
            scratch: Vec::new(),
        }
    }

    pub fn add_source(&mut self, source: impl AudioMixerSource + 'static) {
        self.sources.push(Box::new(source));
    }

    pub fn remove_source(&mut self, ssrc: i32) {
        self.sources.retain(|s| s.ssrc() != ssrc);
    }

    /// Mix the active sources into a single 10ms 48 kHz buffer.
    ///
    /// Each source's frame is resampled and remixed to the target format
    /// (48 kHz, `num_channels`) when needed, then summed into the output
    /// using i32 accumulation with saturation clamping. Sources returning
    /// `None` are skipped silently.
    pub fn mix(&mut self, num_channels: usize) -> &[i16] {
        const TARGET_SAMPLE_RATE: u32 = 48_000;
        const SAMPLES_PER_10MS_48K: usize = 480;
        let frame_size = SAMPLES_PER_10MS_48K * num_channels;

        self.output.clear();
        self.output.resize(frame_size, 0);

        if self.sources.is_empty() {
            return &self.output;
        }

        // Collect frames first to release the immutable borrow on `self.sources`
        // before we touch `self.resampler` mutably below.
        let frames: Vec<AudioFrame<'static>> = self
            .sources
            .iter()
            .filter_map(|s| {
                s.get_audio_frame_with_info(TARGET_SAMPLE_RATE).map(|f| AudioFrame {
                    data: std::borrow::Cow::Owned(f.data.into_owned()),
                    sample_rate: f.sample_rate,
                    num_channels: f.num_channels,
                    samples_per_channel: f.samples_per_channel,
                })
            })
            .collect();

        for frame in &frames {
            // Resample/remix into a local scratch buffer if needed, otherwise
            // borrow the source frame directly. The resampler returns a slice
            // borrowed from itself, so we must copy it out before mixing into
            // `self.output` (which is mutably borrowed during the sum loop).
            self.scratch.clear();
            let samples: &[i16] = if frame.sample_rate != TARGET_SAMPLE_RATE
                || frame.num_channels != num_channels as u32
            {
                let resampled = self.resampler.remix_and_resample(
                    &frame.data,
                    frame.samples_per_channel,
                    frame.num_channels,
                    frame.sample_rate,
                    num_channels as u32,
                    TARGET_SAMPLE_RATE,
                );
                self.scratch.extend_from_slice(resampled);
                &self.scratch
            } else {
                &frame.data
            };

            // Optimization: skip silent sources.
            if samples.iter().all(|&s| s == 0) {
                continue;
            }

            // Sum with i32 intermediate + saturation clamp.
            let mix_len = self.output.len().min(samples.len());
            for i in 0..mix_len {
                let sum = self.output[i] as i32 + samples[i] as i32;
                self.output[i] = sum.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            }
        }

        &self.output
    }
}
