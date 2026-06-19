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

//! Data track and RPC type definitions for the LiveKit NAPI bindings.
//!
//! The actual data publishing method lives on
//! [`crate::participant::LkLocalParticipant::publish_data`]. The companion
//! [`crate::participant::LkDataPublishOptions`] type defines the publish
//! options. This module only carries the supporting RPC types.

use napi_derive_ohos::napi;

/// Parameters for performing an RPC call to a remote participant.
#[napi(object)]
pub struct LkPerformRpcData {
    /// Identity of the destination participant.
    pub destination_identity: String,
    /// RPC method name.
    pub method: String,
    /// Payload string (typically JSON).
    pub payload: String,
    /// Response timeout in milliseconds. Defaults to 15000 if not provided.
    pub response_timeout_ms: Option<u32>,
}

/// RPC error returned when a remote method call fails.
#[napi(object)]
pub struct LkRpcError {
    /// Error code.
    pub code: u32,
    /// Error message.
    pub message: String,
    /// Optional additional data.
    pub data: Option<String>,
}

/// RPC invocation data delivered to a registered RPC method handler.
#[napi(object)]
pub struct LkRpcInvocationData {
    /// Identity of the caller.
    pub caller_identity: String,
    /// RPC request ID (used to correlate the response).
    pub request_id: String,
    /// Method name that was called.
    pub method: String,
    /// Payload string.
    pub payload: String,
    /// Response timeout in milliseconds.
    pub response_timeout_ms: u32,
}
