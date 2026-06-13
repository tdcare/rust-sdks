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

//! Per-track RTP send pipeline.
//!
//! Owns the per-track sequence number / SSRC / payload type and forwards
//! packetised fragments to the [`RtcIoDriver`] via [`ControlCommand::WriteRtp`].
//! One [`RtpSendPipeline`] is created per outbound track and bound to the
//! corresponding [`NativeAudioSource`]/[`NativeVideoSource`].

use tokio::sync::mpsc;

use super::rtc_io_driver::{ControlCommand, RtpPacketData};
use super::rtp_packetizer;
use crate::{RtcError, RtcErrorType};

/// Per-track RTP send pipeline.
///
/// Cheap to clone (the underlying channel sender is reference-counted).
/// Sequence numbers advance per *fragment*; cloning the pipeline forks the
/// counter, so callers should keep a single owner per track and rely on
/// interior mutability when concurrent access is needed.
#[derive(Clone)]
pub(crate) struct RtpSendPipeline {
    track_id: String,
    ssrc: u32,
    /// Payload type for H.264 (primary codec).
    payload_type: u8,
    /// Payload type for VP8 (fallback codec).
    payload_type_vp8: u8,
    /// VP8 PictureID counter (RFC 7741 §4.2), incremented per encoded frame.
    /// Wraps from 0x7FFF → 0 per spec (15-bit field, 0x8000 reserved).
    vp8_picture_id: u16,
    clock_rate: u32,
    sequence_number: u16,
    cmd_tx: mpsc::UnboundedSender<ControlCommand>,
    /// Number of consecutive send failures (driver closed).
    send_fail_count: u32,
}

impl RtpSendPipeline {
    pub(crate) fn new(
        track_id: String,
        ssrc: u32,
        payload_type: u8,
        payload_type_vp8: u8,
        clock_rate: u32,
        cmd_tx: mpsc::UnboundedSender<ControlCommand>,
    ) -> Self {
        Self {
            track_id,
            ssrc,
            payload_type,
            payload_type_vp8,
            vp8_picture_id: 0,
            clock_rate,
            sequence_number: random_initial_seq(),
            cmd_tx,
            send_fail_count: 0,
        }
    }

    /// Track id this pipeline belongs to.
    #[allow(dead_code)]
    pub(crate) fn track_id(&self) -> &str {
        &self.track_id
    }

    /// SSRC used by this pipeline.
    pub(crate) fn ssrc(&self) -> u32 {
        self.ssrc
    }

    /// Send an encoded video frame.
    ///
    /// Performs RFC 6184 (H.264) or RFC 7741 (VP8) packetisation and
    /// forwards the resulting RTP packets to the driver. Returns the number
    /// of RTP packets emitted.
    pub(crate) fn send_encoded_video(
        &mut self,
        data: &[u8],
        timestamp_ms: u64,
        codec: &str,
        is_key_frame: bool,
    ) -> Result<u32, RtcError> {
        // Select correct PT based on the actual codec being used.
        // This is critical when H.264 encoder fails and VP8 fallback kicks in:
        // VP8 data MUST use VP8's PT (96), not H.264's PT (125), otherwise
        // the receiver will try to decode VP8 as H.264 → black screen.
        let (pt, fragments) = match codec.to_ascii_uppercase().as_str() {
            "H264" => {
                let frags = rtp_packetizer::pack_h264_frame(data);
                // Log packetization details for debugging
                if !frags.is_empty() {
                    let first_payload = &frags[0].payload;
                    let head_hex: Vec<String> = first_payload.iter().take(8).map(|b| format!("{:02x}", b)).collect();
                    log::info!(
                        "[RtpSendPipeline] H264 packetized: {} bytes input → {} fragments, first frag: {} bytes, head=[{}], marker={}, pt={}",
                        data.len(), frags.len(), first_payload.len(), head_hex.join(" "), frags[0].marker, self.payload_type
                    );
                }
                (self.payload_type, frags)
            }
            "VP8" => {
                let pid = self.vp8_picture_id;
                let frags = rtp_packetizer::pack_vp8_frame(data, is_key_frame, pid);
                // Increment PictureID for next frame. RFC 7741 §4.2: 15-bit
                // value, wraps from 0x7FFF → 0 (0x8000 is reserved).
                self.vp8_picture_id = (self.vp8_picture_id + 1) & 0x7FFF;
                if !frags.is_empty() {
                    let first_payload = &frags[0].payload;
                    let head_hex: Vec<String> = first_payload.iter().take(8).map(|b| format!("{:02x}", b)).collect();
                    log::info!(
                        "[RtpSendPipeline] VP8 packetized: {} bytes input → {} fragments, first frag: {} bytes, head=[{}], marker={}, pt={}, pid={}, key={}",
                        data.len(), frags.len(), first_payload.len(), head_hex.join(" "), frags[0].marker, self.payload_type_vp8, pid, is_key_frame
                    );
                }
                (self.payload_type_vp8, frags)
            }
            other => {
                return Err(RtcError {
                    error_type: RtcErrorType::Internal,
                    message: format!("unsupported video codec: {other}"),
                });
            }
        };

        if fragments.is_empty() {
            return Ok(0);
        }

        let rtp_ts = ms_to_rtp_timestamp(timestamp_ms, self.clock_rate);
        let packets = rtp_packetizer::fragments_to_packets(
            &fragments,
            pt,
            self.ssrc,
            rtp_ts,
            &mut self.sequence_number,
        );

        let count = packets.len() as u32;
        for pkt in packets {
            self.send_packet(pkt)?;
        }
        Ok(count)
    }

