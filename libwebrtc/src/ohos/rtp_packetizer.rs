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

//! RTP packetization for H.264 (RFC 6184) and VP8 (RFC 7741).
//!
//! Splits encoded media frames into MTU-sized RTP fragments. The driver
//! later wraps each fragment in an `rtp::Packet` and feeds it through the
//! SRTP/ICE pipeline.

use bytes::Bytes;
use super::rtc_io_driver::RtpPacketData;

/// Maximum RTP payload size to avoid IP fragmentation
/// (typical MTU 1500 minus IP/UDP/RTP/SRTP overhead).
const MAX_RTP_PAYLOAD_SIZE: usize = 1200;

/// H.264 NAL unit type for FU-A fragmentation (RFC 6184 §5.8).
const H264_NAL_TYPE_FU_A: u8 = 28;

/// A single RTP payload ready for transmission.
#[derive(Debug, Clone)]
pub(crate) struct RtpFragment {
    pub payload: Bytes,
    pub marker: bool,
}

// ---------------------------------------------------------------------------
// H.264 (RFC 6184)
// ---------------------------------------------------------------------------

/// Pack an H.264 Annex-B bitstream into RTP fragments using Single NALU or
/// FU-A mode.
///
/// Marker bit semantics:
/// - SPS/PPS/SEI NALUs (types 6/7/8) never set the marker bit.
/// - The marker bit is set only on the last fragment of the last *picture*
///   NALU (types 1-5) of the frame.
pub(crate) fn pack_h264_frame(annexb_data: &[u8]) -> Vec<RtpFragment> {
    let nalus = split_annexb_nalus(annexb_data);
    if nalus.is_empty() {
        return Vec::new();
    }

    let last_picture_idx = find_last_picture_nalu(&nalus);
    let mut fragments = Vec::with_capacity(nalus.len());

    for (idx, nalu) in nalus.iter().enumerate() {
        if nalu.is_empty() {
            continue;
        }
        let is_last_picture = Some(idx) == last_picture_idx;

        if nalu.len() <= MAX_RTP_PAYLOAD_SIZE {
            // Single NALU packet: payload IS the NALU bytes (header + RBSP).
            fragments.push(RtpFragment { payload: Bytes::copy_from_slice(nalu), marker: is_last_picture });
        } else {
            pack_h264_fua(nalu, is_last_picture, &mut fragments);
        }
    }

    fragments
}

/// FU-A fragmentation (RFC 6184 §5.8).
///
/// Each fragment is prefixed with two bytes:
/// - FU indicator: F(1)|NRI(2)|Type(5)=28
/// - FU header   : S(1)|E(1)|R(1)|Type(5) where Type is the original NALU type
fn pack_h264_fua(nalu: &[u8], is_last_picture_of_frame: bool, fragments: &mut Vec<RtpFragment>) {
    let nalu_header = nalu[0];
    let nri = nalu_header & 0xE0;
    let nalu_type = nalu_header & 0x1F;
    let fu_indicator = nri | H264_NAL_TYPE_FU_A;

    // Subtract two header bytes from the per-fragment payload budget.
    let max_chunk = MAX_RTP_PAYLOAD_SIZE - 2;
    let mut offset = 1usize; // Skip original NALU header byte.
    let mut is_first = true;

    while offset < nalu.len() {
        let remaining = nalu.len() - offset;
        let chunk = remaining.min(max_chunk);
        let is_last = offset + chunk >= nalu.len();

        let mut fu_header = nalu_type;
        if is_first {
            fu_header |= 0x80; // Start bit
        }
        if is_last {
            fu_header |= 0x40; // End bit
        }

        let mut payload = Vec::with_capacity(2 + chunk);
        payload.push(fu_indicator);
        payload.push(fu_header);
        payload.extend_from_slice(&nalu[offset..offset + chunk]);

        let marker = is_last && is_last_picture_of_frame;
        fragments.push(RtpFragment { payload: Bytes::from(payload), marker });

        offset += chunk;
        is_first = false;
    }
}

// ---------------------------------------------------------------------------
// VP8 (RFC 7741)
// ---------------------------------------------------------------------------

