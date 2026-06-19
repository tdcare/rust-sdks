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

//! OHOS audio resampler backed by `soxr-sys`.
//!
//! Performs sample-rate conversion (via soxr) and channel remixing
//! (mono <-> stereo) on interleaved INT16 PCM buffers. The soxr instance
//! is cached and only re-created when `(src_rate, dst_rate, channels)`
//! change between successive calls.

use std::ffi::c_void;
use std::ptr;

/// RAII wrapper around the raw `soxr_t` pointer.
struct SoxrHandle {
    ptr: soxr_sys::soxr_t,
}

impl Drop for SoxrHandle {
    fn drop(&mut self) {
        // SAFETY: `ptr` was returned by `soxr_create` and is non-null
        // (we only build a `SoxrHandle` after a successful create).
        unsafe { soxr_sys::soxr_delete(self.ptr) };
    }
}

// SAFETY: A soxr instance is single-threaded but otherwise self-contained;
// it is safe to move across threads as long as it is not accessed
// concurrently. `AudioResampler` is not `Sync` so this is upheld.
unsafe impl Send for SoxrHandle {}

pub struct AudioResampler {
    /// Cached soxr instance (created lazily).
    soxr: Option<SoxrHandle>,
    /// Source sample rate the cached soxr was created with.
    cached_src_rate: u32,
    /// Destination sample rate the cached soxr was created with.
    cached_dst_rate: u32,
    /// Number of channels the cached soxr was created with.
    cached_channels: u32,
    /// Scratch buffer used when remixing channels after resampling.
    remix_buf: Vec<i16>,
    /// Final output buffer returned to the caller. Reused across calls.
    output: Vec<i16>,
}

impl Default for AudioResampler {
    fn default() -> Self {
        Self {
            soxr: None,
            cached_src_rate: 0,
            cached_dst_rate: 0,
            cached_channels: 0,
            remix_buf: Vec::new(),
            output: Vec::new(),
        }
    }
}

