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

//! OHOS pure-Rust media track shared state.
//!
//! [`MediaStreamTrack`] holds the per-track fields ([id], [kind], [enabled]
//! flag, [state]) that the audio/video track wrappers delegate to via the
//! [`media_stream_track!`] macro defined in the public layer.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use parking_lot::Mutex;

use crate::{
    media_stream_track::{self as mst_pub, RtcTrackState},
    MediaType,
};

#[derive(Clone)]
pub struct MediaStreamTrack {
    pub(crate) id: String,
    pub(crate) kind: MediaType,
    pub(crate) enabled: Arc<AtomicBool>,
    pub(crate) state: Arc<Mutex<RtcTrackState>>,
}

impl MediaStreamTrack {
    pub(crate) fn new(id: String, kind: MediaType) -> Self {
        Self {
            id,
            kind,
            enabled: Arc::new(AtomicBool::new(true)),
            state: Arc::new(Mutex::new(RtcTrackState::Live)),
        }
    }

    pub fn id(&self) -> String {
        self.id.clone()
    }

    pub fn kind(&self) -> MediaType {
        self.kind
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    pub fn set_enabled(&self, enabled: bool) -> bool {
        self.enabled.store(enabled, Ordering::SeqCst);
        true
    }

    pub fn state(&self) -> RtcTrackState {
        *self.state.lock()
    }

    pub(crate) fn set_state(&self, state: RtcTrackState) {
        *self.state.lock() = state;
    }
}

/// Build a top-level [`mst_pub::MediaStreamTrack`] enum from a kind hint and id.
///
/// Used by `RTCRtpReceiver` glue to surface remote tracks as the user-facing
/// audio/video track types. The OHOS implementation just constructs fresh
/// [`MediaStreamTrack`] state since there's no platform handle to wrap.
pub fn new_media_stream_track(id: String, kind: MediaType) -> mst_pub::MediaStreamTrack {
    let track = MediaStreamTrack::new(id, kind);
    match kind {
        MediaType::Audio => mst_pub::MediaStreamTrack::Audio(crate::audio_track::RtcAudioTrack {
            handle: super::audio_track::RtcAudioTrack::new(track),
        }),
        MediaType::Video => mst_pub::MediaStreamTrack::Video(crate::video_track::RtcVideoTrack {
            handle: super::video_track::RtcVideoTrack::new(track),
        }),
        _ => panic!("unsupported media kind for MediaStreamTrack"),
    }
}