/// Pack a VP8 frame into RTP fragments with RFC 7741 Payload Descriptor
/// including mandatory PictureID.
///
/// Descriptor structure (RFC 7741 §4.2):
///   Byte 0: X(1)|R(0)|N(1)|S(1)|PartID(4)
///           X=1 enables extended control bits.
///           R=0 (not a non-reference, i.e. may be referenced).
///           N=1 on non-reference (droppable) frames; 0 otherwise.
///           S=1 on first fragment; 0 on continuation.
///   Byte 1: I(1)|L(0)|T(0)|K(0)|RSV(4)  — extended control when X=1
///           I=1: PictureID present.
///   Byte 2: M(1)|PictureID(7)             — 7-bit PictureID (< 128)
///   Bytes 2-3: M(1)|PID(hi 7)|PID(lo 8)  — 15-bit PictureID (≥ 128)
///
/// Without PictureID, some WebRTC receivers (e.g. Chrome) cannot correctly
/// track inter-frame references, causing green tint and ghosting.
pub(crate) fn pack_vp8_frame(vp8_data: &[u8], is_key_frame: bool, picture_id: u16) -> Vec<RtpFragment> {
    if vp8_data.is_empty() {
        return Vec::new();
    }

    // Build descriptor prefix (same PictureID for all fragments of this frame).
    // N=0 for all frames: they MAY be referenced by subsequent frames.
    // N=1 (=0x20) would mark the frame as non-reference / droppable, causing
    // the decoder to discard it → broken inter-frame prediction → green/ghosting.
    let n_bit = 0x00; // N=0: frame is referencable

    let (first_byte, desc_size, desc_prefix_small, desc_prefix_large): (u8, usize, [u8; 3], [u8; 4]) =
        if picture_id < 128 {
            // 7-bit PictureID: 3 descriptor bytes total
            let fb = 0x90 | n_bit; // X=1, S=1, PartID=0
            let ec = 0x80;         // I=1
            let pid = picture_id as u8;
            (fb, 3, [fb, ec, pid], [0; 4])
        } else {
            // 15-bit PictureID: 4 descriptor bytes
            let fb = 0x90 | n_bit;
            let ec = 0x80;                     // I=1
            let pid_hi = 0x80 | ((picture_id >> 8) as u8 & 0x7F); // M=1
            let pid_lo = picture_id as u8;
            (fb, 4, [0; 3], [fb, ec, pid_hi, pid_lo])
        };

    // Continuation fragment first byte: same X/N but S=0
    let cont_first_byte = first_byte & !0x10; // clear S bit

    let max_chunk = MAX_RTP_PAYLOAD_SIZE - desc_size;
    let est = 1 + vp8_data.len() / MAX_RTP_PAYLOAD_SIZE;
    let mut fragments = Vec::with_capacity(est);
    let mut offset = 0usize;
    let mut is_first = true;

    while offset < vp8_data.len() {
        let remaining = vp8_data.len() - offset;
        let chunk = remaining.min(max_chunk);
        let is_last = offset + chunk >= vp8_data.len();

        let mut payload = Vec::with_capacity(desc_size + chunk);
        if picture_id < 128 {
            let prefix = if is_first { desc_prefix_small } else {
                [cont_first_byte, desc_prefix_small[1], desc_prefix_small[2]]
            };
            payload.extend_from_slice(&prefix);
        } else {
            if is_first {
                payload.extend_from_slice(&desc_prefix_large);
            } else {
                payload.push(cont_first_byte);
                payload.extend_from_slice(&desc_prefix_large[1..]);
            }
        }
        payload.extend_from_slice(&vp8_data[offset..offset + chunk]);

        fragments.push(RtpFragment { payload: Bytes::from(payload), marker: is_last });

        offset += chunk;
        is_first = false;
    }

    fragments
}

// ---------------------------------------------------------------------------
// Opus
// ---------------------------------------------------------------------------

/// Pack an Opus audio frame. Opus frames at typical bitrates always fit
/// inside a single RTP packet, so no fragmentation is required.
pub(crate) fn pack_opus_frame(opus_data: &[u8]) -> RtpFragment {
    RtpFragment { payload: Bytes::copy_from_slice(opus_data), marker: true }
}

// ---------------------------------------------------------------------------
// Fragment → RtpPacketData
// ---------------------------------------------------------------------------

