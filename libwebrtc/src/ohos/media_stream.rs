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

//! OHOS pure-Rust [`MediaStream`].

use std::sync::Arc;

use parking_lot::Mutex;

use crate::{audio_track::RtcAudioTrack, video_track::RtcVideoTrack};

#[derive(Clone)]
pub struct MediaStream {
    pub(crate) id: String,
    pub(crate) audio_tracks: Arc<Mutex<Vec<RtcAudioTrack>>>,
    pub(crate) video_tracks: Arc<Mutex<Vec<RtcVideoTrack>>>,
}

impl MediaStream {
    pub(crate) fn new(id: String) -> Self {
        Self {
            id,
            audio_tracks: Arc::new(Mutex::new(Vec::new())),
            video_tracks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn id(&self) -> String {
        self.id.clone()
    }

    pub fn audio_tracks(&self) -> Vec<RtcAudioTrack> {
        self.audio_tracks.lock().clone()
    }

    pub fn video_tracks(&self) -> Vec<RtcVideoTrack> {
        self.video_tracks.lock().clone()
    }
}
