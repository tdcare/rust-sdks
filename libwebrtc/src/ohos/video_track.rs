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

//! OHOS video track wrapper.
//!
//! Adds an optional [`PacketTrailerHandler`] alongside the shared
//! [`MediaStreamTrack`] state, mirroring the native implementation's
//! ability to forward per-frame metadata.

use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::media_stream_track::RtcTrackState;

use super::{
    media_stream_track::MediaStreamTrack, packet_trailer::PacketTrailerHandler,
    rtc_io_driver::ReceivedRtpPacket, video_source::NativeVideoSource,
};

#[derive(Clone)]
pub struct RtcVideoTrack {
    pub(crate) track: MediaStreamTrack,
    pub(crate) packet_trailer_handler: Arc<Mutex<Option<PacketTrailerHandler>>>,
    /// Video source feeding this track. `None` for remote tracks.
    pub(crate) source: Option<NativeVideoSource>,
    /// Pre-allocated inbound RTP queue for remote tracks. Set by the peer
    /// connection when a `RemoteTrack` event fires; consumed lazily by
    /// `NativeVideoStream` at construction time.
    pub(crate) rtp_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<ReceivedRtpPacket>>>>,
    /// Codec MIME type for remote tracks (e.g., "video/VP8", "video/H264").
    /// Set during SDP negotiation.
    pub(crate) codec_mime: Arc<Mutex<Option<String>>>,
}

impl RtcVideoTrack {
    /// Create a track that does not own a source. Used by remote-track glue.
    pub(crate) fn new(track: MediaStreamTrack) -> Self {
        Self {
            track,
            packet_trailer_handler: Arc::new(Mutex::new(None)),
            source: None,
            rtp_rx: Arc::new(Mutex::new(None)),
            codec_mime: Arc::new(Mutex::new(None)),
        }
    }

    /// Create a track tied to a local [`NativeVideoSource`].
    pub(crate) fn with_source(track: MediaStreamTrack, source: NativeVideoSource) -> Self {
        Self {
            track,
            packet_trailer_handler: Arc::new(Mutex::new(None)),
            source: Some(source),
            rtp_rx: Arc::new(Mutex::new(None)),
            codec_mime: Arc::new(Mutex::new(None)),
        }
    }

    /// Store the pre-allocated inbound RTP receiver on the track so that
    /// a later `NativeVideoStream` construction can pick it up.
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

    pub fn set_packet_trailer_handler(&self, handler: PacketTrailerHandler) {
        self.packet_trailer_handler.lock().replace(handler);
    }

    pub fn packet_trailer_handler(&self) -> Option<PacketTrailerHandler> {
        self.packet_trailer_handler.lock().clone()
    }

    /// Local video source feeding this track, if any.
    #[allow(dead_code)]
    pub(crate) fn source(&self) -> Option<&NativeVideoSource> {
        self.source.as_ref()
    }

    /// Get the codec MIME type for this track.
    /// Returns the default "video/H264" when no codec was negotiated.
    pub fn codec_name(&self) -> String {
        self.codec_mime.lock().clone().unwrap_or_else(|| "video/H264".to_string())
    }

    /// Get the actual codec MIME value, returning `None` if not set via SDP.
    /// Use this instead of `codec_name()` when the caller needs to
    /// distinguish "explicitly set to H264" from "not set (default)".
    pub(crate) fn codec_mime_value(&self) -> Option<String> {
        self.codec_mime.lock().clone()
    }

    /// Set the codec MIME type for this track.
    pub(crate) fn set_codec_mime(&self, mime: String) {
        log::info!("[video_track] set_codec_mime: track={}, mime={}", self.track.id(), mime);
        *self.codec_mime.lock() = Some(mime);
    }
}
