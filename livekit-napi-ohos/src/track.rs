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

//! ArkTS-facing wrappers around LiveKit tracks and publications.

use livekit::prelude::*;
use livekit::webrtc::audio_source::RtcAudioSource;
use livekit::webrtc::video_source::RtcVideoSource;
use napi_derive_ohos::napi;
#[allow(unused_imports)]
use napi_ohos::bindgen_prelude::*;

use crate::audio_source::LkAudioSource;
use crate::video_source::LkVideoSource;

fn track_kind_str(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Audio => "audio",
        TrackKind::Video => "video",
    }
}

// ===== Local Audio Track =====

/// Local audio track handle.
#[napi]
pub struct LkLocalAudioTrack {
    pub(crate) inner: Option<LocalAudioTrack>,
}

impl LkLocalAudioTrack {
    #[allow(dead_code)]
    pub(crate) fn from_inner(inner: LocalAudioTrack) -> Self {
        Self { inner: Some(inner) }
    }
}

#[napi]
impl LkLocalAudioTrack {
    /// Placeholder constructor required by napi-ohos.
    #[napi(constructor)]
    pub fn new() -> Self {
        Self { inner: None }
    }

    /// Create a local audio track that pulls audio frames from the given
    /// [`LkAudioSource`].
    ///
    /// The returned track can be passed to
    /// [`LkLocalParticipant::publish_audio_track`] to publish it to the room.
    #[napi(factory)]
    pub fn create_track(name: String, source: &LkAudioSource) -> Self {
        let rtc_source = RtcAudioSource::Native(source.inner.clone());
        let track = LocalAudioTrack::create_audio_track(&name, rtc_source);
        Self { inner: Some(track) }
    }

    /// Server-assigned track SID.
    #[napi(getter)]
    pub fn sid(&self) -> String {
        self.inner
            .as_ref()
            .map(|t| t.sid().to_string())
            .unwrap_or_default()
    }

    /// Track display name.
    #[napi(getter)]
    pub fn name(&self) -> String {
        self.inner.as_ref().map(|t| t.name()).unwrap_or_default()
    }

    /// Track kind, always `"audio"`.
    #[napi(getter)]
    pub fn kind(&self) -> String {
        "audio".to_string()
    }

    /// Whether the track is currently muted.
    #[napi(getter)]
    pub fn is_muted(&self) -> bool {
        self.inner.as_ref().map(|t| t.is_muted()).unwrap_or(false)
    }

    /// Mute or unmute the track.
    #[napi]
    pub fn set_muted(&self, muted: bool) {
        if let Some(inner) = self.inner.as_ref() {
            if muted {
                inner.mute();
            } else {
                inner.unmute();
            }
        }
    }
}

// ===== Local Video Track =====

/// Local video track handle.
#[napi]
pub struct LkLocalVideoTrack {
    pub(crate) inner: Option<LocalVideoTrack>,
}

impl LkLocalVideoTrack {
    #[allow(dead_code)]
    pub(crate) fn from_inner(inner: LocalVideoTrack) -> Self {
        Self { inner: Some(inner) }
    }
}

#[napi]
impl LkLocalVideoTrack {
    /// Placeholder constructor required by napi-ohos.
    #[napi(constructor)]
    pub fn new() -> Self {
        Self { inner: None }
    }

    /// Create a local video track that pulls video frames from the given
    /// [`LkVideoSource`].
    ///
    /// The returned track can be passed to
    /// [`LkLocalParticipant::publish_video_track`] to publish it to the room.
    #[napi(factory)]
    pub fn create_track(name: String, source: &LkVideoSource) -> Self {
        let rtc_source = RtcVideoSource::Native(source.inner.clone());
        let track = LocalVideoTrack::create_video_track(&name, rtc_source);
        Self { inner: Some(track) }
    }

    /// Server-assigned track SID.
    #[napi(getter)]
    pub fn sid(&self) -> String {
        self.inner
            .as_ref()
            .map(|t| t.sid().to_string())
            .unwrap_or_default()
    }

    /// Track display name.
    #[napi(getter)]
    pub fn name(&self) -> String {
        self.inner.as_ref().map(|t| t.name()).unwrap_or_default()
    }

    /// Track kind, always `"video"`.
    #[napi(getter)]
    pub fn kind(&self) -> String {
        "video".to_string()
    }

    /// Whether the track is currently muted.
    #[napi(getter)]
    pub fn is_muted(&self) -> bool {
        self.inner.as_ref().map(|t| t.is_muted()).unwrap_or(false)
    }

    /// Mute or unmute the track.
    #[napi]
    pub fn set_muted(&self, muted: bool) {
        if let Some(inner) = self.inner.as_ref() {
            if muted {
                inner.mute();
            } else {
                inner.unmute();
            }
        }
    }
}

// ===== Remote Audio Track =====

/// Remote audio track handle.
#[napi]
pub struct LkRemoteAudioTrack {
    pub(crate) inner: Option<RemoteAudioTrack>,
}

impl LkRemoteAudioTrack {
    #[allow(dead_code)]
    pub(crate) fn from_inner(inner: RemoteAudioTrack) -> Self {
        Self { inner: Some(inner) }
    }
}

#[napi]
impl LkRemoteAudioTrack {
    /// Placeholder constructor required by napi-ohos.
    #[napi(constructor)]
    pub fn new() -> Self {
        Self { inner: None }
    }

    /// Server-assigned track SID.
    #[napi(getter)]
    pub fn sid(&self) -> String {
        self.inner
            .as_ref()
            .map(|t| t.sid().to_string())
            .unwrap_or_default()
    }

