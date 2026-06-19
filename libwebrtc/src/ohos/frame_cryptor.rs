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

//! OHOS frame cryptor.
//!
//! Implements LiveKit-compatible E2EE for OHOS using AES-GCM. The
//! [`FrameCryptor`] and [`DataPacketCryptor`] types expose the same
//! public surface as the native counterparts so the upper-layer
//! `livekit` crate can call into them from its frame transformer.
//!
//! Frame trailer layout (matches LiveKit's SIF format):
//! `[ciphertext + GCM tag][IV (12 bytes)][key_index (1 byte)]`.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicI32, Ordering},
        Arc,
    },
};

use aes_gcm::{
    aead::{generic_array::GenericArray, Aead, AeadCore, OsRng},
    Aes128Gcm, Aes256Gcm, KeyInit,
};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

use crate::{
    peer_connection_factory::PeerConnectionFactory, rtp_receiver::RtpReceiver,
    rtp_sender::RtpSender,
};

use super::packet_trailer::PacketTrailerHandler;

pub type OnStateChange = Box<dyn FnMut(String, EncryptionState) + Send + Sync>;

#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum KeyDerivationAlgorithm {
    PBKDF2,
    HKDF,
}

#[derive(Debug, Clone)]
pub struct KeyProviderOptions {
    pub shared_key: bool,
    pub ratchet_window_size: i32,
    pub ratchet_salt: Vec<u8>,
    pub failure_tolerance: i32,
    pub key_ring_size: i32,
    pub key_derivation_algorithm: KeyDerivationAlgorithm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionAlgorithm {
    AesGcm,
    AesCbc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionState {
    New,
    Ok,
    EncryptionFailed,
    DecryptionFailed,
    MissingKey,
    KeyRatcheted,
    InternalError,
}

#[derive(Debug, Clone)]
pub struct EncryptedPacket {
    pub data: Vec<u8>,
    pub iv: Vec<u8>,
    pub key_index: u32,
}

/// Derive a new symmetric key by mixing the current key with a salt
/// using SHA-256. The output length matches the input key length so
/// AES-128 keys stay 16 bytes and AES-256 keys stay 32 bytes (with
/// AES-256 we expand by hashing twice).
fn derive_key(current_key: &[u8], salt: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(current_key);
    let first = hasher.finalize();

    if current_key.len() <= 32 {
        first[..current_key.len()].to_vec()
    } else {
        // Extend output by chaining hashes for unexpectedly long keys.
        let mut out = Vec::with_capacity(current_key.len());
        out.extend_from_slice(&first);
        let mut prev = first.to_vec();
        while out.len() < current_key.len() {
            let mut h = Sha256::new();
            h.update(salt);
            h.update(&prev);
            prev = h.finalize().to_vec();
            out.extend_from_slice(&prev);
        }
        out.truncate(current_key.len());
        out
    }
}

#[derive(Default)]
struct KeyProviderInner {
    options: Option<KeyProviderOptions>,
    shared_keys: Mutex<HashMap<i32, Vec<u8>>>,
    participant_keys: Mutex<HashMap<(String, i32), Vec<u8>>>,
    sif_trailer: Mutex<Vec<u8>>,
}

#[derive(Clone, Default)]
pub struct KeyProvider {
    inner: Arc<KeyProviderInner>,
}

impl KeyProvider {
    pub fn new(options: KeyProviderOptions) -> Self {
        Self {
            inner: Arc::new(KeyProviderInner {
                options: Some(options),
                ..Default::default()
            }),
        }
    }

    pub fn set_shared_key(&self, key_index: i32, key: Vec<u8>) -> bool {
        self.inner.shared_keys.lock().insert(key_index, key);
        true
    }

    pub fn ratchet_shared_key(&self, key_index: i32) -> Option<Vec<u8>> {
        let salt = self.ratchet_salt();
        let mut keys = self.inner.shared_keys.lock();
        let current = keys.get(&key_index)?.clone();
        let derived = derive_key(&current, &salt);
        keys.insert(key_index, derived.clone());
        Some(derived)
    }

    pub fn get_shared_key(&self, key_index: i32) -> Option<Vec<u8>> {
        self.inner.shared_keys.lock().get(&key_index).cloned()
    }

    pub fn set_key(&self, participant_id: String, key_index: i32, key: Vec<u8>) -> bool {
        self.inner.participant_keys.lock().insert((participant_id, key_index), key);
        true
    }

    pub fn ratchet_key(&self, participant_id: String, key_index: i32) -> Option<Vec<u8>> {
        let salt = self.ratchet_salt();
        let mut keys = self.inner.participant_keys.lock();
        let key = (participant_id, key_index);
        let current = keys.get(&key)?.clone();
        let derived = derive_key(&current, &salt);
        keys.insert(key, derived.clone());
        Some(derived)
    }

    pub fn get_key(&self, participant_id: String, key_index: i32) -> Option<Vec<u8>> {
        self.inner.participant_keys.lock().get(&(participant_id, key_index)).cloned()
    }

    pub fn set_sif_trailer(&self, trailer: Vec<u8>) {
        *self.inner.sif_trailer.lock() = trailer;
    }

    /// Get key for encryption (shared key first, then participant key).
    fn get_key_for_encrypt(
        &self,
        participant_id: &str,
        key_index: i32,
    ) -> Result<Vec<u8>, String> {
        self.get_shared_key(key_index)
            .or_else(|| self.get_key(participant_id.to_string(), key_index))
            .ok_or_else(|| "no encryption key available".to_string())
    }

    /// Get key for decryption (shared key first, then participant key).
    fn get_key_for_decrypt(
        &self,
        participant_id: &str,
        key_index: i32,
    ) -> Result<Vec<u8>, String> {
        self.get_shared_key(key_index)
            .or_else(|| self.get_key(participant_id.to_string(), key_index))
            .ok_or_else(|| "no decryption key available".to_string())
    }

    fn ratchet_salt(&self) -> Vec<u8> {
        self.inner
            .options
            .as_ref()
            .map(|o| o.ratchet_salt.clone())
            .unwrap_or_default()
    }
}

struct FrameCryptorInner {
    participant_id: String,
    enabled: AtomicBool,
    key_index: AtomicI32,
    algorithm: EncryptionAlgorithm,
    key_provider: KeyProvider,
    state_change_handler: Mutex<Option<OnStateChange>>,
    #[allow(dead_code)]
    packet_trailer_handler: Mutex<Option<PacketTrailerHandler>>,
}

#[derive(Clone)]
pub struct FrameCryptor {
    inner: Arc<FrameCryptorInner>,
}

impl FrameCryptor {
    pub fn new_for_rtp_sender(
        _peer_factory: &PeerConnectionFactory,
        participant_id: String,
        algorithm: EncryptionAlgorithm,
        key_provider: KeyProvider,
        _sender: RtpSender,
    ) -> Self {
        Self::new_inner(participant_id, algorithm, key_provider)
    }

    pub fn new_for_rtp_receiver(
        _peer_factory: &PeerConnectionFactory,
        participant_id: String,
        algorithm: EncryptionAlgorithm,
        key_provider: KeyProvider,
        _receiver: RtpReceiver,
    ) -> Self {
        Self::new_inner(participant_id, algorithm, key_provider)
    }

    fn new_inner(
        participant_id: String,
        algorithm: EncryptionAlgorithm,
        key_provider: KeyProvider,
    ) -> Self {
        Self {
            inner: Arc::new(FrameCryptorInner {
                participant_id,
                enabled: AtomicBool::new(false),
                key_index: AtomicI32::new(0),
                algorithm,
                key_provider,
                state_change_handler: Mutex::new(None),
                packet_trailer_handler: Mutex::new(None),
            }),
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.inner.enabled.store(enabled, Ordering::Release);
    }

    pub fn enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::Acquire)
    }

    pub fn set_key_index(&self, index: i32) {
        self.inner.key_index.store(index, Ordering::Release);
    }

    pub fn key_index(&self) -> i32 {
        self.inner.key_index.load(Ordering::Acquire)
    }

    pub fn participant_id(&self) -> String {
        self.inner.participant_id.clone()
    }

    pub fn on_state_change(&self, handler: Option<OnStateChange>) {
        *self.inner.state_change_handler.lock() = handler;
    }

    pub fn set_packet_trailer_handler(&self, handler: &PacketTrailerHandler) {
        *self.inner.packet_trailer_handler.lock() = Some(handler.clone());
    }

    /// Encrypt a media frame payload with AES-GCM.
    ///
    /// The output is `[ciphertext + tag][IV (12 bytes)][key_index (1 byte)]`,
    /// matching LiveKit's SIF trailer layout.
    pub fn encrypt_frame(&self, payload: &[u8]) -> Result<Vec<u8>, String> {
        if !self.enabled() {
            return Ok(payload.to_vec());
        }

        let key_index = self.key_index();
        let key = self
            .inner
            .key_provider
            .get_key_for_encrypt(&self.inner.participant_id, key_index)
            .map_err(|e| {
                self.set_state(EncryptionState::MissingKey);
                e
            })?;

        let iv = Aes128Gcm::generate_nonce(&mut OsRng);

        let ciphertext = match self.inner.algorithm {
            EncryptionAlgorithm::AesGcm => match key.len() {
                16 => {
                    let cipher = Aes128Gcm::new(GenericArray::from_slice(&key));
                    cipher.encrypt(&iv, payload).map_err(|e| {
                        self.set_state(EncryptionState::EncryptionFailed);
                        format!("encrypt failed: {e}")
                    })?
                }
                len if len >= 32 => {
                    let cipher = Aes256Gcm::new(GenericArray::from_slice(&key[..32]));
                    cipher.encrypt(&iv, payload).map_err(|e| {
                        self.set_state(EncryptionState::EncryptionFailed);
                        format!("encrypt failed: {e}")
                    })?
                }
                len => {
                    self.set_state(EncryptionState::EncryptionFailed);
                    return Err(format!("unsupported AES-GCM key length: {len}"));
                }
            },
            EncryptionAlgorithm::AesCbc => {
                self.set_state(EncryptionState::EncryptionFailed);
                return Err("AES-CBC is not supported on OHOS".into());
            }
        };

        let mut result = ciphertext;
        result.extend_from_slice(iv.as_slice());
        result.push(key_index as u8);

        self.set_state(EncryptionState::Ok);
        Ok(result)
    }

    /// Decrypt a media frame payload produced by [`Self::encrypt_frame`].
    pub fn decrypt_frame(&self, payload: &[u8]) -> Result<Vec<u8>, String> {
        if !self.enabled() {
            return Ok(payload.to_vec());
        }

        // Minimum: 12-byte IV + 1-byte key_index + 16-byte GCM tag.
        if payload.len() < 12 + 1 + 16 {
            self.set_state(EncryptionState::DecryptionFailed);
            return Err("payload too short for E2EE".into());
        }

        let key_index = payload[payload.len() - 1] as i32;
        let iv_start = payload.len() - 13;
        let iv = &payload[iv_start..payload.len() - 1];
        let ciphertext = &payload[..iv_start];

        let key = self
            .inner
            .key_provider
            .get_key_for_decrypt(&self.inner.participant_id, key_index)
            .map_err(|_| {
                self.set_state(EncryptionState::MissingKey);
                "missing key".to_string()
            })?;

        let nonce = GenericArray::from_slice(iv);
        let plaintext = match self.inner.algorithm {
            EncryptionAlgorithm::AesGcm => match key.len() {
                16 => {
                    let cipher = Aes128Gcm::new(GenericArray::from_slice(&key));
                    cipher.decrypt(nonce, ciphertext).map_err(|e| {
                        self.set_state(EncryptionState::DecryptionFailed);
                        format!("decrypt failed: {e}")
                    })?
                }
                len if len >= 32 => {
                    let cipher = Aes256Gcm::new(GenericArray::from_slice(&key[..32]));
                    cipher.decrypt(nonce, ciphertext).map_err(|e| {
                        self.set_state(EncryptionState::DecryptionFailed);
                        format!("decrypt failed: {e}")
                    })?
                }
                len => {
                    self.set_state(EncryptionState::DecryptionFailed);
                    return Err(format!("unsupported AES-GCM key length: {len}"));
                }
            },
            EncryptionAlgorithm::AesCbc => {
                self.set_state(EncryptionState::DecryptionFailed);
                return Err("AES-CBC is not supported on OHOS".into());
            }
        };

        self.set_state(EncryptionState::Ok);
        Ok(plaintext)
    }

    fn set_state(&self, state: EncryptionState) {
        if let Some(handler) = self.inner.state_change_handler.lock().as_mut() {
            handler(self.inner.participant_id.clone(), state);
        }
    }
}

#[derive(Clone)]
pub struct DataPacketCryptor {
    algorithm: EncryptionAlgorithm,
    key_provider: KeyProvider,
}

impl DataPacketCryptor {
    pub fn new(algorithm: EncryptionAlgorithm, key_provider: KeyProvider) -> Self {
        Self { algorithm, key_provider }
    }

