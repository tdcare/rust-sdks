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

//! ArkTS-facing wrappers around LiveKit participants.

use std::collections::HashMap;

use livekit::options::{TrackPublishOptions, VideoCodec};
use livekit::prelude::*;
use napi_derive_ohos::napi;
use napi_ohos::bindgen_prelude::*;

use crate::data_track::LkPerformRpcData;
use crate::track::{LkLocalAudioTrack, LkLocalTrackPublication, LkLocalVideoTrack};

/// Options for publishing data to other participants.
#[napi(object)]
pub struct LkDataPublishOptions {
    /// Topic for the data message.
    pub topic: Option<String>,
    /// Whether the data should be sent reliably (TCP-like). Defaults to `true`.
    pub reliable: Option<bool>,
    /// Identities of the participants to send data to. Empty means broadcast.
    pub destination_identities: Option<Vec<String>>,
}

fn not_initialized() -> Error {
    Error::from_reason("participant is not initialized")
}

/// Local participant handle.
#[napi]
pub struct LkLocalParticipant {
    pub(crate) inner: Option<LocalParticipant>,
}

impl LkLocalParticipant {
    pub(crate) fn from_inner(inner: LocalParticipant) -> Self {
        Self { inner: Some(inner) }
    }
}

#[napi]
impl LkLocalParticipant {
    /// Placeholder constructor required by napi-ohos. The instance is empty
    /// and will return default values until populated by the SDK internally.
    #[napi(constructor)]
    pub fn new() -> Self {
        Self { inner: None }
    }

    /// Participant identity.
    #[napi(getter)]
    pub fn identity(&self) -> String {
        self.inner
            .as_ref()
            .map(|p| p.identity().to_string())
            .unwrap_or_default()
    }

    /// Participant display name.
    #[napi(getter)]
    pub fn name(&self) -> String {
        self.inner.as_ref().map(|p| p.name()).unwrap_or_default()
    }

    /// Server-assigned participant SID.
    #[napi(getter)]
    pub fn sid(&self) -> String {
        self.inner
            .as_ref()
            .map(|p| p.sid().to_string())
            .unwrap_or_default()
    }

    /// Participant metadata set via server API or `setMetadata`.
    #[napi(getter)]
    pub fn metadata(&self) -> String {
        self.inner.as_ref().map(|p| p.metadata()).unwrap_or_default()
    }

    /// Key-value attributes associated with this participant.
    #[napi(getter)]
    pub fn attributes(&self) -> HashMap<String, String> {
        self.inner
            .as_ref()
            .map(|p| p.attributes())
            .unwrap_or_default()
    }

    /// Whether the participant is currently speaking.
    #[napi(getter)]
    pub fn is_speaking(&self) -> bool {
        self.inner.as_ref().map(|p| p.is_speaking()).unwrap_or(false)
    }

    /// Unpublish a track by its SID.
    #[napi]
    pub async fn unpublish_track(&self, track_sid: String) -> Result<()> {
        let inner = self.inner.as_ref().ok_or_else(not_initialized)?;
        let sid: TrackSid = track_sid
            .try_into()
            .map_err(|s: String| Error::from_reason(format!("invalid track sid: {s}")))?;
        inner
            .unpublish_track(&sid)
            .await
            .map_err(|e| Error::from_reason(format!("unpublish failed: {e}")))?;
        Ok(())
    }

    /// Publish a local audio track to the room.
    ///
    /// Returns a [`LkLocalTrackPublication`] handle for the resulting
    /// publication.
    #[napi]
    pub async fn publish_audio_track(
        &self,
        track: &LkLocalAudioTrack,
    ) -> Result<LkLocalTrackPublication> {
        let participant = self.inner.as_ref().ok_or_else(not_initialized)?;
        let local_track = track
            .inner
            .as_ref()
            .ok_or_else(|| Error::from_reason("audio track is not initialized"))?
            .clone();

        let options = TrackPublishOptions {
            source: TrackSource::Microphone,
            ..Default::default()
        };

        let publication = participant
            .publish_track(LocalTrack::Audio(local_track), options)
            .await
            .map_err(|e| Error::from_reason(format!("publish_audio_track failed: {e}")))?;
        Ok(LkLocalTrackPublication::from_inner(publication))
    }

    /// Publish a local video track to the room.
    ///
    /// Returns a [`LkLocalTrackPublication`] handle for the resulting
    /// publication. H.264 hardware encoder is preferred; falls back to
    /// VP8 software encoder automatically when H.264 is unavailable.
    #[napi]
    pub async fn publish_video_track(
        &self,
        track: &LkLocalVideoTrack,
    ) -> Result<LkLocalTrackPublication> {
        let participant = self.inner.as_ref().ok_or_else(not_initialized)?;
        let local_track = track
            .inner
            .as_ref()
            .ok_or_else(|| Error::from_reason("video track is not initialized"))?
            .clone();

        let options = TrackPublishOptions {
            video_codec: VideoCodec::H264,
            source: TrackSource::Camera,
            simulcast: false,
            ..Default::default()
        };

        let publication = participant
            .publish_track(LocalTrack::Video(local_track), options)
            .await
            .map_err(|e| Error::from_reason(format!("publish_video_track failed: {e}")))?;
        Ok(LkLocalTrackPublication::from_inner(publication))
    }

