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

//! Room event types and the event loop that forwards [`RoomEvent`]s to ArkTS
//! via a [`ThreadsafeFunction`] callback.

use std::sync::Arc;

use livekit::prelude::*;
use napi_derive_ohos::napi;
use napi_ohos::bindgen_prelude::*;
use napi_ohos::threadsafe_function::{
    ThreadsafeFunction, ThreadsafeFunctionCallMode, UnknownReturnValue,
};
use napi_ohos::Status;
use tokio::sync::mpsc;
use tokio::sync::Mutex;

/// Lightweight participant info delivered with events.
#[napi(object)]
#[derive(Clone)]
pub struct LkParticipantInfo {
    pub identity: String,
    pub sid: String,
    pub name: String,
    pub metadata: String,
    pub is_local: bool,
}

/// Track info delivered with events.
#[napi(object)]
#[derive(Clone)]
pub struct LkTrackInfo {
    /// Track SID.
    pub sid: String,
    /// Track name.
    pub name: String,
    /// `"audio"` or `"video"`.
    pub kind: String,
    /// `"camera"`, `"microphone"`, `"screen_share"`, `"screen_share_audio"`, or `"unknown"`.
    pub source: String,
}

/// Publication info delivered with events.
#[napi(object)]
#[derive(Clone)]
pub struct LkPublicationInfo {
    pub sid: String,
    pub name: String,
    /// `"audio"` or `"video"`.
    pub kind: String,
    pub is_subscribed: bool,
}

/// A room event delivered to ArkTS via the registered callback.
#[napi(object)]
pub struct LkRoomEvent {
    /// Event name, e.g. `"participantConnected"`, `"trackSubscribed"`.
    pub event_type: String,
    /// Participant info (when applicable).
    pub participant: Option<LkParticipantInfo>,
    /// Track info (when applicable).
    pub track: Option<LkTrackInfo>,
    /// Publication info (when applicable).
    pub publication: Option<LkPublicationInfo>,
    /// Data payload (for `dataReceived` events).
    pub data: Option<Buffer>,
    /// Topic (for `dataReceived` events).
    pub topic: Option<String>,
    /// Connection state string (for `connectionStateChanged`).
    pub connection_state: Option<String>,
    /// Disconnect reason (for `disconnected`).
    pub reason: Option<String>,
    /// Metadata value (for `roomMetadataChanged`).
    pub metadata: Option<String>,
}

// ---------------------------------------------------------------------------
// Event loop
// ---------------------------------------------------------------------------

/// ThreadsafeFunction type for event callbacks (Fatal mode, unlimited queue).
pub(crate) type EventCallback =
    ThreadsafeFunction<LkRoomEvent, UnknownReturnValue, LkRoomEvent, Status, false, false, 0>;

/// Spawns an async task that drains `event_rx` and invokes `callback` for each
/// [`RoomEvent`], converting it to an [`LkRoomEvent`] first.
pub(crate) fn start_event_loop(
    event_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<RoomEvent>>>>,
    callback: EventCallback,
) {
    // Use the global runtime managed by napi-ohos. Calling bare `tokio::spawn`
    // from a synchronous `#[napi]` method (which executes on the JS thread
    // without an enclosing runtime context) would panic and abort the app.
    napi_ohos::bindgen_prelude::spawn(async move {
        let mut rx = {
            let mut guard = event_rx.lock().await;
            match guard.take() {
                Some(rx) => rx,
                None => return, // already consumed
            }
        };

        while let Some(event) = rx.recv().await {
            let lk_event = convert_room_event(event);
            callback.call(lk_event, ThreadsafeFunctionCallMode::NonBlocking);
        }
    });
}

// ---------------------------------------------------------------------------
// Conversion
// ---------------------------------------------------------------------------

fn default_event() -> LkRoomEvent {
    LkRoomEvent {
        event_type: String::new(),
        participant: None,
        track: None,
        publication: None,
        data: None,
        topic: None,
        connection_state: None,
        reason: None,
        metadata: None,
    }
}

