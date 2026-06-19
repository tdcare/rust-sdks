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

//! ArkTS-facing wrapper around [`livekit::Room`].

use std::sync::Arc;

use livekit::{ConnectionState, RoomEvent, RoomOptions};
use livekit::prelude::*;
use napi_derive_ohos::napi;
use napi_ohos::bindgen_prelude::*;
use napi_ohos::threadsafe_function::UnknownReturnValue;
use parking_lot::Mutex as ParkingMutex;
use tokio::sync::{mpsc, Mutex};

use crate::events::LkRoomEvent;
use crate::participant::{LkLocalParticipant, LkRemoteParticipant};
use crate::track::{LkRemoteAudioTrack, LkRemoteVideoTrack};

/// Room options passed from ArkTS.
#[napi(object)]
pub struct LkRoomOptions {
    pub auto_subscribe: Option<bool>,
    pub adaptive_stream: Option<bool>,
    pub dynacast: Option<bool>,
}

/// LiveKit `Room` handle exposed to ArkTS.
///
/// Uses a two-phase construction pattern: call `new()` first, then `connect()`.
#[napi]
pub struct LkRoom {
    inner: Arc<ParkingMutex<Option<Arc<livekit::Room>>>>,
    event_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<RoomEvent>>>>,
}

#[napi]
impl LkRoom {
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ParkingMutex::new(None)),
            event_rx: Arc::new(Mutex::new(None)),
        }
    }

    /// Connect to a LiveKit server. Must be called after construction.
    #[napi]
    pub async fn connect(
        &self,
        url: String,
        token: String,
        options: Option<LkRoomOptions>,
    ) -> Result<()> {
        crate::init_logger();
        log::info!("LkRoom::connect called, url={}, token_len={}", url, token.len());

        let mut room_options = RoomOptions::default();
        if let Some(opts) = options {
            if let Some(v) = opts.auto_subscribe {
                room_options.auto_subscribe = v;
            }
            if let Some(v) = opts.adaptive_stream {
                room_options.adaptive_stream = v;
            }
            if let Some(v) = opts.dynacast {
                room_options.dynacast = v;
            }
        }

        let (room, event_rx) = livekit::Room::connect(&url, &token, room_options)
            .await
            .map_err(|e| {
                let msg = format!("connect failed: {}", e);
                log::error!("Room connect failed: {:?}", e);
                Error::from_reason(msg)
            })?;

        log::info!("Room connected successfully, name={}", room.name());
        *self.inner.lock() = Some(Arc::new(room));
        *self.event_rx.lock().await = Some(event_rx);
        Ok(())
    }

    /// Disconnect from the room.
    #[napi]
    pub async fn disconnect(&self) -> Result<()> {
        log::info!("LkRoom::disconnect called");
        let room = self.inner.lock().clone();
        match room {
            Some(r) => r
                .close()
                .await
                .map_err(|e| Error::from_reason(format!("disconnect failed: {e}"))),
            None => Ok(()),
        }
    }

    /// Returns a clone of the inner Arc<Room> if connected.
    fn get_room(&self) -> Option<Arc<livekit::Room>> {
        self.inner.lock().clone()
    }

    /// Room name.
    #[napi(getter)]
    pub fn name(&self) -> String {
        self.inner
            .lock()
            .as_ref()
            .map(|r| r.name())
            .unwrap_or_default()
    }

    /// Room SID. Returns `null` until the server has assigned one.
    #[napi(getter)]
    pub fn sid(&self) -> Option<String> {
        self.inner
            .lock()
            .as_ref()
            .and_then(|r| r.maybe_sid().map(|s| s.to_string()))
    }

    /// Room metadata.
    #[napi(getter)]
    pub fn metadata(&self) -> String {
        self.inner
            .lock()
            .as_ref()
            .map(|r| r.metadata())
            .unwrap_or_default()
    }

    /// Connection state: `"connected"`, `"disconnected"`, or `"reconnecting"`.
    #[napi(getter)]
    pub fn connection_state(&self) -> &'static str {
        let guard = self.inner.lock();
        match guard.as_ref() {
            Some(r) => match r.connection_state() {
                ConnectionState::Connected => "connected",
                ConnectionState::Disconnected => "disconnected",
                ConnectionState::Reconnecting => "reconnecting",
            },
            None => "disconnected",
        }
    }

    /// Local participant handle.
    #[napi(getter)]
    pub fn local_participant(&self) -> Option<LkLocalParticipant> {
        self.inner
            .lock()
            .as_ref()
            .map(|r| LkLocalParticipant::from_inner(r.local_participant()))
    }

    /// Snapshot of all remote participants in the room.
    #[napi(getter)]
    pub fn remote_participants(&self) -> Vec<LkRemoteParticipant> {
        let guard = self.inner.lock();
        match guard.as_ref() {
            Some(r) => r
                .remote_participants()
                .values()
                .cloned()
                .map(LkRemoteParticipant::from_inner)
                .collect(),
            None => Vec::new(),
        }
    }

    /// Look up a remote video track by participant identity and track SID.
    ///
    /// Returns a real native [`LkRemoteVideoTrack`] that can be passed to
    /// `LkVideoStream.fromTrack()`. Returns `null` when the participant or
    /// track cannot be found (e.g. the track is audio-only or has not been
    /// subscribed yet).
    #[napi]
    pub fn get_remote_video_track(
        &self,
        participant_identity: String,
        track_sid: String,
    ) -> Option<LkRemoteVideoTrack> {
        let guard = self.inner.lock();
        let room = guard.as_ref()?;

        let identity: ParticipantIdentity = participant_identity.into();
        let participant = room.remote_participants().get(&identity)?.clone();

        let tsid: TrackSid = track_sid.try_into().ok()?;
        let publication = participant.get_track_publication(&tsid)?;
        let remote_track = publication.track()?;

        match remote_track {
            RemoteTrack::Video(video_track) => {
                Some(LkRemoteVideoTrack::from_inner(video_track))
            }
            _ => None,
        }
    }

    /// Look up a remote audio track by participant identity and track SID.
    ///
    /// Returns a native [`LkRemoteAudioTrack`] that can be passed to
    /// `LkAudioStream.fromTrack()`. Returns `null` when the participant or
    /// track cannot be found (e.g. the track is video-only or has not been
    /// subscribed yet).
    #[napi]
    pub fn get_remote_audio_track(
        &self,
        participant_identity: String,
        track_sid: String,
    ) -> Option<LkRemoteAudioTrack> {
        let guard = self.inner.lock();
        let room = guard.as_ref()?;

        let identity: ParticipantIdentity = participant_identity.into();
        let participant = room.remote_participants().get(&identity)?.clone();

        let tsid: TrackSid = track_sid.try_into().ok()?;
        let publication = participant.get_track_publication(&tsid)?;
        let remote_track = publication.track()?;

        match remote_track {
            RemoteTrack::Audio(audio_track) => {
                Some(LkRemoteAudioTrack::from_inner(audio_track))
            }
            _ => None,
        }
    }

    /// Get all subscribed audio track SIDs for a given remote participant.
    #[napi]
    pub fn get_remote_audio_track_sids(&self, participant_identity: String) -> Vec<String> {
        let guard = self.inner.lock();
        let room = match guard.as_ref() {
            Some(r) => r,
            None => return Vec::new(),
        };

        let identity: ParticipantIdentity = participant_identity.into();
        let participant = match room.remote_participants().get(&identity) {
            Some(p) => p.clone(),
            None => return Vec::new(),
        };

        participant
            .track_publications()
            .values()
            .filter_map(|pub_| {
                let track = pub_.track()?;
                match track {
                    RemoteTrack::Audio(_) => Some(pub_.sid().to_string()),
                    _ => None,
                }
            })
            .collect()
    }

    /// Register an event callback. Events will be delivered asynchronously.
    ///
    /// Call this once after connect to start receiving room events.
    #[napi(ts_args_type = "callback: (event: LkRoomEvent) => void")]
    pub fn on(
        &self,
        callback: Function<'_, LkRoomEvent, UnknownReturnValue>,
    ) -> Result<()> {
        let tsfn: crate::events::EventCallback = callback
            .build_threadsafe_function::<LkRoomEvent>()
            .build_callback(|ctx| Ok(ctx.value))?;
        crate::events::start_event_loop(self.event_rx.clone(), tsfn);
        Ok(())
    }

    /// Get all remote participant identities currently in the room.
    #[napi]
    pub fn get_remote_participant_identities(&self) -> Vec<String> {
        let guard = self.inner.lock();
        match guard.as_ref() {
            Some(room) => room
                .remote_participants()
                .keys()
                .map(|id| id.as_str().to_string())
                .collect(),
            None => Vec::new(),
        }
    }

    /// Get all subscribed video track SIDs for a given remote participant.
    #[napi]
    pub fn get_remote_video_track_sids(&self, participant_identity: String) -> Vec<String> {
        let guard = self.inner.lock();
        let room = match guard.as_ref() {
            Some(r) => r,
            None => return Vec::new(),
        };

        let identity: ParticipantIdentity = participant_identity.into();
        let participant = match room.remote_participants().get(&identity) {
            Some(p) => p.clone(),
            None => return Vec::new(),
        };

        participant
            .track_publications()
            .values()
            .filter_map(|pub_| {
                let track = pub_.track()?;
                match track {
                    RemoteTrack::Video(_) => Some(pub_.sid().to_string()),
                    _ => None,
                }
            })
            .collect()
    }
}
