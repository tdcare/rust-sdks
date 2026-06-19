//! Statistics types for the LiveKit NAPI bindings.
//!
//! The `get_stats()` method is exposed on [`LkRoom`](crate::room::LkRoom) (in `room.rs`).
//! This module defines the output types and conversion helpers.

use napi_derive_ohos::napi;

/// A single RTC statistics entry.
///
/// Stats are returned as JSON to avoid creating dozens of nested NAPI types.
/// ArkTS consumers can parse `json_data` with `JSON.parse()`.
#[napi(object)]
#[derive(Clone)]
pub struct LkRtcStats {
    /// Stats category: "codec", "inbound-rtp", "outbound-rtp", "remote-inbound-rtp",
    /// "transport", "candidate-pair", "local-candidate", "remote-candidate",
    /// "peer-connection", "data-channel", "media-source", "media-playout", etc.
    pub stats_type: String,
    /// Unique identifier for this stats object.
    pub id: String,
    /// Timestamp (milliseconds since Unix epoch).
    pub timestamp: i64,
    /// Complete stats serialized as a JSON string.
    pub json_data: String,
}

/// Convert libwebrtc [`RtcStats`](libwebrtc::stats::RtcStats) into NAPI-friendly representations.
pub(crate) fn convert_stats(stats: Vec<libwebrtc::stats::RtcStats>) -> Vec<LkRtcStats> {
    stats.into_iter().map(convert_single_stat).collect()
}

fn convert_single_stat(s: libwebrtc::stats::RtcStats) -> LkRtcStats {
    use libwebrtc::stats::RtcStats;
    match s {
        RtcStats::Codec(ref inner) => LkRtcStats {
            stats_type: "codec".into(),
            id: inner.rtc.id.clone(),
            timestamp: inner.rtc.timestamp,
            json_data: codec_to_json(inner),
        },
        RtcStats::InboundRtp(ref inner) => LkRtcStats {
            stats_type: "inbound-rtp".into(),
            id: inner.rtc.id.clone(),
            timestamp: inner.rtc.timestamp,
            json_data: inbound_rtp_to_json(inner),
        },
        RtcStats::OutboundRtp(ref inner) => LkRtcStats {
            stats_type: "outbound-rtp".into(),
            id: inner.rtc.id.clone(),
            timestamp: inner.rtc.timestamp,
            json_data: outbound_rtp_to_json(inner),
        },
        RtcStats::RemoteInboundRtp(ref inner) => LkRtcStats {
            stats_type: "remote-inbound-rtp".into(),
            id: inner.rtc.id.clone(),
            timestamp: inner.rtc.timestamp,
            json_data: remote_inbound_rtp_to_json(inner),
        },
        RtcStats::RemoteOutboundRtp(ref inner) => LkRtcStats {
            stats_type: "remote-outbound-rtp".into(),
            id: inner.rtc.id.clone(),
            timestamp: inner.rtc.timestamp,
            json_data: remote_outbound_rtp_to_json(inner),
        },
        RtcStats::MediaSource(ref inner) => LkRtcStats {
            stats_type: "media-source".into(),
            id: inner.rtc.id.clone(),
            timestamp: inner.rtc.timestamp,
            json_data: media_source_to_json(inner),
        },
        RtcStats::MediaPlayout(ref inner) => LkRtcStats {
            stats_type: "media-playout".into(),
            id: inner.rtc.id.clone(),
            timestamp: inner.rtc.timestamp,
            json_data: media_playout_to_json(inner),
        },
        RtcStats::PeerConnection(ref inner) => LkRtcStats {
            stats_type: "peer-connection".into(),
            id: inner.rtc.id.clone(),
            timestamp: inner.rtc.timestamp,
            json_data: peer_connection_to_json(inner),
        },
        RtcStats::DataChannel(ref inner) => LkRtcStats {
            stats_type: "data-channel".into(),
            id: inner.rtc.id.clone(),
            timestamp: inner.rtc.timestamp,
            json_data: data_channel_to_json(inner),
        },
        RtcStats::Transport(ref inner) => LkRtcStats {
            stats_type: "transport".into(),
            id: inner.rtc.id.clone(),
            timestamp: inner.rtc.timestamp,
            json_data: transport_to_json(inner),
        },
        RtcStats::CandidatePair(ref inner) => LkRtcStats {
            stats_type: "candidate-pair".into(),
            id: inner.rtc.id.clone(),
            timestamp: inner.rtc.timestamp,
            json_data: candidate_pair_to_json(inner),
        },
        RtcStats::LocalCandidate(ref inner) => LkRtcStats {
            stats_type: "local-candidate".into(),
            id: inner.rtc.id.clone(),
            timestamp: inner.rtc.timestamp,
            json_data: ice_candidate_to_json(&inner.local_candidate),
        },
        RtcStats::RemoteCandidate(ref inner) => LkRtcStats {
            stats_type: "remote-candidate".into(),
            id: inner.rtc.id.clone(),
            timestamp: inner.rtc.timestamp,
            json_data: ice_candidate_to_json(&inner.remote_candidate),
        },
        RtcStats::Certificate(ref inner) => LkRtcStats {
            stats_type: "certificate".into(),
            id: inner.rtc.id.clone(),
            timestamp: inner.rtc.timestamp,
            json_data: certificate_to_json(inner),
        },
        RtcStats::Stream(ref inner) => LkRtcStats {
            stats_type: "stream".into(),
            id: inner.rtc.id.clone(),
            timestamp: inner.rtc.timestamp,
            json_data: stream_to_json(inner),
        },
        RtcStats::Track => LkRtcStats {
            stats_type: "track".into(),
            id: String::new(),
            timestamp: 0,
            json_data: "{}".into(),
        },
    }
}

