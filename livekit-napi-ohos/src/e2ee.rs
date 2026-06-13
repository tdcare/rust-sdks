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

//! E2EE (End-to-End Encryption) NAPI bindings for ArkTS.

use libwebrtc::native::frame_cryptor::{KeyDerivationAlgorithm, KeyProvider, KeyProviderOptions};
use napi_derive_ohos::napi;
use napi_ohos::bindgen_prelude::*;

/// E2EE encryption options passed when connecting to a room.
#[napi(object)]
pub struct LkE2eeOptions {
    /// Encryption type: "gcm" (default) or "cbc".
    pub encryption_type: Option<String>,
    /// Key provider configuration.
    pub key_provider_options: Option<LkKeyProviderOptions>,
}

/// Key provider configuration.
#[napi(object)]
pub struct LkKeyProviderOptions {
    /// Whether to use a shared key for all participants.
    pub shared_key: Option<bool>,
    /// Number of keys to keep for ratcheting.
    pub ratchet_window_size: Option<i32>,
    /// Salt used for key derivation during ratcheting.
    pub ratchet_salt: Option<Buffer>,
    /// Tolerance for decryption failures before raising error.
    pub failure_tolerance: Option<i32>,
}

/// LiveKit E2EE Key Provider exposed to ArkTS.
#[napi]
pub struct LkKeyProvider {
    inner: KeyProvider,
}

#[napi]
impl LkKeyProvider {
    /// Create a new key provider with the given options.
    #[napi(constructor)]
    pub fn new(options: Option<LkKeyProviderOptions>) -> Self {
        let kp_options = if let Some(opts) = options {
            KeyProviderOptions {
                shared_key: opts.shared_key.unwrap_or(true),
                ratchet_window_size: opts.ratchet_window_size.unwrap_or(16),
                ratchet_salt: opts.ratchet_salt.map(|b| b.to_vec()).unwrap_or_default(),
                failure_tolerance: opts.failure_tolerance.unwrap_or(-1),
                key_ring_size: 16,
                key_derivation_algorithm: KeyDerivationAlgorithm::HKDF,
            }
        } else {
            KeyProviderOptions {
                shared_key: true,
                ratchet_window_size: 16,
                ratchet_salt: Vec::new(),
                failure_tolerance: -1,
                key_ring_size: 16,
                key_derivation_algorithm: KeyDerivationAlgorithm::HKDF,
            }
        };
        Self {
            inner: KeyProvider::new(kp_options),
        }
    }

    /// Set a shared encryption key.
    #[napi]
    pub fn set_shared_key(&self, key_index: i32, key: Buffer) -> bool {
        self.inner.set_shared_key(key_index, key.to_vec())
    }

    /// Ratchet (derive next) the shared key at the given index.
    #[napi]
    pub fn ratchet_shared_key(&self, key_index: i32) -> Option<Buffer> {
        self.inner
            .ratchet_shared_key(key_index)
            .map(|k| Buffer::from(k))
    }

    /// Get the current shared key at the given index.
    #[napi]
    pub fn get_shared_key(&self, key_index: i32) -> Option<Buffer> {
        self.inner
            .get_shared_key(key_index)
            .map(|k| Buffer::from(k))
    }

    /// Set a per-participant encryption key.
    #[napi]
    pub fn set_key(&self, participant_id: String, key_index: i32, key: Buffer) -> bool {
        self.inner.set_key(participant_id, key_index, key.to_vec())
    }

    /// Ratchet a per-participant key.
    #[napi]
    pub fn ratchet_key(&self, participant_id: String, key_index: i32) -> Option<Buffer> {
        self.inner
            .ratchet_key(participant_id, key_index)
            .map(|k| Buffer::from(k))
    }

    /// Get a per-participant key.
    #[napi]
    pub fn get_key(&self, participant_id: String, key_index: i32) -> Option<Buffer> {
        self.inner
            .get_key(participant_id, key_index)
            .map(|k| Buffer::from(k))
    }

    /// Set the SIF (Sender Identity Frame) trailer bytes.
    #[napi]
    pub fn set_sif_trailer(&self, trailer: Buffer) {
        self.inner.set_sif_trailer(trailer.to_vec());
    }
}

impl LkKeyProvider {
    pub(crate) fn inner(&self) -> &KeyProvider {
        &self.inner
    }
}
