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

//! OHOS pure-Rust `SessionDescription` implementation.
//!
//! Backed by an in-memory string + [`SdpType`] pair instead of the libwebrtc C++
//! `SessionDescription`. Parsing is intentionally permissive: we only check that
//! the SDP is non-empty and starts with the version line (`v=`). Real
//! validation happens later when the SDP is fed into the rtc crate's
//! peer connection.

use crate::session_description::{self, SdpParseError, SdpType};

#[derive(Clone)]
pub struct SessionDescription {
    pub(crate) sdp_type: SdpType,
    pub(crate) sdp: String,
}

impl SessionDescription {
    /// Parse and wrap an SDP string with its type.
    ///
    /// Returns an [`SdpParseError`] if the SDP is empty or doesn't start with
    /// the mandatory `v=` line as required by [RFC 8866].
    ///
    /// [RFC 8866]: https://datatracker.ietf.org/doc/html/rfc8866
    pub fn parse(
        sdp: &str,
        sdp_type: SdpType,
    ) -> Result<session_description::SessionDescription, SdpParseError> {
        let trimmed = sdp.trim_start();
        if trimmed.is_empty() {
            return Err(SdpParseError {
                line: String::new(),
                description: "empty SDP".to_owned(),
            });
        }

        // Allow the rollback type to carry an empty body; otherwise require a
        // version marker so we fail fast on obviously malformed input.
        if !matches!(sdp_type, SdpType::Rollback) && !trimmed.starts_with("v=") {
            let first_line = trimmed.lines().next().unwrap_or("").to_owned();
            return Err(SdpParseError {
                line: first_line,
                description: "SDP must start with 'v=' line".to_owned(),
            });
        }

        Ok(session_description::SessionDescription {
            handle: SessionDescription { sdp_type, sdp: sdp.to_owned() },
        })
    }

    pub fn sdp_type(&self) -> SdpType {
        self.sdp_type
    }
}

impl ToString for SessionDescription {
    fn to_string(&self) -> String {
        self.sdp.clone()
    }
}