    /// Send an encoded audio frame. Audio frames at typical bitrates fit in
    /// a single RTP packet, so no fragmentation is performed.
    pub(crate) fn send_encoded_audio(
        &mut self,
        data: &[u8],
        timestamp_ms: u64,
    ) -> Result<(), RtcError> {
        let rtp_ts = ms_to_rtp_timestamp(timestamp_ms, self.clock_rate);
        self.sequence_number = self.sequence_number.wrapping_add(1);

        let packet = RtpPacketData {
            payload_type: self.payload_type,
            sequence_number: self.sequence_number,
            timestamp: rtp_ts,
            ssrc: self.ssrc,
            marker: true,
            payload: data.to_vec(),
        };

        self.send_packet(packet)
    }

    fn send_packet(&mut self, packet: RtpPacketData) -> Result<(), RtcError> {
        self.cmd_tx
            .send(ControlCommand::WriteRtp { track_id: self.track_id.clone(), packet })
            .map_err(|_| {
                self.send_fail_count += 1;
                // Log at power-of-2 intervals to avoid log spam
                if self.send_fail_count == 1
                    || self.send_fail_count == 10
                    || self.send_fail_count == 100
                    || self.send_fail_count % 1000 == 0
                {
                    log::error!(
                        "[RtpSendPipeline] rtc-io driver closed: track={}, ssrc={}, fail_count={}",
                        self.track_id, self.ssrc, self.send_fail_count
                    );
                }
                RtcError {
                    error_type: RtcErrorType::Internal,
                    message: format!(
                        "rtc-io driver closed (track={}, failures={})",
                        self.track_id, self.send_fail_count
                    ),
                }
            })
    }
}

fn ms_to_rtp_timestamp(timestamp_ms: u64, clock_rate: u32) -> u32 {
    ((timestamp_ms.saturating_mul(clock_rate as u64)) / 1000) as u32
}

/// Random initial sequence number (RFC 3550 §5.1 recommends randomisation
/// to harden against known-plaintext attacks on SRTP).
fn random_initial_seq() -> u16 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos & 0xFFFF) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ms_to_rtp_timestamp_audio_48k() {
        // 20ms @ 48kHz -> 960 samples.
        assert_eq!(ms_to_rtp_timestamp(20, 48_000), 960);
    }

    #[test]
    fn ms_to_rtp_timestamp_video_90k() {
        // ~33ms @ 90kHz -> 2970.
        assert_eq!(ms_to_rtp_timestamp(33, 90_000), 2970);
    }

    #[test]
    fn send_encoded_audio_increments_seq_and_writes_command() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut p = RtpSendPipeline::new("a".into(), 0xCAFE, 111, 0, 48_000, tx);
        p.sequence_number = 10;
        p.send_encoded_audio(&[1, 2, 3], 20).unwrap();
        assert_eq!(p.sequence_number, 11);
        match rx.try_recv().unwrap() {
            ControlCommand::WriteRtp { track_id, packet } => {
                assert_eq!(track_id, "a");
                assert_eq!(packet.sequence_number, 11);
                assert_eq!(packet.timestamp, 960);
                assert!(packet.marker);
                assert_eq!(packet.payload, vec![1, 2, 3]);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn send_encoded_video_unsupported_codec() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut p = RtpSendPipeline::new("v".into(), 0, 125, 96, 90_000, tx);
        let err = p.send_encoded_video(&[0u8; 4], 0, "AV1", false).unwrap_err();
        assert!(matches!(err.error_type, RtcErrorType::Internal));
    }
}
