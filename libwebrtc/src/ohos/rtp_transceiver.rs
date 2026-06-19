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

use super::{rtp_receiver::RtpReceiver as ImpRtpReceiver, rtp_sender::RtpSender as ImpRtpSender};
use crate::{
    rtp_parameters::RtpCodecCapability,
    rtp_receiver::RtpReceiver,
    rtp_sender::RtpSender,
    rtp_transceiver::{RtpTransceiverDirection, RtpTransceiverInit},
    RtcError,
};

/// OHOS transceiver handle.
///
/// Direction and `current_direction` are kept behind interior mutability so
/// the transceiver can be cloned (for e.g. observer callbacks) while the SDP
/// negotiation flow updates them in place.
#[derive(Clone)]
pub struct RtpTransceiver {
    pub(crate) mid: Arc<Mutex<Option<String>>>,
    pub(crate) direction: Arc<Mutex<RtpTransceiverDirection>>,
    pub(crate) current_direction: Arc<Mutex<Option<RtpTransceiverDirection>>>,
    pub(crate) sender: ImpRtpSender,
    pub(crate) receiver: ImpRtpReceiver,
}

impl RtpTransceiver {
    pub(crate) fn new(
        init: &RtpTransceiverInit,
        sender: ImpRtpSender,
        receiver: ImpRtpReceiver,
    ) -> Self {
        Self {
            mid: Arc::new(Mutex::new(None)),
            direction: Arc::new(Mutex::new(init.direction)),
            current_direction: Arc::new(Mutex::new(None)),
            sender,
            receiver,
        }
    }

    pub fn mid(&self) -> Option<String> {
        self.mid.lock().clone()
    }

    pub fn current_direction(&self) -> Option<RtpTransceiverDirection> {
        *self.current_direction.lock()
    }

    pub fn direction(&self) -> RtpTransceiverDirection {
        *self.direction.lock()
    }

    pub fn sender(&self) -> RtpSender {
        RtpSender { handle: self.sender.clone() }
    }

    pub fn receiver(&self) -> RtpReceiver {
        RtpReceiver { handle: self.receiver.clone() }
    }

    pub fn set_codec_preferences(&self, _codecs: Vec<RtpCodecCapability>) -> Result<(), RtcError> {
        // TODO(ohos): Full implementation with rtc crate codec negotiation.
        Ok(())
    }

    pub fn stop(&self) -> Result<(), RtcError> {
        *self.direction.lock() = RtpTransceiverDirection::Stopped;
        *self.current_direction.lock() = Some(RtpTransceiverDirection::Stopped);
        Ok(())
    }
}
