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

//! ArkTS-facing wrapper around the libwebrtc `NativeAudioSource` used to push
//! captured PCM audio frames into a LiveKit room.

use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};

use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::{native::NativeAudioSource, AudioSourceOptions};
use napi_derive_ohos::napi;
use napi_ohos::bindgen_prelude::*;

static AUDIO_FRAME_COUNT: AtomicU64 = AtomicU64::new(0);

/// Default queue size in milliseconds used when none is provided.
///
/// `100ms` matches the value commonly used by the LiveKit FFI bindings; it
/// allows the caller to push arbitrarily-sized PCM frames without having to
/// align them to 10 ms boundaries.
const DEFAULT_QUEUE_SIZE_MS: u32 = 20;

/// Audio source for capturing and sending interleaved 16-bit PCM audio to a
/// LiveKit room.
#[napi]
pub struct LkAudioSource {
    pub(crate) inner: NativeAudioSource,
}

impl LkAudioSource {
    /// Borrow the underlying [`NativeAudioSource`].
    #[allow(dead_code)]
    pub(crate) fn inner(&self) -> &NativeAudioSource {
        &self.inner
    }
}

#[napi]
impl LkAudioSource {
    /// Create a new audio source.
    ///
    /// * `sample_rate` – Sample rate in Hz, e.g. `48000`.
    /// * `num_channels` – Number of interleaved audio channels.
    /// * `queue_size_ms` – Internal buffer size in milliseconds (must be a
    ///   multiple of 10). Defaults to 100 ms when omitted. Pass `0` to require
    ///   exactly-10 ms frames on every [`capture_frame`] call.
    #[napi(constructor)]
    pub fn new(sample_rate: u32, num_channels: u32, queue_size_ms: Option<u32>) -> Result<Self> {
        let queue_size_ms = queue_size_ms.unwrap_or(DEFAULT_QUEUE_SIZE_MS);
        if queue_size_ms % 10 != 0 {
            return Err(Error::from_reason(
                "queue_size_ms must be a multiple of 10".to_string(),
            ));
        }
        let options = AudioSourceOptions {
            echo_cancellation: false,
            noise_suppression: false,
            auto_gain_control: true,
        };
        Ok(Self {
            inner: NativeAudioSource::new(options, sample_rate, num_channels, queue_size_ms),
        })
    }

    /// Push a PCM audio frame (little-endian, signed 16-bit, interleaved).
    ///
    /// `data` must contain `samples_per_channel * num_channels * 2` bytes.
    /// `sample_rate` and `num_channels` must match the values passed to the
    /// constructor.
    #[napi]
    pub async fn capture_frame(
        &self,
        data: Uint8Array,
        sample_rate: u32,
        num_channels: u32,
        samples_per_channel: u32,
    ) -> Result<()> {
        let bytes: &[u8] = data.as_ref();
        let expected_bytes = (samples_per_channel as usize)
            .checked_mul(num_channels as usize)
            .and_then(|s| s.checked_mul(2))
            .ok_or_else(|| Error::from_reason("audio frame size overflow".to_string()))?;
        if bytes.len() != expected_bytes {
            return Err(Error::from_reason(format!(
                "audio frame buffer size mismatch: got {} bytes, expected {}",
                bytes.len(),
                expected_bytes,
            )));
        }

        // SAFETY: OHOS ARM is always little-endian.  The input bytes represent
        //         packed interleaved i16 PCM samples whose byte-order matches
        //         the host.  Alignment of i16 (2 bytes) is satisfied because
        //         the OHOS audio capturer produces naturally-aligned buffers.
        let samples = bytes.len() / 2;
        let i16_data: &[i16] = unsafe {
            std::slice::from_raw_parts(bytes.as_ptr() as *const i16, samples)
        };

        // We must own the data for the async boundary — `bytes` is a NAPI
        // reference that cannot outlive `data`.  Use memcpy (not per-element
        // decode) via `to_vec()` on the already-reinterpreted slice.
        let owned = i16_data.to_vec();

        let source = self.inner.clone();
        let frame = AudioFrame {
            data: Cow::Owned(owned),
            sample_rate,
            num_channels,
            samples_per_channel,
        };
        let result = source
            .capture_frame(&frame)
            .await
            .map_err(|e| Error::from_reason(format!("capture_frame failed: {e:?}")));

        let count = AUDIO_FRAME_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if count % 100 == 1 {
            log::info!("[LkAudioSource] capture_frame #{}: rate={} ch={} spc={}",
                count, sample_rate, num_channels, samples_per_channel);
        }

        result
    }

    /// Drop any buffered samples that have not yet been encoded.
    #[napi]
    pub fn clear_buffer(&self) {
        self.inner.clear_buffer();
    }

    /// Configured sample rate in Hz.
    #[napi(getter)]
    pub fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    /// Configured number of audio channels.
    #[napi(getter)]
    pub fn num_channels(&self) -> u32 {
        self.inner.num_channels()
    }
}