// --- JSON serialization helpers (manual, since RtcStats lacks Serialize) ---

fn codec_to_json(s: &libwebrtc::stats::CodecStats) -> String {
    serde_json::json!({
        "payloadType": s.codec.payload_type,
        "transportId": s.codec.transport_id,
        "mimeType": s.codec.mime_type,
        "clockRate": s.codec.clock_rate,
        "channels": s.codec.channels,
        "sdpFmtpLine": s.codec.sdp_fmtp_line,
    })
    .to_string()
}

fn inbound_rtp_to_json(s: &libwebrtc::stats::InboundRtpStats) -> String {
    serde_json::json!({
        "ssrc": s.stream.ssrc,
        "kind": s.stream.kind,
        "transportId": s.stream.transport_id,
        "codecId": s.stream.codec_id,
        "packetsReceived": s.received.packets_received,
        "packetsLost": s.received.packets_lost,
        "jitter": s.received.jitter,
        "trackIdentifier": s.inbound.track_identifier,
        "mid": s.inbound.mid,
        "remoteId": s.inbound.remote_id,
        "framesDecoded": s.inbound.frames_decoded,
        "keyFramesDecoded": s.inbound.key_frames_decoded,
        "framesRendered": s.inbound.frames_rendered,
        "framesDropped": s.inbound.frames_dropped,
        "frameWidth": s.inbound.frame_width,
        "frameHeight": s.inbound.frame_height,
        "framesPerSecond": s.inbound.frames_per_second,
        "totalDecodeTime": s.inbound.total_decode_time,
        "totalInterFrameDelay": s.inbound.total_inter_frame_delay,
        "freezeCount": s.inbound.freeze_count,
        "totalFreezeDuration": s.inbound.total_freeze_duration,
        "pauseCount": s.inbound.pause_count,
        "totalPauseDuration": s.inbound.total_pause_duration,
        "bytesReceived": s.inbound.bytes_received,
        "headerBytesReceived": s.inbound.header_bytes_received,
        "nackCount": s.inbound.nack_count,
        "firCount": s.inbound.fir_count,
        "pliCount": s.inbound.pli_count,
        "totalProcessingDelay": s.inbound.total_processing_delay,
        "jitterBufferDelay": s.inbound.jitter_buffer_delay,
        "jitterBufferTargetDelay": s.inbound.jitter_buffer_target_delay,
        "jitterBufferEmittedCount": s.inbound.jitter_buffer_emitted_count,
        "totalSamplesReceived": s.inbound.total_samples_received,
        "concealedSamples": s.inbound.concealed_samples,
        "audioLevel": s.inbound.audio_level,
        "totalAudioEnergy": s.inbound.total_audio_energy,
        "totalSamplesDuration": s.inbound.total_samples_duration,
        "framesReceived": s.inbound.frames_received,
        "decoderImplementation": s.inbound.decoder_implementation,
        "playoutId": s.inbound.playout_id,
    })
    .to_string()
}