impl AudioResampler {
    /// Resample and/or remix the input buffer.
    ///
    /// `src` is interleaved INT16 PCM with `samples_per_channel` frames and
    /// `num_channels` channels. The returned slice references an internal
    /// buffer owned by `self` and is valid until the next call.
    pub fn remix_and_resample<'a>(
        &'a mut self,
        src: &[i16],
        samples_per_channel: u32,
        num_channels: u32,
        sample_rate: u32,
        dst_num_channels: u32,
        dst_sample_rate: u32,
    ) -> &'a [i16] {
        let total_samples = (samples_per_channel as usize)
            .checked_mul(num_channels as usize)
            .expect("samples_per_channel * num_channels overflow");
        assert!(src.len() >= total_samples, "src buffer too small");
        let input = &src[..total_samples];

        // Fast path: nothing to do.
        if num_channels == dst_num_channels && sample_rate == dst_sample_rate {
            self.output.clear();
            self.output.extend_from_slice(input);
            return &self.output;
        }

        // Step 1: resample at the source channel count (writes into self.output).
        let frames_after_resample;
        if sample_rate != dst_sample_rate {
            self.ensure_soxr(sample_rate, dst_sample_rate, num_channels);
            frames_after_resample = self.do_resample(
                input,
                samples_per_channel as usize,
                num_channels as usize,
                src_to_dst_estimate(samples_per_channel, sample_rate, dst_sample_rate),
            );
        } else {
            self.output.clear();
            self.output.extend_from_slice(input);
            frames_after_resample = samples_per_channel as usize;
        }

        // Step 2: channel remixing (reads from self.output via remix_buf swap).
        if num_channels != dst_num_channels {
            self.do_channel_convert(frames_after_resample, num_channels, dst_num_channels);
        }

        &self.output
    }

    /// Create or reuse a soxr instance matching the requested parameters.
    fn ensure_soxr(&mut self, src_rate: u32, dst_rate: u32, channels: u32) {
        if self.soxr.is_some()
            && self.cached_src_rate == src_rate
            && self.cached_dst_rate == dst_rate
            && self.cached_channels == channels
        {
            return;
        }

        // Drop the previous instance (if any) before creating a new one.
        self.soxr = None;
        self.cached_src_rate = 0;
        self.cached_dst_rate = 0;
        self.cached_channels = 0;

        let io_spec = soxr_sys::soxr_io_spec {
            itype: soxr_sys::soxr_datatype_t_SOXR_INT16_I,
            otype: soxr_sys::soxr_datatype_t_SOXR_INT16_I,
            scale: 1.0,
            e: ptr::null_mut(),
            flags: 0,
        };

        let mut error: soxr_sys::soxr_error_t = ptr::null();
        // SAFETY: We pass valid pointers (or null where allowed) and check
        // both `error` and the returned handle before using it.
        let handle = unsafe {
            soxr_sys::soxr_create(
                src_rate as f64,
                dst_rate as f64,
                channels,
                &mut error,
                &io_spec,
                ptr::null(),
                ptr::null(),
            )
        };

        if !error.is_null() || handle.is_null() {
            log::error!(
                "soxr_create failed for {}Hz -> {}Hz x{} channels",
                src_rate,
                dst_rate,
                channels
            );
            return;
        }

        self.soxr = Some(SoxrHandle { ptr: handle });
        self.cached_src_rate = src_rate;
        self.cached_dst_rate = dst_rate;
        self.cached_channels = channels;
    }

    /// Run soxr on `input` and place the resampled audio into `self.output`.
    /// Returns the number of frames written.
    fn do_resample(
        &mut self,
        input: &[i16],
        in_frames: usize,
        channels: usize,
        out_frames_estimate: usize,
    ) -> usize {
        let soxr = match &self.soxr {
            Some(h) => h.ptr,
            None => {
                // soxr unavailable: degrade to pass-through.
                self.output.clear();
                self.output.extend_from_slice(input);
                return in_frames;
            }
        };

        // Reserve output capacity. soxr may produce slightly more or fewer
        // frames than the linear estimate, so add a small safety margin.
        let out_capacity_frames = out_frames_estimate + 16;
        self.output.clear();
        self.output.resize(out_capacity_frames * channels, 0);

        let mut idone: usize = 0;
        let mut odone: usize = 0;
        // SAFETY: `input` and `self.output` are valid for `in_frames` /
        // `out_capacity_frames` frames respectively. soxr does not retain
        // these pointers beyond the call.
        let error = unsafe {
            soxr_sys::soxr_process(
                soxr,
                input.as_ptr() as *const c_void,
                in_frames,
                &mut idone,
                self.output.as_mut_ptr() as *mut c_void,
                out_capacity_frames,
                &mut odone,
            )
        };

        if !error.is_null() {
            log::error!("soxr_process failed");
            self.output.clear();
            self.output.extend_from_slice(input);
            return in_frames;
        }

        self.output.truncate(odone * channels);
        odone
    }

    /// Convert the channel layout in-place by reading from `self.output`,
    /// writing into `self.remix_buf`, and swapping the buffers so the
    /// final result lives in `self.output`.
    fn do_channel_convert(&mut self, frames: usize, src_ch: u32, dst_ch: u32) {
        let src_ch_us = src_ch as usize;
        let dst_ch_us = dst_ch as usize;
        self.remix_buf.clear();
        self.remix_buf.reserve(frames * dst_ch_us);

        let src = self.output.as_slice();
        match (src_ch, dst_ch) {
            (1, 2) => {
                // mono -> stereo: duplicate each sample.
                for i in 0..frames {
                    let s = src[i];
                    self.remix_buf.push(s);
                    self.remix_buf.push(s);
                }
            }
            (2, 1) => {
                // stereo -> mono: average L+R.
                for i in 0..frames {
                    let l = src[i * 2] as i32;
                    let r = src[i * 2 + 1] as i32;
                    self.remix_buf.push(((l + r) / 2) as i16);
                }
            }
            _ => {
                // Generic fallback: copy the first min(src_ch, dst_ch)
                // channels and zero-pad any extra destination channels.
                for i in 0..frames {
                    for ch in 0..dst_ch_us {
                        let sample =
                            if ch < src_ch_us { src[i * src_ch_us + ch] } else { 0 };
                        self.remix_buf.push(sample);
                    }
                }
            }
        }

        std::mem::swap(&mut self.output, &mut self.remix_buf);
    }
}

/// Linear estimate of the output frame count for a resampling step.
fn src_to_dst_estimate(in_frames: u32, src_rate: u32, dst_rate: u32) -> usize {
    ((in_frames as u64 * dst_rate as u64) / src_rate.max(1) as u64 + 1) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_when_formats_match() {
        let mut r = AudioResampler::default();
        let input: Vec<i16> = (0..960).map(|i| i as i16).collect();
        let out = r.remix_and_resample(&input, 480, 2, 48_000, 2, 48_000);
        assert_eq!(out, input.as_slice());
    }

    #[test]
    fn mono_to_stereo_duplicates_samples() {
        let mut r = AudioResampler::default();
        let input: Vec<i16> = vec![1, 2, 3, 4];
        let out = r.remix_and_resample(&input, 4, 1, 48_000, 2, 48_000);
        assert_eq!(out, &[1, 1, 2, 2, 3, 3, 4, 4]);
    }

    #[test]
    fn stereo_to_mono_averages_samples() {
        let mut r = AudioResampler::default();
        let input: Vec<i16> = vec![10, 20, 30, 40, -10, 10, 0, 0];
        let out = r.remix_and_resample(&input, 4, 2, 48_000, 1, 48_000);
        assert_eq!(out, &[15, 35, 0, 0]);
    }
}
