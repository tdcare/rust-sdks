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

//! OHOS audio processing module.
//!
//! Mirrors the API of the native `AudioProcessingModule`. On OHOS the only
//! DSP stage implemented in pure Rust is a 2nd-order Butterworth high-pass
//! filter (80 Hz cutoff) intended to remove DC offset and low-frequency
//! rumble from the capture path. Echo cancellation, automatic gain control
//! and noise suppression are deliberately left as pass-through: OHOS
//! exposes those features through `OH_AudioCapturer` / `OH_AudioRenderer`
//! and they are expected to be enabled at the application layer.
//!
//! `process_reverse_stream` also remains a pass-through because the reverse
//! stream is only consumed by the AEC reference, which OHOS handles
//! natively.

use crate::{RtcError, RtcErrorType};

const HPF_CUTOFF_HZ: f64 = 80.0;
const MAX_CHANNELS: usize = 2;

/// 2nd-order IIR (biquad) filter state for one channel.
///
/// Implements the Direct Form II Transposed delay line; `z1` and `z2` are
/// the two delay registers and must persist across `process_stream` calls
/// for the filter to behave correctly.
struct BiquadState {
    z1: f64,
    z2: f64,
}

impl BiquadState {
    fn new() -> Self {
        Self { z1: 0.0, z2: 0.0 }
    }

    fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

/// Precomputed biquad coefficients (numerator `b*` and denominator `a*`,
/// already normalized by `a0`).
struct BiquadCoeffs {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl BiquadCoeffs {
    /// Compute 2nd-order Butterworth high-pass filter coefficients for the
    /// given sample rate and cutoff frequency using the bilinear transform.
    fn butterworth_hpf(sample_rate: f64, cutoff_hz: f64) -> Self {
        let c = (std::f64::consts::PI * cutoff_hz / sample_rate).tan();
        let sqrt2 = std::f64::consts::SQRT_2;
        let c2 = c * c;
        let a0 = 1.0 + sqrt2 * c + c2;

        Self {
            b0: 1.0 / a0,
            b1: -2.0 / a0,
            b2: 1.0 / a0,
            a1: 2.0 * (c2 - 1.0) / a0,
            a2: (1.0 - sqrt2 * c + c2) / a0,
        }
    }
}

#[allow(dead_code)]
pub struct AudioProcessingModule {
    // The AEC / AGC / NS toggles are kept on the struct to preserve the
    // public API surface even though the OHOS implementation delegates
    // those stages to the native audio framework.
    echo_canceller_enabled: bool,
    gain_controller_enabled: bool,
    high_pass_filter_enabled: bool,
    noise_suppression_enabled: bool,
    stream_delay_ms: i32,
    /// HPF state per channel (up to [`MAX_CHANNELS`]).
    hpf_states: [BiquadState; MAX_CHANNELS],
    /// Cached HPF coefficients, recomputed when the sample rate changes.
    hpf_coeffs: Option<BiquadCoeffs>,
    /// Sample rate the cached coefficients were computed for.
    hpf_sample_rate: i32,
}

impl AudioProcessingModule {
    pub fn new(
        echo_canceller_enabled: bool,
        gain_controller_enabled: bool,
        high_pass_filter_enabled: bool,
        noise_suppression_enabled: bool,
    ) -> Self {
        Self {
            echo_canceller_enabled,
            gain_controller_enabled,
            high_pass_filter_enabled,
            noise_suppression_enabled,
            stream_delay_ms: 0,
            hpf_states: [BiquadState::new(), BiquadState::new()],
            hpf_coeffs: None,
            hpf_sample_rate: 0,
        }
    }

    pub fn process_stream(
        &mut self,
        data: &mut [i16],
        sample_rate: i32,
        num_channels: i32,
    ) -> Result<(), RtcError> {
        Self::validate_chunking(data, sample_rate, num_channels)?;

        if self.high_pass_filter_enabled {
            self.apply_hpf(data, sample_rate, num_channels as usize);
        }

        // AEC / AGC / NS are intentionally pass-through on OHOS: those
        // stages are provided by `OH_AudioCapturer` at the application
        // layer.
        Ok(())
    }