fn outbound_rtp_to_json(s: &libwebrtc::stats::OutboundRtpStats) -> String {
    serde_json::json!({
        "ssrc": s.stream.ssrc,
        "kind": s.stream.kind,
        "transportId": s.stream.transport_id,
        "codecId": s.stream.codec_id,
        "packetsSent": s.sent.packets_sent,
        "bytesSent": s.sent.bytes_sent,
        "mid": s.outbound.mid,
        "mediaSourceId": s.outbound.media_source_id,
        "remoteId": s.outbound.remote_id,
        "rid": s.outbound.rid,
        "headerBytesSent": s.outbound.header_bytes_sent,
        "retransmittedPacketsSent": s.outbound.retransmitted_packets_sent,
        "retransmittedBytesSent": s.outbound.retransmitted_bytes_sent,
        "targetBitrate": s.outbound.target_bitrate,
        "frameWidth": s.outbound.frame_width,
        "frameHeight": s.outbound.frame_height,
        "framesPerSecond": s.outbound.frames_per_second,
        "framesSent": s.outbound.frames_sent,
        "framesEncoded": s.outbound.frames_encoded,
        "keyFramesEncoded": s.outbound.key_frames_encoded,
        "qpSum": s.outbound.qp_sum,
        "totalEncodeTime": s.outbound.total_encode_time,
        "totalPacketSendDelay": s.outbound.total_packet_send_delay,
        "qualityLimitationReason": format!("{:?}", s.outbound.quality_limitation_reason),
        "qualityLimitationResolutionChanges": s.outbound.quality_limitation_resolution_changes,
        "nackCount": s.outbound.nack_count,
        "firCount": s.outbound.fir_count,
        "pliCount": s.outbound.pli_count,
        "encoderImplementation": s.outbound.encoder_implementation,
        "active": s.outbound.active,
        "scalabilityMode": s.outbound.scalibility_mode,
    })
    .to_string()
}

fn remote_inbound_rtp_to_json(s: &libwebrtc::stats::RemoteInboundRtpStats) -> String {
    serde_json::json!({
        "ssrc": s.stream.ssrc,
        "kind": s.stream.kind,
        "transportId": s.stream.transport_id,
        "codecId": s.stream.codec_id,
        "packetsReceived": s.received.packets_received,
        "packetsLost": s.received.packets_lost,
        "jitter": s.received.jitter,
        "localId": s.remote_inbound.local_id,
        "roundTripTime": s.remote_inbound.round_trip_time,
        "totalRoundTripTime": s.remote_inbound.total_round_trip_time,
        "fractionLost": s.remote_inbound.fraction_lost,
        "roundTripTimeMeasurements": s.remote_inbound.round_trip_time_measurements,
    })
    .to_string()
}

fn remote_outbound_rtp_to_json(s: &libwebrtc::stats::RemoteOutboundRtpStats) -> String {
    serde_json::json!({
        "ssrc": s.stream.ssrc,
        "kind": s.stream.kind,
        "transportId": s.stream.transport_id,
        "codecId": s.stream.codec_id,
        "packetsSent": s.sent.packets_sent,
        "bytesSent": s.sent.bytes_sent,
        "localId": s.remote_outbound.local_id,
        "remoteTimestamp": s.remote_outbound.remote_timestamp,
        "reportsSent": s.remote_outbound.reports_sent,
        "roundTripTime": s.remote_outbound.round_trip_time,
        "totalRoundTripTime": s.remote_outbound.total_round_trip_time,
        "roundTripTimeMeasurements": s.remote_outbound.round_trip_time_measurements,
    })
    .to_string()
}

fn media_source_to_json(s: &libwebrtc::stats::MediaSourceStats) -> String {
    serde_json::json!({
        "trackIdentifier": s.source.track_identifier,
        "kind": s.source.kind,
        "audioLevel": s.audio.audio_level,
        "totalAudioEnergy": s.audio.total_audio_energy,
        "totalSamplesDuration": s.audio.total_samples_duration,
        "echoReturnLoss": s.audio.echo_return_loss,
        "echoReturnLossEnhancement": s.audio.echo_return_loss_enhancement,
        "width": s.video.width,
        "height": s.video.height,
        "frames": s.video.frames,
        "framesPerSecond": s.video.frames_per_second,
    })
    .to_string()
}

fn media_playout_to_json(s: &libwebrtc::stats::MediaPlayoutStats) -> String {
    serde_json::json!({
        "kind": s.audio_playout.kind,
        "synthesizedSamplesDuration": s.audio_playout.synthesized_samples_duration,
        "synthesizedSamplesEvents": s.audio_playout.synthesized_samples_events,
        "totalSamplesDuration": s.audio_playout.total_samples_duration,
        "totalPlayoutDelay": s.audio_playout.total_playout_delay,
        "totalSamplesCount": s.audio_playout.total_samples_count,
    })
    .to_string()
}

fn peer_connection_to_json(s: &libwebrtc::stats::PeerConnectionStats) -> String {
    serde_json::json!({
        "dataChannelsOpened": s.pc.data_channels_opened,
        "dataChannelsClosed": s.pc.data_channels_closed,
    })
    .to_string()
}

