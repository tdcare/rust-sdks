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

//! OHOS pure-Rust [`DataChannel`].
//!
//! Stores label/id/state plus the user callbacks. Sending data through the
//! actual SCTP association is handled by the higher-level OHOS peer
//! connection wrapper, which is expected to wire its `RTCDataChannel` up to
//! this struct's [`set_state`] / [`emit_message`] / [`emit_buffered_amount_change`]
//! methods.

use std::sync::{
    atomic::{AtomicI32, AtomicU64, Ordering},
    Arc,
};

use parking_lot::Mutex;

use crate::data_channel::{
    DataBuffer, DataChannelError, DataChannelState, OnBufferedAmountChange, OnMessage,
    OnStateChange,
};

#[derive(Clone)]
pub struct DataChannel {
    inner: Arc<Inner>,
}

struct Inner {
    id: AtomicI32,
    label: String,
    state: Mutex<DataChannelState>,
    buffered_amount: AtomicU64,
    on_state_change: Mutex<Option<OnStateChange>>,
    on_message: Mutex<Option<OnMessage>>,
    on_buffered_amount_change: Mutex<Option<OnBufferedAmountChange>>,
    /// Outgoing send queue. The OHOS peer connection drains this when the
    /// underlying SCTP transport is ready.
    send_queue: Mutex<Vec<(Vec<u8>, bool)>>,
}

impl DataChannel {
    /// Create a new data channel. `id` may be `-1` until the association
    /// negotiates a stream id, at which point [`set_id`] should be called.
    pub fn new(label: String, id: i32) -> Self {
        Self {
            inner: Arc::new(Inner {
                id: AtomicI32::new(id),
                label,
                state: Mutex::new(DataChannelState::Connecting),
                buffered_amount: AtomicU64::new(0),
                on_state_change: Mutex::new(None),
                on_message: Mutex::new(None),
                on_buffered_amount_change: Mutex::new(None),
                send_queue: Mutex::new(Vec::new()),
            }),
        }
    }

    pub fn send(&self, data: &[u8], binary: bool) -> Result<(), DataChannelError> {
        let state = *self.inner.state.lock();
        if !matches!(state, DataChannelState::Open) {
            return Err(DataChannelError::Send);
        }
        self.inner.send_queue.lock().push((data.to_vec(), binary));
        let new_amount =
            self.inner.buffered_amount.fetch_add(data.len() as u64, Ordering::SeqCst)
                + data.len() as u64;
        if let Some(cb) = self.inner.on_buffered_amount_change.lock().as_mut() {
            cb(new_amount);
        }
        Ok(())
    }

    pub fn id(&self) -> i32 {
        self.inner.id.load(Ordering::SeqCst)
    }

    pub fn label(&self) -> String {
        self.inner.label.clone()
    }

    pub fn state(&self) -> DataChannelState {
        *self.inner.state.lock()
    }

    pub fn close(&self) {
        self.set_state(DataChannelState::Closed);
    }

    pub fn buffered_amount(&self) -> u64 {
        self.inner.buffered_amount.load(Ordering::SeqCst)
    }

    pub fn on_state_change(&self, callback: Option<OnStateChange>) {
        *self.inner.on_state_change.lock() = callback;
    }

    pub fn on_message(&self, callback: Option<OnMessage>) {
        *self.inner.on_message.lock() = callback;
    }

    pub fn on_buffered_amount_change(&self, callback: Option<OnBufferedAmountChange>) {
        *self.inner.on_buffered_amount_change.lock() = callback;
    }

    // ---------------------------------------------------------------------
    // Internal hooks used by the OHOS peer connection wiring.
    // ---------------------------------------------------------------------

    pub(crate) fn set_id(&self, id: i32) {
        self.inner.id.store(id, Ordering::SeqCst);
    }

    pub(crate) fn set_state(&self, new_state: DataChannelState) {
        {
            let mut state = self.inner.state.lock();
            if *state == new_state {
                return;
            }
            *state = new_state;
        }
        if let Some(cb) = self.inner.on_state_change.lock().as_mut() {
            cb(new_state);
        }
    }

    pub(crate) fn emit_message(&self, data: &[u8], binary: bool) {
        if let Some(cb) = self.inner.on_message.lock().as_mut() {
            let buffer = DataBuffer { data, binary };
            cb(buffer);
        }
    }

    pub(crate) fn emit_buffered_amount_change(&self, amount: u64) {
        self.inner.buffered_amount.store(amount, Ordering::SeqCst);
        if let Some(cb) = self.inner.on_buffered_amount_change.lock().as_mut() {
            cb(amount);
        }
    }

    /// Drain pending sends queued via [`send`].
    ///
    /// Returns the queued (data, binary) pairs in FIFO order.
    pub(crate) fn drain_send_queue(&self) -> Vec<(Vec<u8>, bool)> {
        std::mem::take(&mut *self.inner.send_queue.lock())
    }
}