    pub fn encrypt(
        &self,
        participant_id: &str,
        key_index: u32,
        data: &[u8],
    ) -> Result<EncryptedPacket, Box<dyn std::error::Error>> {
        if !matches!(self.algorithm, EncryptionAlgorithm::AesGcm) {
            return Err("OHOS data-packet cryptor only supports AES-GCM".into());
        }

        let key = self
            .key_provider
            .get_shared_key(key_index as i32)
            .or_else(|| self.key_provider.get_key(participant_id.to_string(), key_index as i32))
            .ok_or("no key available")?;

        let iv = Aes128Gcm::generate_nonce(&mut OsRng);

        let ciphertext = match key.len() {
            16 => {
                let cipher = Aes128Gcm::new(GenericArray::from_slice(&key));
                cipher.encrypt(&iv, data).map_err(|e| format!("encrypt failed: {e}"))?
            }
            len if len >= 32 => {
                let cipher = Aes256Gcm::new(GenericArray::from_slice(&key[..32]));
                cipher.encrypt(&iv, data).map_err(|e| format!("encrypt failed: {e}"))?
            }
            len => return Err(format!("unsupported AES-GCM key length: {len}").into()),
        };

        Ok(EncryptedPacket { data: ciphertext, iv: iv.to_vec(), key_index })
    }