    /// Track display name.
    #[napi(getter)]
    pub fn name(&self) -> String {
        self.inner.as_ref().map(|t| t.name()).unwrap_or_default()
    }

    /// Track kind, always `"audio"`.
    #[napi(getter)]
    pub fn kind(&self) -> String {
        "audio".to_string()
    }

    /// Whether the track is currently muted.
    #[napi(getter)]
    pub fn is_muted(&self) -> bool {
        self.inner.as_ref().map(|t| t.is_muted()).unwrap_or(false)
    }
}

// ===== Remote Video Track =====

/// Remote video track handle.
#[napi]
pub struct LkRemoteVideoTrack {
    pub(crate) inner: Option<RemoteVideoTrack>,
}

impl LkRemoteVideoTrack {
    #[allow(dead_code)]
    pub(crate) fn from_inner(inner: RemoteVideoTrack) -> Self {
        Self { inner: Some(inner) }
    }
}

#[napi]
impl LkRemoteVideoTrack {
    /// Placeholder constructor required by napi-ohos.
    #[napi(constructor)]
    pub fn new() -> Self {
        Self { inner: None }
    }

    /// Server-assigned track SID.
    #[napi(getter)]
    pub fn sid(&self) -> String {
        self.inner
            .as_ref()
            .map(|t| t.sid().to_string())
            .unwrap_or_default()
    }

    /// Track display name.
    #[napi(getter)]
    pub fn name(&self) -> String {
        self.inner.as_ref().map(|t| t.name()).unwrap_or_default()
    }

    /// Track kind, always `"video"`.
    #[napi(getter)]
    pub fn kind(&self) -> String {
        "video".to_string()
    }

    /// Whether the track is currently muted.
    #[napi(getter)]
    pub fn is_muted(&self) -> bool {
        self.inner.as_ref().map(|t| t.is_muted()).unwrap_or(false)
    }
}

// ===== Local Track Publication =====

/// Handle to a track publication owned by the local participant.
#[napi]
pub struct LkLocalTrackPublication {
    pub(crate) inner: Option<LocalTrackPublication>,
}

impl LkLocalTrackPublication {
    #[allow(dead_code)]
    pub(crate) fn from_inner(inner: LocalTrackPublication) -> Self {
        Self { inner: Some(inner) }
    }
}

#[napi]
impl LkLocalTrackPublication {
    /// Placeholder constructor required by napi-ohos.
    #[napi(constructor)]
    pub fn new() -> Self {
        Self { inner: None }
    }

    /// Server-assigned track SID.
    #[napi(getter)]
    pub fn sid(&self) -> String {
        self.inner
            .as_ref()
            .map(|p| p.sid().to_string())
            .unwrap_or_default()
    }

    /// Track display name.
    #[napi(getter)]
    pub fn name(&self) -> String {
        self.inner.as_ref().map(|p| p.name()).unwrap_or_default()
    }

    /// Track kind: `"audio"` or `"video"`.
    #[napi(getter)]
    pub fn kind(&self) -> String {
        self.inner
            .as_ref()
            .map(|p| track_kind_str(p.kind()).to_string())
            .unwrap_or_default()
    }

    /// Whether the publication is currently muted.
    #[napi(getter)]
    pub fn is_muted(&self) -> bool {
        self.inner.as_ref().map(|p| p.is_muted()).unwrap_or(false)
    }

    /// Mute or unmute the publication.
    #[napi]
    pub fn set_muted(&self, muted: bool) {
        if let Some(inner) = self.inner.as_ref() {
            if muted {
                inner.mute();
            } else {
                inner.unmute();
            }
        }
    }
}

// ===== Remote Track Publication =====

/// Handle to a track publication owned by a remote participant.
#[napi]
pub struct LkRemoteTrackPublication {
    pub(crate) inner: Option<RemoteTrackPublication>,
}

impl LkRemoteTrackPublication {
    #[allow(dead_code)]
    pub(crate) fn from_inner(inner: RemoteTrackPublication) -> Self {
        Self { inner: Some(inner) }
    }
}

#[napi]
impl LkRemoteTrackPublication {
    /// Placeholder constructor required by napi-ohos.
    #[napi(constructor)]
    pub fn new() -> Self {
        Self { inner: None }
    }

    /// Server-assigned track SID.
    #[napi(getter)]
    pub fn sid(&self) -> String {
        self.inner
            .as_ref()
            .map(|p| p.sid().to_string())
            .unwrap_or_default()
    }

    /// Track display name.
    #[napi(getter)]
    pub fn name(&self) -> String {
        self.inner.as_ref().map(|p| p.name()).unwrap_or_default()
    }

    /// Track kind: `"audio"` or `"video"`.
    #[napi(getter)]
    pub fn kind(&self) -> String {
        self.inner
            .as_ref()
            .map(|p| track_kind_str(p.kind()).to_string())
            .unwrap_or_default()
    }

    /// Whether this remote track is currently subscribed.
    #[napi(getter)]
    pub fn is_subscribed(&self) -> bool {
        self.inner
            .as_ref()
            .map(|p| p.is_subscribed())
            .unwrap_or(false)
    }

    /// Whether the underlying track is muted on the publisher side.
    #[napi(getter)]
    pub fn is_muted(&self) -> bool {
        self.inner.as_ref().map(|p| p.is_muted()).unwrap_or(false)
    }

    /// Subscribe or unsubscribe from this remote track.
    #[napi]
    pub fn set_subscribed(&self, subscribed: bool) {
        if let Some(inner) = self.inner.as_ref() {
            inner.set_subscribed(subscribed);
        }
    }
}
