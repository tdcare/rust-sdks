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

//! Opus audio decoder wrapper for the OHOS platform.
//!
//! Wraps the [`opus`] crate (libopus binding) with a small adapter that
//! plugs into [`super::audio_stream::NativeAudioStream`]'s RTP receive
//! pipeline. The decoder is created lazily once the stream's negotiated
//! sample rate / channel count is known and re-uses a single output
//! buffer sized for the worst-case Opus frame (120 ms @ 48 kHz stereo).

use opus::{Channels, Decoder};

use crate::{RtcError, RtcErrorType};

/// Maximum samples-per-channel for a single Opus frame is 120 ms at the
/// highest sample rate Opus supports (48 kHz), i.e. 5760 samples.
const MAX_SAMPLES_PER_CHANNEL: usize = 5760;

/// Stateful Opus decoder bound to a fixed sample rate / channel layout.
pub(crate) struct OpusDecoder {
    decoder: Decoder,
    num_channels: u32,
    /// Pre-allocated interleaved PCM scratch buffer reused across calls.
    decode_buf: Vec<i16>,
}

impl OpusDecoder {
    /// Create a decoder for the given sample rate / channel count.
    ///
    /// libopus only supports a discrete set of sample rates; anything
    /// outside the supported set is coerced to 48 kHz (Opus' native rate).
    pub fn new(sample_rate: u32, num_channels: u32) -> Result<Self, RtcError> {
        let resolved_rate = match sample_rate {
            8_000 | 12_000 | 16_000 | 24_000 | 48_000 => sample_rate,
            _ => 48_000,
        };
        let resolved_channels = if num_channels >= 2 { 2 } else { 1 };

        let opus_channels =
            if resolved_channels == 2 { Channels::Stereo } else { Channels::Mono };

        let decoder = Decoder::new(resolved_rate, opus_channels).map_err(|e| RtcError {
            error_type: RtcErrorType::Internal,
            message: format!("failed to create opus decoder: {e}"),
        })?;

        let buf_len = MAX_SAMPLES_PER_CHANNEL * resolved_channels as usize;

        Ok(Self {
            decoder,
            num_channels: resolved_channels,
            decode_buf: vec![0i16; buf_len],
        })
    }

    /// Decode a single Opus packet into interleaved PCM samples.
    ///
    /// Returned `Vec` length is `samples_per_channel * num_channels`.
    pub fn decode(&mut self, opus_data: &[u8]) -> Result<Vec<i16>, RtcError> {
        let samples_per_channel =
            self.decoder.decode(opus_data, &mut self.decode_buf, false).map_err(|e| {
                RtcError {
                    error_type: RtcErrorType::Internal,
                    message: format!("opus decode error: {e}"),
                }
            })?;

        let total = samples_per_channel * self.num_channels as usize;
        Ok(self.decode_buf[..total].to_vec())
    }

    /// Generate a Packet Loss Concealment (PLC) frame.
    ///
    /// libopus treats an empty input slice as "packet lost" and synthesises
    /// a concealment frame whose duration matches the previously decoded
    /// packet.
    pub fn decode_plc(&mut self) -> Result<Vec<i16>, RtcError> {
        let samples_per_channel =
            self.decoder.decode(&[], &mut self.decode_buf, false).map_err(|e| RtcError {
                error_type: RtcErrorType::Internal,
                message: format!("opus plc error: {e}"),
            })?;

        let total = samples_per_channel * self.num_channels as usize;
        Ok(self.decode_buf[..total].to_vec())
    }
}