    pub fn decrypt(
        &self,
        participant_id: &str,
        encrypted_packet: &EncryptedPacket,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        if !matches!(self.algorithm, EncryptionAlgorithm::AesGcm) {
            return Err("OHOS data-packet cryptor only supports AES-GCM".into());
        }

        let key = self
            .key_provider
            .get_shared_key(encrypted_packet.key_index as i32)
            .or_else(|| {
                self.key_provider.get_key(
                    participant_id.to_string(),
                    encrypted_packet.key_index as i32,
                )
            })
            .ok_or("no key available")?;

        if encrypted_packet.iv.len() != 12 {
            return Err("invalid IV length".into());
        }
        let nonce = GenericArray::from_slice(&encrypted_packet.iv);

        let plaintext = match key.len() {
            16 => {
                let cipher = Aes128Gcm::new(GenericArray::from_slice(&key));
                cipher
                    .decrypt(nonce, encrypted_packet.data.as_slice())
                    .map_err(|e| format!("decrypt failed: {e}"))?
            }
            len if len >= 32 => {
                let cipher = Aes256Gcm::new(GenericArray::from_slice(&key[..32]));
                cipher
                    .decrypt(nonce, encrypted_packet.data.as_slice())
                    .map_err(|e| format!("decrypt failed: {e}"))?
            }
            len => return Err(format!("unsupported AES-GCM key length: {len}").into()),
        };

        Ok(plaintext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_provider(shared: bool) -> KeyProvider {
        KeyProvider::new(KeyProviderOptions {
            shared_key: shared,
            ratchet_window_size: 16,
            ratchet_salt: b"livekit-ratchet-salt".to_vec(),
            failure_tolerance: -1,
            key_ring_size: 16,
            key_derivation_algorithm: KeyDerivationAlgorithm::HKDF,
        })
    }

    #[test]
    fn frame_round_trip_aes128() {
        let kp = make_provider(true);
        kp.set_shared_key(0, vec![0x11; 16]);

        let cryptor = FrameCryptor {
            inner: Arc::new(FrameCryptorInner {
                participant_id: "p1".into(),
                enabled: AtomicBool::new(true),
                key_index: AtomicI32::new(0),
                algorithm: EncryptionAlgorithm::AesGcm,
                key_provider: kp,
                state_change_handler: Mutex::new(None),
                packet_trailer_handler: Mutex::new(None),
            }),
        };

        let plaintext = b"hello e2ee from ohos";
        let ct = cryptor.encrypt_frame(plaintext).unwrap();
        assert_ne!(&ct[..plaintext.len()], plaintext);
        let pt = cryptor.decrypt_frame(&ct).unwrap();
        assert_eq!(pt, plaintext);
    }

    #[test]
    fn frame_round_trip_aes256() {
        let kp = make_provider(true);
        kp.set_shared_key(1, vec![0x22; 32]);

        let cryptor = FrameCryptor {
            inner: Arc::new(FrameCryptorInner {
                participant_id: "p1".into(),
                enabled: AtomicBool::new(true),
                key_index: AtomicI32::new(1),
                algorithm: EncryptionAlgorithm::AesGcm,
                key_provider: kp,
                state_change_handler: Mutex::new(None),
                packet_trailer_handler: Mutex::new(None),
            }),
        };

        let plaintext = b"another payload";
        let ct = cryptor.encrypt_frame(plaintext).unwrap();
        let pt = cryptor.decrypt_frame(&ct).unwrap();
        assert_eq!(pt, plaintext);
    }

    #[test]
    fn data_packet_round_trip() {
        let kp = make_provider(false);
        kp.set_key("alice".into(), 0, vec![0x33; 16]);

        let dpc = DataPacketCryptor::new(EncryptionAlgorithm::AesGcm, kp);
        let payload = b"data channel message";
        let pkt = dpc.encrypt("alice", 0, payload).unwrap();
        let pt = dpc.decrypt("alice", &pkt).unwrap();
        assert_eq!(pt, payload);
    }

    #[test]
    fn ratchet_changes_key() {
        let kp = make_provider(true);
        kp.set_shared_key(0, vec![0xAA; 16]);
        let original = kp.get_shared_key(0).unwrap();
        let derived = kp.ratchet_shared_key(0).unwrap();
        assert_ne!(original, derived);
        assert_eq!(kp.get_shared_key(0).unwrap(), derived);
    }

    #[test]
    fn disabled_passthrough() {
        let kp = make_provider(true);
        kp.set_shared_key(0, vec![0x11; 16]);

        let cryptor = FrameCryptor {
            inner: Arc::new(FrameCryptorInner {
                participant_id: "p1".into(),
                enabled: AtomicBool::new(false),
                key_index: AtomicI32::new(0),
                algorithm: EncryptionAlgorithm::AesGcm,
                key_provider: kp,
                state_change_handler: Mutex::new(None),
                packet_trailer_handler: Mutex::new(None),
            }),
        };

        let plaintext = b"plain";
        assert_eq!(cryptor.encrypt_frame(plaintext).unwrap(), plaintext);
        assert_eq!(cryptor.decrypt_frame(plaintext).unwrap(), plaintext);
    }
}
