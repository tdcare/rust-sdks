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

//! ArkTS-facing wrapper around a remote audio track stream.

use std::sync::Arc;

use futures::StreamExt;
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use napi_derive_ohos::napi;
use napi_ohos::bindgen_prelude::*;
use tokio::sync::Mutex;

use crate::track::LkRemoteAudioTrack;

/// Default sample rate (Hz) used when the consumer does not specify one.
const DEFAULT_SAMPLE_RATE: i32 = 48_000;
/// Default channel count used when the consumer does not specify one.
const DEFAULT_NUM_CHANNELS: i32 = 1;

/// Audio frame received from a remote participant.
///
/// `data` carries 16-bit signed PCM samples encoded as little-endian bytes,
/// interleaved across channels.
#[napi(object)]
pub struct LkAudioFrame {
    /// PCM audio samples (i16 little-endian, channel-interleaved).
    pub data: Buffer,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Number of audio channels.
    pub num_channels: u32,
    /// Samples per channel in this frame.
    pub samples_per_channel: u32,
}

/// Stream for receiving decoded audio frames from a remote audio track.
#[napi]
pub struct LkAudioStream {
    stream: Arc<Mutex<Option<NativeAudioStream>>>,
}

#[napi]
impl LkAudioStream {
    /// Placeholder constructor required by napi-ohos. Use
    /// [`Self::from_track`] or [`Self::from_track_with_options`] to obtain a
    /// usable instance bound to a remote track.
    #[napi(constructor)]
    pub fn new() -> Self {
        Self { stream: Arc::new(Mutex::new(None)) }
    }

    /// Create an audio stream from a remote audio track.
    #[napi(factory)]
    pub fn from_track(track: &LkRemoteAudioTrack) -> Result<Self> {
        let inner = track
            .inner
            .as_ref()
            .ok_or_else(|| Error::from_reason("track is not initialized"))?;
        let native = NativeAudioStream::new(
            inner.rtc_track(),
            DEFAULT_SAMPLE_RATE,
            DEFAULT_NUM_CHANNELS,
        );
        Ok(Self { stream: Arc::new(Mutex::new(Some(native))) })
    }

    /// Create an audio stream with explicit playback parameters.
    ///
    /// `sample_rate` is in Hz (e.g. `48000`) and `num_channels` is the
    /// expected number of interleaved channels in the produced frames.
    #[napi(factory)]
    pub fn from_track_with_options(
        track: &LkRemoteAudioTrack,
        sample_rate: u32,
        num_channels: u32,
    ) -> Result<Self> {
        let inner = track
            .inner
            .as_ref()
            .ok_or_else(|| Error::from_reason("track is not initialized"))?;
        let native =
            NativeAudioStream::new(inner.rtc_track(), sample_rate as i32, num_channels as i32);
        Ok(Self { stream: Arc::new(Mutex::new(Some(native))) })
    }

    /// Await the next audio frame.
    ///
    /// Returns `null` once the stream has been closed or the underlying
    /// track has ended.
    #[napi]
    pub async fn next_frame(&self) -> Result<Option<LkAudioFrame>> {
        let mut guard = self.stream.lock().await;
        let stream = match guard.as_mut() {
            Some(s) => s,
            None => return Ok(None),
        };

        let Some(frame) = stream.next().await else {
            return Ok(None);
        };

        let mut bytes = Vec::with_capacity(frame.data.len() * 2);
        for &sample in frame.data.iter() {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }

        // Periodic diagnostic: log PCM stats of the decoded audio frame
        {
            use std::sync::atomic::{AtomicU64, Ordering};
            static RX_FRAME_COUNT: AtomicU64 = AtomicU64::new(0);
            let count = RX_FRAME_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            if count == 1 || count % 200 == 0 {
                let min = frame.data.iter().copied().min().unwrap_or(0);
                let max = frame.data.iter().copied().max().unwrap_or(0);
                log::info!(
                    "[LkAudioStream] rx_frame #{}: {} samples, {}ch {}Hz, pcm_range=[{},{}], head=[{:02x},{:02x},{:02x},{:02x}]",
                    count,
                    frame.data.len(),
                    frame.num_channels,
                    frame.sample_rate,
                    min,
                    max,
                    bytes.first().copied().unwrap_or(0),
                    bytes.get(1).copied().unwrap_or(0),
                    bytes.get(2).copied().unwrap_or(0),
                    bytes.get(3).copied().unwrap_or(0),
                );
            }
        }

        Ok(Some(LkAudioFrame {
            data: Buffer::from(bytes),
            sample_rate: frame.sample_rate,
            num_channels: frame.num_channels,
            samples_per_channel: frame.samples_per_channel,
        }))
    }

    /// Close the stream and release the underlying receive queue.
    ///
    /// Pending and subsequent calls to [`Self::next_frame`] will resolve
    /// to `null`.
    #[napi]
    pub fn close(&self) {
        let stream = self.stream.clone();
        // Dispatch on the napi-ohos global runtime so this works when called
        // from a synchronous JS context where no tokio runtime is entered.
        napi_ohos::bindgen_prelude::spawn(async move {
            if let Some(mut s) = stream.lock().await.take() {
                s.close();
            }
        });
    }
}