    pub fn process_reverse_stream(
        &mut self,
        data: &mut [i16],
        sample_rate: i32,
        num_channels: i32,
    ) -> Result<(), RtcError> {
        Self::validate_chunking(data, sample_rate, num_channels)?;
        // Reverse stream is only used as the AEC reference, which OHOS
        // handles natively. Nothing to do here.
        Ok(())
    }

    pub fn set_stream_delay_ms(&mut self, delay_ms: i32) -> Result<(), RtcError> {
        if delay_ms < 0 {
            return Err(RtcError {
                error_type: RtcErrorType::Internal,
                message: "stream delay must be non-negative".to_string(),
            });
        }
        self.stream_delay_ms = delay_ms;
        Ok(())
    }

    /// Apply the 2nd-order Butterworth high-pass filter in place on the
    /// interleaved 16-bit PCM buffer. Recomputes coefficients and resets
    /// the per-channel state if the sample rate changed since the last
    /// invocation.
    fn apply_hpf(&mut self, data: &mut [i16], sample_rate: i32, num_channels: usize) {
        if self.hpf_sample_rate != sample_rate || self.hpf_coeffs.is_none() {
            self.hpf_coeffs =
                Some(BiquadCoeffs::butterworth_hpf(sample_rate as f64, HPF_CUTOFF_HZ));
            self.hpf_sample_rate = sample_rate;
            for state in &mut self.hpf_states {
                state.reset();
            }
        }

        let coeffs = self.hpf_coeffs.as_ref().expect("coefficients initialized above");
        let channels = num_channels.min(MAX_CHANNELS);
        if channels == 0 || num_channels == 0 {
            return;
        }

        let num_frames = data.len() / num_channels;
        for frame in 0..num_frames {
            for ch in 0..channels {
                let idx = frame * num_channels + ch;
                let x = data[idx] as f64;
                let state = &mut self.hpf_states[ch];

                // Direct Form II Transposed
                let y = coeffs.b0 * x + state.z1;
                state.z1 = coeffs.b1 * x - coeffs.a1 * y + state.z2;
                state.z2 = coeffs.b2 * x - coeffs.a2 * y;

                data[idx] = y.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16;
            }
        }
    }

    fn validate_chunking(
        data: &[i16],
        sample_rate: i32,
        num_channels: i32,
    ) -> Result<(), RtcError> {
        if sample_rate <= 0 || num_channels <= 0 {
            return Err(RtcError {
                error_type: RtcErrorType::Internal,
                message: "sample_rate and num_channels must be positive".to_string(),
            });
        }
        let samples_per_10ms = (sample_rate as usize / 100) * num_channels as usize;
        if samples_per_10ms == 0
            || data.len() < samples_per_10ms
            || data.len() % samples_per_10ms != 0
        {
            return Err(RtcError {
                error_type: RtcErrorType::Internal,
                message: "slice must have a multiple of 10ms worth of samples".to_string(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dc_buffer(value: i16, samples_per_channel: usize, channels: usize) -> Vec<i16> {
        vec![value; samples_per_channel * channels]
    }

    #[test]
    fn hpf_attenuates_dc() {
        let mut apm = AudioProcessingModule::new(false, false, true, false);
        // 10 ms @ 48 kHz, mono, constant DC offset of 5000.
        let mut data = dc_buffer(5000, 480, 1);
        // Run several frames so the IIR settles.
        for _ in 0..50 {
            apm.process_stream(&mut data, 48000, 1).expect("process ok");
            data = dc_buffer(5000, 480, 1);
        }
        // After warm-up the filter should drive the DC component close to zero.
        apm.process_stream(&mut data, 48000, 1).expect("process ok");
        let max_abs = data.iter().map(|s| s.unsigned_abs() as i32).max().unwrap_or(0);
        assert!(max_abs < 100, "DC not attenuated, residual peak = {max_abs}");
    }

    #[test]
    fn hpf_passthrough_when_disabled() {
        let mut apm = AudioProcessingModule::new(false, false, false, false);
        let original = dc_buffer(5000, 480, 1);
        let mut data = original.clone();
        apm.process_stream(&mut data, 48000, 1).expect("process ok");
        assert_eq!(data, original);
    }

    #[test]
    fn validate_chunking_rejects_bad_shape() {
        let mut apm = AudioProcessingModule::new(false, false, true, false);
        let mut data = vec![0i16; 100];
        assert!(apm.process_stream(&mut data, 48000, 1).is_err());
    }
}