    /// Send data to other participants.
    #[napi]
    pub async fn publish_data(
        &self,
        payload: Buffer,
        options: Option<LkDataPublishOptions>,
    ) -> Result<()> {
        let inner = self.inner.as_ref().ok_or_else(not_initialized)?;
        let reliable = options.as_ref().and_then(|o| o.reliable).unwrap_or(true);
        let topic = options.as_ref().and_then(|o| o.topic.clone());
        let destination_identities: Vec<ParticipantIdentity> = options
            .as_ref()
            .and_then(|o| o.destination_identities.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.into())
            .collect();

        let packet = DataPacket {
            payload: payload.to_vec(),
            topic,
            reliable,
            destination_identities,
        };

        inner
            .publish_data(packet)
            .await
            .map_err(|e| Error::from_reason(format!("publish_data failed: {e}")))
    }

    /// Set participant metadata.
    #[napi]
    pub async fn set_metadata(&self, metadata: String) -> Result<()> {
        let inner = self.inner.as_ref().ok_or_else(not_initialized)?;
        inner
            .set_metadata(metadata)
            .await
            .map_err(|e| Error::from_reason(format!("set_metadata failed: {e}")))
    }

    /// Set participant display name.
    #[napi]
    pub async fn set_name(&self, name: String) -> Result<()> {
        let inner = self.inner.as_ref().ok_or_else(not_initialized)?;
        inner
            .set_name(name)
            .await
            .map_err(|e| Error::from_reason(format!("set_name failed: {e}")))
    }

    /// Set participant attributes (key-value pairs).
    #[napi]
    pub async fn set_attributes(&self, attributes: HashMap<String, String>) -> Result<()> {
        let inner = self.inner.as_ref().ok_or_else(not_initialized)?;
        inner
            .set_attributes(attributes)
            .await
            .map_err(|e| Error::from_reason(format!("set_attributes failed: {e}")))
    }

    /// Perform an RPC call to a remote participant.
    ///
    /// Returns the payload string returned by the remote handler. If the
    /// remote method raises an error, this method returns an `Error` with the
    /// RPC failure details. If `response_timeout_ms` is omitted, the SDK
    /// default (15 seconds) is used.
    #[napi]
    pub async fn perform_rpc(&self, data: LkPerformRpcData) -> Result<String> {
        let inner = self.inner.as_ref().ok_or_else(not_initialized)?;
        let mut rpc_data = PerformRpcData {
            destination_identity: data.destination_identity,
            method: data.method,
            payload: data.payload,
            ..Default::default()
        };
        if let Some(ms) = data.response_timeout_ms {
            rpc_data.response_timeout = std::time::Duration::from_millis(ms as u64);
        }
        inner
            .perform_rpc(rpc_data)
            .await
            .map_err(|e| Error::from_reason(format!("rpc failed: {e}")))
    }

    /// Unregister a previously registered RPC method handler.
    #[napi]
    pub fn unregister_rpc_method(&self, method: String) {
        if let Some(inner) = self.inner.as_ref() {
            inner.unregister_rpc_method(method);
        }
    }

    // TODO(task-31): implement register_rpc_method.
    // It requires bridging a JS callback that returns a Promise into a Rust
    // `Fn(RpcInvocationData) -> Pin<Box<dyn Future<Output = Result<String, RpcError>> + Send>>`,
    // which needs careful NAPI ThreadsafeFunction handling.
}

/// Remote participant handle.
#[napi]
pub struct LkRemoteParticipant {
    pub(crate) inner: Option<RemoteParticipant>,
}

impl LkRemoteParticipant {
    pub(crate) fn from_inner(inner: RemoteParticipant) -> Self {
        Self { inner: Some(inner) }
    }
}

#[napi]
impl LkRemoteParticipant {
    /// Placeholder constructor required by napi-ohos. The instance is empty
    /// and will return default values until populated by the SDK internally.
    #[napi(constructor)]
    pub fn new() -> Self {
        Self { inner: None }
    }

    /// Participant identity.
    #[napi(getter)]
    pub fn identity(&self) -> String {
        self.inner
            .as_ref()
            .map(|p| p.identity().to_string())
            .unwrap_or_default()
    }

    /// Participant display name.
    #[napi(getter)]
    pub fn name(&self) -> String {
        self.inner.as_ref().map(|p| p.name()).unwrap_or_default()
    }

    /// Server-assigned participant SID.
    #[napi(getter)]
    pub fn sid(&self) -> String {
        self.inner
            .as_ref()
            .map(|p| p.sid().to_string())
            .unwrap_or_default()
    }

    /// Participant metadata.
    #[napi(getter)]
    pub fn metadata(&self) -> String {
        self.inner.as_ref().map(|p| p.metadata()).unwrap_or_default()
    }

    /// Key-value attributes associated with this participant.
    #[napi(getter)]
    pub fn attributes(&self) -> HashMap<String, String> {
        self.inner
            .as_ref()
            .map(|p| p.attributes())
            .unwrap_or_default()
    }

    /// Whether the participant is currently speaking.
    #[napi(getter)]
    pub fn is_speaking(&self) -> bool {
        self.inner.as_ref().map(|p| p.is_speaking()).unwrap_or(false)
    }
}