fn convert_room_event(event: RoomEvent) -> LkRoomEvent {
    match event {
        RoomEvent::ParticipantConnected(p) => LkRoomEvent {
            event_type: "participantConnected".into(),
            participant: Some(remote_participant_info(&p)),
            ..default_event()
        },
        RoomEvent::ParticipantActive(p) => LkRoomEvent {
            event_type: "participantActive".into(),
            participant: Some(remote_participant_info(&p)),
            ..default_event()
        },
        RoomEvent::ParticipantDisconnected(p) => LkRoomEvent {
            event_type: "participantDisconnected".into(),
            participant: Some(remote_participant_info(&p)),
            ..default_event()
        },
        RoomEvent::TrackSubscribed { track, publication, participant } => LkRoomEvent {
            event_type: "trackSubscribed".into(),
            participant: Some(remote_participant_info(&participant)),
            track: Some(track_info_from_remote(&track)),
            publication: Some(publication_info_from_remote(&publication)),
            ..default_event()
        },
        RoomEvent::TrackUnsubscribed { track, publication, participant } => LkRoomEvent {
            event_type: "trackUnsubscribed".into(),
            participant: Some(remote_participant_info(&participant)),
            track: Some(track_info_from_remote(&track)),
            publication: Some(publication_info_from_remote(&publication)),
            ..default_event()
        },
        RoomEvent::TrackPublished { publication, participant } => LkRoomEvent {
            event_type: "trackPublished".into(),
            participant: Some(remote_participant_info(&participant)),
            publication: Some(publication_info_from_remote(&publication)),
            ..default_event()
        },
        RoomEvent::TrackUnpublished { publication, participant } => LkRoomEvent {
            event_type: "trackUnpublished".into(),
            participant: Some(remote_participant_info(&participant)),
            publication: Some(publication_info_from_remote(&publication)),
            ..default_event()
        },
        RoomEvent::TrackMuted { participant, publication } => LkRoomEvent {
            event_type: "trackMuted".into(),
            participant: Some(participant_info(&participant)),
            publication: Some(publication_info(&publication)),
            ..default_event()
        },
        RoomEvent::TrackUnmuted { participant, publication } => LkRoomEvent {
            event_type: "trackUnmuted".into(),
            participant: Some(participant_info(&participant)),
            publication: Some(publication_info(&publication)),
            ..default_event()
        },
        RoomEvent::DataReceived { payload, topic, kind: _, participant } => LkRoomEvent {
            event_type: "dataReceived".into(),
            participant: participant.as_ref().map(remote_participant_info),
            data: Some(Buffer::from(payload.as_ref().clone())),
            topic,
            ..default_event()
        },
        RoomEvent::ConnectionStateChanged(state) => LkRoomEvent {
            event_type: "connectionStateChanged".into(),
            connection_state: Some(connection_state_str(state).into()),
            ..default_event()
        },
        RoomEvent::Connected { .. } => LkRoomEvent {
            event_type: "connected".into(),
            ..default_event()
        },
        RoomEvent::Disconnected { reason } => LkRoomEvent {
            event_type: "disconnected".into(),
            reason: Some(format!("{reason:?}")),
            ..default_event()
        },
        RoomEvent::Reconnecting => LkRoomEvent {
            event_type: "reconnecting".into(),
            ..default_event()
        },
        RoomEvent::Reconnected => LkRoomEvent {
            event_type: "reconnected".into(),
            ..default_event()
        },
        RoomEvent::RoomMetadataChanged { metadata, .. } => LkRoomEvent {
            event_type: "roomMetadataChanged".into(),
            metadata: Some(metadata),
            ..default_event()
        },
        RoomEvent::ActiveSpeakersChanged { .. } => LkRoomEvent {
            event_type: "activeSpeakersChanged".into(),
            ..default_event()
        },
        RoomEvent::ParticipantMetadataChanged { participant, metadata, .. } => LkRoomEvent {
            event_type: "participantMetadataChanged".into(),
            participant: Some(participant_info(&participant)),
            metadata: Some(metadata),
            ..default_event()
        },
        RoomEvent::ParticipantNameChanged { participant, name, .. } => LkRoomEvent {
            event_type: "participantNameChanged".into(),
            participant: Some(participant_info(&participant)),
            metadata: Some(name),
            ..default_event()
        },
        RoomEvent::LocalTrackPublished { participant, .. } => LkRoomEvent {
            event_type: "localTrackPublished".into(),
            participant: Some(local_participant_info(&participant)),
            ..default_event()
        },
        RoomEvent::LocalTrackUnpublished { participant, .. } => LkRoomEvent {
            event_type: "localTrackUnpublished".into(),
            participant: Some(local_participant_info(&participant)),
            ..default_event()
        },
        _ => LkRoomEvent {
            event_type: "unknown".into(),
            ..default_event()
        },
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn connection_state_str(state: ConnectionState) -> &'static str {
    match state {
        ConnectionState::Connected => "connected",
        ConnectionState::Disconnected => "disconnected",
        ConnectionState::Reconnecting => "reconnecting",
    }
}

fn remote_participant_info(p: &RemoteParticipant) -> LkParticipantInfo {
    LkParticipantInfo {
        identity: p.identity().to_string(),
        sid: p.sid().to_string(),
        name: p.name(),
        metadata: p.metadata(),
        is_local: false,
    }
}

fn local_participant_info(p: &LocalParticipant) -> LkParticipantInfo {
    LkParticipantInfo {
        identity: p.identity().to_string(),
        sid: p.sid().to_string(),
        name: p.name(),
        metadata: p.metadata(),
        is_local: true,
    }
}

fn participant_info(p: &Participant) -> LkParticipantInfo {
    LkParticipantInfo {
        identity: p.identity().to_string(),
        sid: p.sid().to_string(),
        name: p.name(),
        metadata: p.metadata(),
        is_local: matches!(p, Participant::Local(_)),
    }
}

fn track_kind_str(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Audio => "audio",
        TrackKind::Video => "video",
    }
}

fn track_source_str(source: TrackSource) -> &'static str {
    match source {
        TrackSource::Camera => "camera",
        TrackSource::Microphone => "microphone",
        TrackSource::Screenshare => "screen_share",
        TrackSource::ScreenshareAudio => "screen_share_audio",
        TrackSource::Unknown => "unknown",
    }
}

fn track_info_from_remote(track: &RemoteTrack) -> LkTrackInfo {
    LkTrackInfo {
        sid: track.sid().to_string(),
        name: track.name(),
        kind: track_kind_str(track.kind()).into(),
        source: track_source_str(track.source()).into(),
    }
}

fn publication_info_from_remote(pub_: &RemoteTrackPublication) -> LkPublicationInfo {
    LkPublicationInfo {
        sid: pub_.sid().to_string(),
        name: pub_.name(),
        kind: track_kind_str(pub_.kind()).into(),
        is_subscribed: pub_.is_subscribed(),
    }
}

fn publication_info(pub_: &TrackPublication) -> LkPublicationInfo {
    LkPublicationInfo {
        sid: pub_.sid().to_string(),
        name: pub_.name(),
        kind: track_kind_str(pub_.kind()).into(),
        is_subscribed: matches!(pub_, TrackPublication::Remote(r) if r.is_subscribed()),
    }
}
