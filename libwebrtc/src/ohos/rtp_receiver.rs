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

use std::sync::Arc;

use parking_lot::Mutex;

use crate::{
    media_stream_track::MediaStreamTrack,
    rtp_parameters::{RtcpParameters, RtpParameters},
    stats::RtcStats,
    RtcError,
};

/// OHOS RTP receiver handle.
#[derive(Clone)]
pub struct RtpReceiver {
    pub(crate) id: String,
    pub(crate) track: Arc<Mutex<Option<MediaStreamTrack>>>,
    pub(crate) parameters: Arc<Mutex<RtpParameters>>,
}

impl RtpReceiver {
    pub(crate) fn new(id: String, track: Option<MediaStreamTrack>) -> Self {
        Self {
            id,
            track: Arc::new(Mutex::new(track)),
            parameters: Arc::new(Mutex::new(RtpParameters {
                codecs: Vec::new(),
                header_extensions: Vec::new(),
                rtcp: RtcpParameters::default(),
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

    pub fn parameters(&self) -> RtpParameters {
        self.parameters.lock().clone()
    }
}