fn data_channel_to_json(s: &libwebrtc::stats::DataChannelStats) -> String {
    serde_json::json!({
        "label": s.dc.label,
        "protocol": s.dc.protocol,
        "dataChannelIdentifier": s.dc.data_channel_identifier,
        "state": s.dc.state.as_ref().map(|st| format!("{:?}", st)),
        "messagesSent": s.dc.messages_sent,
        "bytesSent": s.dc.bytes_sent,
        "messagesReceived": s.dc.messages_received,
        "bytesReceived": s.dc.bytes_received,
    })
    .to_string()
}

fn transport_to_json(s: &libwebrtc::stats::TransportStats) -> String {
    serde_json::json!({
        "packetsSent": s.transport.packets_sent,
        "packetsReceived": s.transport.packets_received,
        "bytesSent": s.transport.bytes_sent,
        "bytesReceived": s.transport.bytes_received,
        "iceRole": format!("{:?}", s.transport.ice_role),
        "iceLocalUsernameFragment": s.transport.ice_local_username_fragment,
        "dtlsState": s.transport.dtls_state.as_ref().map(|st| format!("{:?}", st)),
        "iceState": s.transport.ice_state.as_ref().map(|st| format!("{:?}", st)),
        "selectedCandidatePairId": s.transport.selected_candidate_pair_id,
        "localCertificateId": s.transport.local_certificate_id,
        "remoteCertificateId": s.transport.remote_certificate_id,
        "tlsVersion": s.transport.tls_version,
        "dtlsCipher": s.transport.dtls_cipher,
        "dtlsRole": format!("{:?}", s.transport.dtls_role),
        "srtpCipher": s.transport.srtp_cipher,
        "selectedCandidatePairChanges": s.transport.selected_candidate_pair_changes,
    })
    .to_string()
}

fn candidate_pair_to_json(s: &libwebrtc::stats::CandidatePairStats) -> String {
    serde_json::json!({
        "transportId": s.candidate_pair.transport_id,
        "localCandidateId": s.candidate_pair.local_candidate_id,
        "remoteCandidateId": s.candidate_pair.remote_candidate_id,
        "state": s.candidate_pair.state.as_ref().map(|st| format!("{:?}", st)),
        "nominated": s.candidate_pair.nominated,
        "packetsSent": s.candidate_pair.packets_sent,
        "packetsReceived": s.candidate_pair.packets_received,
        "bytesSent": s.candidate_pair.bytes_sent,
        "bytesReceived": s.candidate_pair.bytes_received,
        "lastPacketSentTimestamp": s.candidate_pair.last_packet_sent_timestamp,
        "lastPacketReceivedTimestamp": s.candidate_pair.last_packet_received_timestamp,
        "totalRoundTripTime": s.candidate_pair.total_round_trip_time,
        "currentRoundTripTime": s.candidate_pair.current_round_trip_time,
        "availableOutgoingBitrate": s.candidate_pair.available_outgoing_bitrate,
        "availableIncomingBitrate": s.candidate_pair.available_incoming_bitrate,
        "requestsReceived": s.candidate_pair.requests_received,
        "requestsSent": s.candidate_pair.requests_sent,
        "responsesReceived": s.candidate_pair.responses_received,
        "responsesSent": s.candidate_pair.responses_sent,
        "consentRequestsSent": s.candidate_pair.consent_requests_sent,
        "packetsDiscardedOnSend": s.candidate_pair.packets_discarded_on_send,
        "bytesDiscardedOnSend": s.candidate_pair.bytes_discarded_on_send,
    })
    .to_string()
}

fn ice_candidate_to_json(s: &libwebrtc::stats::dictionaries::IceCandidateStats) -> String {
    serde_json::json!({
        "transportId": s.transport_id,
        "address": s.address,
        "port": s.port,
        "protocol": s.protocol,
        "candidateType": s.candidate_type.as_ref().map(|ct| format!("{:?}", ct)),
        "priority": s.priority,
        "url": s.url,
        "relayProtocol": s.relay_protocol.as_ref().map(|rp| format!("{:?}", rp)),
        "foundation": s.foundation,
        "relatedAddress": s.related_address,
        "relatedPort": s.related_port,
        "usernameFragment": s.username_fragment,
        "tcpType": s.tcp_type.as_ref().map(|tt| format!("{:?}", tt)),
    })
    .to_string()
}

fn certificate_to_json(s: &libwebrtc::stats::CertificateStats) -> String {
    serde_json::json!({
        "fingerprint": s.certificate.fingerprint,
        "fingerprintAlgorithm": s.certificate.fingerprint_algorithm,
        "base64Certificate": s.certificate.base64_certificate,
        "issuerCertificateId": s.certificate.issuer_certificate_id,
    })
    .to_string()
}

fn stream_to_json(s: &libwebrtc::stats::StreamStats) -> String {
    serde_json::json!({
        "streamIdentifier": s.stream.stream_identifier,
    })
    .to_string()
}
