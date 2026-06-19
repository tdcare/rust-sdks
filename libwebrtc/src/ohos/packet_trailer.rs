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

//! OHOS pure-Rust packet trailer support.
//!
//! Mirrors [`native::packet_trailer::PacketTrailerHandler`](crate::native::packet_trailer::PacketTrailerHandler)
//! but stores the metadata map in plain Rust state rather than going through
//! the libwebrtc `RTPSenderInterface` / `RTPReceiverInterface` C++ bindings.
//! Real wire-level trailer embedding/extraction will be wired up once the
//! OHOS encoder pipeline gains a frame transformer.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use parking_lot::Mutex;

#[derive(Clone)]
pub struct PacketTrailerHandler {
    inner: Arc<Inner>,
}

struct Inner {
    enabled: AtomicBool,
    /// Sender side: capture-timestamp -> (user_timestamp, frame_id)
    by_capture: Mutex<HashMap<i64, (u64, u32)>>,
    /// Receiver side: rtp-timestamp -> (user_timestamp, frame_id)
    by_rtp: Mutex<HashMap<u32, (u64, u32)>>,
}

impl Default for PacketTrailerHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl PacketTrailerHandler {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                enabled: AtomicBool::new(false),
                by_capture: Mutex::new(HashMap::new()),
                by_rtp: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.inner.enabled.store(enabled, Ordering::SeqCst);
    }

    pub fn enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::SeqCst)
    }

    /// Receiver-side lookup. Returns and consumes the entry for the given
    /// RTP timestamp.
    pub fn lookup_frame_metadata(&self, rtp_timestamp: u32) -> Option<(u64, u32)> {
        self.inner.by_rtp.lock().remove(&rtp_timestamp)
    }

    /// Sender-side store. Records `(user_timestamp, frame_id)` keyed by the
    /// TimestampAligner-adjusted capture timestamp.
    pub fn store_frame_metadata(
        &self,
        capture_timestamp_us: i64,
        user_timestamp: u64,
        frame_id: u32,
    ) {
        self.inner
            .by_capture
            .lock()
            .insert(capture_timestamp_us, (user_timestamp, frame_id));
    }

    // Hooks consumed by the OHOS encoder/decoder integration -----------------

    /// Sender pipeline: drain the `(user_timestamp, frame_id)` for the frame
    /// that was captured at `capture_timestamp_us`.
    pub fn take_sender_metadata(&self, capture_timestamp_us: i64) -> Option<(u64, u32)> {
        self.inner.by_capture.lock().remove(&capture_timestamp_us)
    }

    /// Receiver pipeline: record metadata extracted from a wire trailer.
    pub fn record_receiver_metadata(
        &self,
        rtp_timestamp: u32,
        user_timestamp: u64,
        frame_id: u32,
    ) {
        self.inner.by_rtp.lock().insert(rtp_timestamp, (user_timestamp, frame_id));
    }
}

/// Stub: OHOS does not yet wire packet trailer transformers to the encoder.
///
/// Returns a fresh, disabled [`PacketTrailerHandler`] so call-sites compile
/// and behave as no-ops.
pub fn create_sender_handler<P, S>(_peer_factory: &P, _sender: &S) -> PacketTrailerHandler {
    PacketTrailerHandler::new()
}

/// Stub: see [`create_sender_handler`].
pub fn create_receiver_handler<P, R>(
    _peer_factory: &P,
    _receiver: &R,
) -> PacketTrailerHandler {
    PacketTrailerHandler::new()
}

// ---------------------------------------------------------------------------
// Stub publish/subscribe timing types (mirrors native::packet_trailer)
// ---------------------------------------------------------------------------

/// Stage reached by a native local video frame in the publish pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishTimingStage {
    EncoderUpload,
    EncoderOutput,
    WebrtcPacketize,
}

/// Stage reached by a native remote video frame in the subscribe pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscribeTimingStage {
    WebrtcReceive,
    DecoderUpload,
    DecoderOutput,
}

/// Timestamped native local video publish pipeline event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishTimingEvent {
    pub stage: PublishTimingStage,
    pub timestamp_us: u64,
    pub capture_timestamp_us: u64,
    pub frame_id: Option<u32>,
}

/// Timestamped native remote video subscribe pipeline event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubscribeTimingEvent {
    pub stage: SubscribeTimingStage,
    pub timestamp_us: u64,
    pub capture_timestamp_us: u64,
    pub frame_id: Option<u32>,
}

/// Callback invoked for native local video publish timing events.
pub type PublishTimingObserver = std::sync::Arc<dyn Fn(PublishTimingEvent) + Send + Sync + 'static>;
/// Callback invoked for native remote video subscribe timing events.
pub type SubscribeTimingObserver = std::sync::Arc<dyn Fn(SubscribeTimingEvent) + Send + Sync + 'static>;

impl PacketTrailerHandler {
    /// Stub: OHOS does not yet implement timing observer callbacks.
    pub fn set_publish_timing_observer(&self, _observer: Option<PublishTimingObserver>) {
        // no-op on OHOS
    }

    /// Stub: OHOS does not yet implement timing observer callbacks.
    pub fn set_subscribe_timing_observer(&self, _observer: Option<SubscribeTimingObserver>) {
        // no-op on OHOS
    }
}