/// Convert fragments into [`RtpPacketData`] with sequential numbering.
///
/// `seq` is incremented in place so callers can preserve sequence-number
/// continuity across successive frames. The wrapping behaviour matches RFC
/// 3550.
pub(crate) fn fragments_to_packets(
    fragments: &[RtpFragment],
    payload_type: u8,
    ssrc: u32,
    timestamp: u32,
    seq: &mut u16,
) -> Vec<RtpPacketData> {
    let mut packets = Vec::with_capacity(fragments.len());
    for frag in fragments {
        *seq = seq.wrapping_add(1);
        packets.push(RtpPacketData {
            payload_type,
            sequence_number: *seq,
            timestamp,
            ssrc,
            marker: frag.marker,
            payload: frag.payload.clone(),
        });
    }
    packets
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Split an Annex-B bitstream into individual NALUs, stripping start codes
/// (`0x000001` or `0x00000001`).
fn split_annexb_nalus(data: &[u8]) -> Vec<&[u8]> {
    let mut nalus = Vec::with_capacity(16);
    let mut i = 0usize;
    let mut nalu_start: Option<usize> = None;

    while i < data.len() {
        if i + 2 < data.len() && data[i] == 0x00 && data[i + 1] == 0x00 {
            let start_code_len = if data[i + 2] == 0x01 {
                3
            } else if i + 3 < data.len() && data[i + 2] == 0x00 && data[i + 3] == 0x01 {
                4
            } else {
                0
            };

            if start_code_len > 0 {
                if let Some(start) = nalu_start {
                    if start < i {
                        nalus.push(&data[start..i]);
                    }
                }
                nalu_start = Some(i + start_code_len);
                i += start_code_len;
                continue;
            }
        }
        i += 1;
    }

    if let Some(start) = nalu_start {
        if start < data.len() {
            nalus.push(&data[start..]);
        }
    }

    // No start code found: treat entire buffer as one NALU.
    if nalus.is_empty() && !data.is_empty() {
        nalus.push(data);
    }

    nalus
}

/// Find the index of the last "picture" NALU (types 1-5) in the given list.
///
/// Used to attach the marker bit only to the closing fragment of the frame's
/// final picture NALU; SPS/PPS/SEI NALUs (types 6-8) never carry the marker.
fn find_last_picture_nalu(nalus: &[&[u8]]) -> Option<usize> {
    nalus.iter().enumerate().rev().find_map(|(idx, nalu)| {
        if nalu.is_empty() {
            return None;
        }
        let nt = nalu[0] & 0x1F;
        if (1..=5).contains(&nt) {
            Some(idx)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_annexb_three_byte_start_code() {
        let data = [0x00, 0x00, 0x01, 0x65, 0xAA, 0x00, 0x00, 0x01, 0x68, 0xBB];
        let nalus = split_annexb_nalus(&data);
        assert_eq!(nalus.len(), 2);
        assert_eq!(nalus[0], &[0x65, 0xAA]);
        assert_eq!(nalus[1], &[0x68, 0xBB]);
    }

    #[test]
    fn h264_single_nalu_marker_only_on_picture() {
        // SPS (type 7) followed by IDR slice (type 5). Only the IDR carries marker.
        let mut data = vec![0x00, 0x00, 0x00, 0x01, 0x67, 0x42];
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x65, 0x88]);
        let frags = pack_h264_frame(&data);
        assert_eq!(frags.len(), 2);
        assert!(!frags[0].marker);
        assert!(frags[1].marker);
    }

    #[test]
    fn h264_fua_split() {
        // Single picture NALU larger than MAX_RTP_PAYLOAD_SIZE.
        let mut data = vec![0x00, 0x00, 0x00, 0x01, 0x65];
        data.extend(std::iter::repeat(0xAB).take(MAX_RTP_PAYLOAD_SIZE * 2));
        let frags = pack_h264_frame(&data);
        assert!(frags.len() >= 2);
        // First fragment: S bit set.
        assert_eq!(frags[0].payload[1] & 0x80, 0x80);
        // Last fragment: E bit set + marker bit set.
        let last = frags.last().unwrap();
        assert_eq!(last.payload[1] & 0x40, 0x40);
        assert!(last.marker);
    }

    #[test]
    fn vp8_descriptor_first_then_continuation() {
        let data = vec![0xCDu8; MAX_RTP_PAYLOAD_SIZE * 2];
        let frags = pack_vp8_frame(&data, true, 5);
        assert!(frags.len() >= 2);
        // First fragment: X=1,N=0,S=1 → 0x90, then I=1 → 0x80, then PictureID
        assert_eq!(frags[0].payload[0], 0x90); // X=1, N=0, S=1, PartID=0
        assert_eq!(frags[0].payload[1], 0x80); // I=1
        assert_eq!(frags[0].payload[2], 5);    // PictureID
        // Continuation: S=0
        assert_eq!(frags[1].payload[0], 0x80); // X=1, N=0, S=0
        assert_eq!(frags[1].payload[1], 0x80); // I=1
        assert!(frags.last().unwrap().marker);
    }

    #[test]
    fn fragments_to_packets_increments_sequence() {
        let frags = vec![
            RtpFragment { payload: vec![1], marker: false },
            RtpFragment { payload: vec![2], marker: true },
        ];
        let mut seq = 100u16;
        let pkts = fragments_to_packets(&frags, 96, 0xDEAD_BEEF, 1234, &mut seq);
        assert_eq!(pkts[0].sequence_number, 101);
        assert_eq!(pkts[1].sequence_number, 102);
        assert_eq!(seq, 102);
        assert_eq!(pkts[0].marker, false);
        assert_eq!(pkts[1].marker, true);
    }
}
