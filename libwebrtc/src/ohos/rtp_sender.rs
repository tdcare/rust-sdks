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
//
// OHOS implementation - pure Rust stub backed by webrtc-rs/rtc.
//
// This module provides the OHOS-platform `RtpSender` handle that the public
// `crate::rtp_sender::RtpSender` wrapper delegates to. The shape mirrors the
// native libwebrtc handle so the public API stays identical.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::{
    rtp_sender::VideoEncoderBackend,
    media_stream_track::MediaStreamTrack,
    rtp_parameters::{RtcpParameters, RtpParameters},
    stats::RtcStats,
    RtcError,
};

/// OHOS RTP sender handle.
///
/// All state is held behind `Arc<Mutex<..>>` so a sender can be cloned cheaply
/// and shared between the peer connection, transceivers, and the public
/// wrapper without losing identity.
#[derive(Clone)]
pub struct RtpSender {
    pub(crate) id: String,
    pub(crate) track: Arc<Mutex<Option<MediaStreamTrack>>>,
    pub(crate) parameters: Arc<Mutex<RtpParameters>>,
}

impl RtpSender {
    /// Build a sender with the given identifier and optional initial track.
    pub(crate) fn new(id: String, track: Option<MediaStreamTrack>) -> Self {
        Self {
            id,
            track: Arc::new(Mutex::new(track)),
            parameters: Arc::new(Mutex::new(RtpParameters {
                codecs: Vec::new(),
                header_extensions: Vec::new(),
                encodings: Vec::new(),
                rtcp: RtcpParameters::default(),
                transaction_id: String::new(),
                mid: String::new(),
                has_degradation_preference: false,
                degradation_preference: 0,
            })),
        }
    }

    pub fn track(&self) -> Option<MediaStreamTrack> {
        self.track.lock().clone()
    }

    pub async fn get_stats(&self) -> Result<Vec<RtcStats>, RtcError> {
        // TODO(ohos): Full implementation with rtc crate stats collection.
        Ok(Vec::new())
    }

    pub fn set_track(&self, track: Option<MediaStreamTrack>) -> Result<(), RtcError> {
        *self.track.lock() = track;
        Ok(())
    }

    pub fn parameters(&self) -> RtpParameters {
        self.parameters.lock().clone()
    }

    pub fn set_parameters(&self, parameters: RtpParameters) -> Result<(), RtcError> {
        *self.parameters.lock() = parameters;
        Ok(())
    }

    /// Stub: OHOS does not yet implement video encoder backend selection.
    pub fn set_video_encoder_backend(&self, _backend: VideoEncoderBackend) {
        // no-op on OHOS
    }

}


/// Stub: OHOS returns an empty encoder list.
/// This is a module-level function (not a method) called by the RtpSender wrapper.
pub fn video_encoder_backend_list() -> Vec<VideoEncoderBackend> {
    Vec::new()
}
