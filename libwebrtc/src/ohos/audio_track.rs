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

//! OHOS audio track wrapper.
//!
//! Delegates `media_stream_track!`-style methods to the inner
//! [`MediaStreamTrack`] and optionally retains a reference to the
//! [`NativeAudioSource`] feeding it (for outbound tracks).

use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::media_stream_track::RtcTrackState;

use super::audio_source::NativeAudioSource;
use super::media_stream_track::MediaStreamTrack;
use super::rtc_io_driver::ReceivedRtpPacket;

#[derive(Clone)]
pub struct RtcAudioTrack {
    pub(crate) track: MediaStreamTrack,
    /// Audio source feeding this track. `None` for remote tracks (no local
    /// source) and for tracks created via [`new`].
    pub(crate) source: Option<NativeAudioSource>,
    /// Pre-allocated inbound RTP queue for remote tracks. Set by the peer
    /// connection when a `RemoteTrack` event fires; consumed lazily by
    /// `NativeAudioStream` at construction time.
    pub(crate) rtp_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<ReceivedRtpPacket>>>>,
}

impl RtcAudioTrack {
    /// Create a track that does not own a source. Used by remote-track glue.
    pub(crate) fn new(track: MediaStreamTrack) -> Self {
        Self { track, source: None, rtp_rx: Arc::new(Mutex::new(None)) }
    }

    /// Create a track tied to a local [`NativeAudioSource`].
    pub(crate) fn with_source(track: MediaStreamTrack, source: NativeAudioSource) -> Self {
        Self { track, source: Some(source), rtp_rx: Arc::new(Mutex::new(None)) }
    }

    /// Store the pre-allocated inbound RTP receiver on the track so that
    /// a later `NativeAudioStream` construction can pick it up.
    pub(crate) fn set_rtp_rx(
        &self,
        rx: mpsc::UnboundedReceiver<ReceivedRtpPacket>,
    ) {
        self.rtp_rx.lock().replace(rx);
    }

    /// Take the pre-allocated inbound RTP receiver, if one was set by the
    /// peer connection. Returns `None` for local tracks or if the receiver
    /// has already been consumed.
    pub(crate) fn take_rtp_rx(&self) -> Option<mpsc::UnboundedReceiver<ReceivedRtpPacket>> {
        self.rtp_rx.lock().take()
    }

    pub fn id(&self) -> String {
        self.track.id()
    }

    pub fn enabled(&self) -> bool {
        self.track.enabled()
    }

    pub fn set_enabled(&self, enabled: bool) -> bool {
        self.track.set_enabled(enabled)
    }

    pub fn state(&self) -> RtcTrackState {
        self.track.state()
    }

    /// Local audio source feeding this track, if any.
    #[allow(dead_code)]
    pub(crate) fn source(&self) -> Option<&NativeAudioSource> {
        self.source.as_ref()
    }
}
