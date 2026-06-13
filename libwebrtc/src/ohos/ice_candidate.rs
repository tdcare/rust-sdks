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

//! OHOS pure-Rust `IceCandidate` implementation.
//!
//! Stores the trickle-ICE fields as plain Rust strings; we don't parse the
//! candidate attribute internally. The rtc crate consumes the raw SDP form
//! when adding remote candidates, so a string-only representation is enough.

use crate::{ice_candidate as ic, session_description::SdpParseError};

#[derive(Clone)]
pub struct IceCandidate {
    pub(crate) sdp_mid: String,
    pub(crate) sdp_mline_index: i32,
    pub(crate) candidate: String,
}

impl IceCandidate {
    /// Build an [`IceCandidate`] from its SDP fields.
    ///
    /// Returns [`SdpParseError`] if the candidate attribute is empty or doesn't
    /// look like an `a=candidate:`/`candidate:` line.
    pub fn parse(
        sdp_mid: &str,
        sdp_mline_index: i32,
        sdp: &str,
    ) -> Result<ic::IceCandidate, SdpParseError> {
        let trimmed = sdp.trim();
        if trimmed.is_empty() {
            return Err(SdpParseError {
                line: String::new(),
                description: "empty ICE candidate".to_owned(),
            });
        }

        // Accept both the bare attribute (`candidate:...`) and the full SDP form
        // (`a=candidate:...`).
        let body = trimmed.strip_prefix("a=").unwrap_or(trimmed);
        if !body.starts_with("candidate:") {
            return Err(SdpParseError {
                line: trimmed.to_owned(),
                description: "ICE candidate must start with 'candidate:'".to_owned(),
            });
        }

        Ok(ic::IceCandidate {
            handle: IceCandidate {
                sdp_mid: sdp_mid.to_owned(),
                sdp_mline_index,
                candidate: sdp.to_owned(),
            },
        })
    }

    pub fn sdp_mid(&self) -> String {
        self.sdp_mid.clone()
    }

    pub fn sdp_mline_index(&self) -> i32 {
        self.sdp_mline_index
    }

    pub fn candidate(&self) -> String {
        self.candidate.clone()
    }
}

impl ToString for IceCandidate {
    fn to_string(&self) -> String {
        self.candidate.clone()
    }
}
