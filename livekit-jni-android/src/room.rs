//! SFU Room JNI wrapper — mirrors `livekit-napi-ohos/src/room.rs` (LkRoom).
//!
//! Provides LiveKit SFU room connectivity for Android.
//! All complex types cross the JNI boundary as JSON strings.
//!
//! Kotlin class: `cn.tdcare.smartward.rust.sfu.LkRoom`

use std::sync::Arc;

use jni::objects::{JClass, JObject, JString};
use jni::sys::{jboolean, jint, jlong, jstring};
use jni::JNIEnv;
use livekit::prelude::*;
use livekit::{ConnectionState, RoomEvent, RoomOptions};
use parking_lot::Mutex as ParkingMutex;
use tokio::sync::{mpsc, Mutex};

use crate::audio_stream::AudioStreamHandle;
use crate::{catch_panic, get_runtime, jstring_to_rust, rust_to_jstring, throw_runtime_exception};

// ============================================================
// Room state — stored as opaque pointer (jlong)
// ============================================================

/// Internal room state.
pub struct RoomState {
    pub room: Arc<ParkingMutex<Option<Arc<livekit::Room>>>>,
    pub event_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<RoomEvent>>>>,
    /// Buffered events for polling
    pub event_buffer: Arc<ParkingMutex<Vec<serde_json::Value>>>,
}

impl RoomState {
    fn new() -> Self {
        Self {
            room: Arc::new(ParkingMutex::new(None)),
            event_rx: Arc::new(Mutex::new(None)),
            event_buffer: Arc::new(ParkingMutex::new(Vec::new())),
        }
    }

    /// Start the event forwarding loop that buffers events for polling.
    fn start_event_loop(&self) {
        let event_rx = self.event_rx.clone();
        let event_buffer = self.event_buffer.clone();
        let runtime = get_runtime();

        runtime.spawn(async move {
            let mut rx = {
                let mut guard = event_rx.lock().await;
                match guard.take() {
                    Some(rx) => rx,
                    None => return,
                }
            };

            while let Some(event) = rx.recv().await {
                let json_event = room_event_to_json(&event);
                event_buffer.lock().push(json_event);
            }
            log::info!("[JNI Room] event loop ended");
        });
    }
}

/// Convert a RoomEvent to a JSON value for polling.
fn room_event_to_json(event: &RoomEvent) -> serde_json::Value {
    match event {
        RoomEvent::ParticipantConnected(participant) => {
            serde_json::json!({
                "type": "participant_connected",
                "identity": participant.identity().as_str(),
            })
        }
        RoomEvent::ParticipantDisconnected(participant) => {
            serde_json::json!({
                "type": "participant_disconnected",
                "identity": participant.identity().as_str(),
            })
        }
        RoomEvent::TrackSubscribed { track, publication, participant } => {
            let kind = match track {
                RemoteTrack::Audio(_) => "audio",
                RemoteTrack::Video(_) => "video",
            };
            serde_json::json!({
                "type": "track_subscribed",
                "identity": participant.identity().as_str(),
                "track_sid": publication.sid().to_string(),
                "kind": kind,
            })
        }
        RoomEvent::TrackUnsubscribed { track, publication, participant } => {
            let kind = match track {
                RemoteTrack::Audio(_) => "audio",
                RemoteTrack::Video(_) => "video",
            };
            serde_json::json!({
                "type": "track_unsubscribed",
                "identity": participant.identity().as_str(),
                "track_sid": publication.sid().to_string(),
                "kind": kind,
            })
        }
        RoomEvent::Disconnected { reason } => {
            serde_json::json!({
                "type": "disconnected",
                "reason": format!("{:?}", reason),
            })
        }
        RoomEvent::Reconnecting => {
            serde_json::json!({ "type": "reconnecting" })
        }
        RoomEvent::Reconnected => {
            serde_json::json!({ "type": "reconnected" })
        }
        _ => {
            serde_json::json!({ "type": "other" })
        }
    }
}

// ============================================================
// JNI Methods — cn.tdcare.smartward.rust.sfu.LkRoom
// ============================================================

/// Create a new LkRoom instance.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkRoom_nativeNew(
    mut env: JNIEnv,
    _class: JClass,
) -> jlong {
    crate::init_logger();
    catch_panic(
        0i64,
        || {
            let state = Box::new(RoomState::new());
            let ptr = Box::into_raw(state) as jlong;
            log::info!("[JNI Room] nativeNew: ptr={}", ptr);
            ptr
        },
    )
}

/// Connect to a LiveKit SFU server.
///
/// `url` - WebSocket URL (e.g. "ws://192.168.1.100:7880")
/// `token` - JWT access token
/// `options_json` - JSON: {"auto_subscribe":true,"adaptive_stream":false,"dynacast":false}
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkRoom_nativeConnect(
    mut env: JNIEnv,
    _class: JClass,
    room_ptr: jlong,
    url: JString,
    token: JString,
    options_json: JString,
) {
    catch_panic((), || {
        let state = unsafe { &*(room_ptr as *const RoomState) };
        let runtime = get_runtime();

        let url_str: String = jstring_to_rust(&mut env, &url).unwrap_or_default();
        let token_str: String = jstring_to_rust(&mut env, &token).unwrap_or_default();
        let options_str: String = jstring_to_rust(&mut env, &options_json)
            .unwrap_or_else(|_| "{}".to_string());

        // Parse options
        let mut room_options = RoomOptions::default();
        if let Ok(opts) = serde_json::from_str::<serde_json::Value>(&options_str) {
            if let Some(v) = opts.get("auto_subscribe").and_then(|v| v.as_bool()) {
                room_options.auto_subscribe = v;
            }
            if let Some(v) = opts.get("adaptive_stream").and_then(|v| v.as_bool()) {
                room_options.adaptive_stream = v;
            }
            if let Some(v) = opts.get("dynacast").and_then(|v| v.as_bool()) {
                room_options.dynacast = v;
            }
        }

        log::info!("[JNI Room] connecting: url={}, token_len={}", url_str, token_str.len());

        // Perform async connect
        let room_clone = state.room.clone();
        let event_rx_clone = state.event_rx.clone();

        let result = runtime.block_on(async {
            match livekit::Room::connect(&url_str, &token_str, room_options).await {
                Ok((room, event_rx)) => {
                    log::info!("[JNI Room] connected: name={}", room.name());
                    *room_clone.lock() = Some(Arc::new(room));
                    *event_rx_clone.lock().await = Some(event_rx);
                    Ok(())
                }
                Err(e) => {
                    log::error!("[JNI Room] connect failed: {:?}", e);
                    Err(format!("connect failed: {}", e))
                }
            }
        });

        if let Err(msg) = result {
            throw_runtime_exception(&mut env, &msg);
            return;
        }

        // Start event loop for polling
        state.start_event_loop();
    });
}

/// Disconnect from the room.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkRoom_nativeDisconnect(
    mut env: JNIEnv,
    _class: JClass,
    room_ptr: jlong,
) {
    catch_panic((), || {
        let state = unsafe { &*(room_ptr as *const RoomState) };
        let runtime = get_runtime();

        let room = state.room.lock().clone();
        if let Some(r) = room {
            runtime.block_on(async {
                if let Err(e) = r.close().await {
                    log::error!("[JNI Room] disconnect error: {}", e);
                }
            });
        }
        log::info!("[JNI Room] disconnected");
    });
}

/// Poll buffered room events.
/// Returns JSON array of events accumulated since last poll.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkRoom_nativePollEvents(
    mut env: JNIEnv,
    _class: JClass,
    room_ptr: jlong,
) -> jstring {
    let result = catch_panic(
        "[]".to_string(),
        || {
            let state = unsafe { &*(room_ptr as *const RoomState) };
            let mut buffer = state.event_buffer.lock();
            if buffer.is_empty() {
                return "[]".to_string();
            }
            let events: Vec<serde_json::Value> = buffer.drain(..).collect();
            serde_json::to_string(&events).unwrap_or_else(|_| "[]".into())
        },
    );

    rust_to_jstring(&mut env, &result)
        .unwrap_or_else(|_| JObject::null().into())
        .into_raw()
}

/// Get connection state: "connected", "disconnected", or "reconnecting".
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkRoom_nativeConnectionState(
    mut env: JNIEnv,
    _class: JClass,
    room_ptr: jlong,
) -> jstring {
    let state_str = catch_panic(
        "disconnected".to_string(),
        || {
            let state = unsafe { &*(room_ptr as *const RoomState) };
            let guard = state.room.lock();
            match guard.as_ref() {
                Some(r) => match r.connection_state() {
                    ConnectionState::Connected => "connected".to_string(),
                    ConnectionState::Disconnected => "disconnected".to_string(),
                    ConnectionState::Reconnecting => "reconnecting".to_string(),
                },
                None => "disconnected".to_string(),
            }
        },
    );

    rust_to_jstring(&mut env, &state_str)
        .unwrap_or_else(|_| JObject::null().into())
        .into_raw()
}

/// Get remote participant identities as JSON array.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkRoom_nativeGetRemoteParticipants(
    mut env: JNIEnv,
    _class: JClass,
    room_ptr: jlong,
) -> jstring {
    let result = catch_panic(
        "[]".to_string(),
        || {
            let state = unsafe { &*(room_ptr as *const RoomState) };
            let guard = state.room.lock();
            match guard.as_ref() {
                Some(room) => {
                    let identities: Vec<String> = room
                        .remote_participants()
                        .keys()
                        .map(|id| id.as_str().to_string())
                        .collect();
                    serde_json::to_string(&identities).unwrap_or_else(|_| "[]".into())
                }
                None => "[]".to_string(),
            }
        },
    );

    rust_to_jstring(&mut env, &result)
        .unwrap_or_else(|_| JObject::null().into())
        .into_raw()
}

/// Get remote audio track SIDs for a participant.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkRoom_nativeGetRemoteAudioTrackSids(
    mut env: JNIEnv,
    _class: JClass,
    room_ptr: jlong,
    participant_identity: JString,
) -> jstring {
    let result = catch_panic(
        "[]".to_string(),
        || {
            let state = unsafe { &*(room_ptr as *const RoomState) };
            let identity_str: String = jstring_to_rust(&mut env, &participant_identity)
                .unwrap_or_default();

            let guard = state.room.lock();
            let room = match guard.as_ref() {
                Some(r) => r,
                None => return "[]".to_string(),
            };

            let identity: ParticipantIdentity = identity_str.clone().into();
            let participant = match room.remote_participants().get(&identity) {
                Some(p) => p.clone(),
                None => return "[]".to_string(),
            };

            let sids: Vec<String> = participant
                .track_publications()
                .values()
                .filter_map(|pub_| {
                    let track = pub_.track()?;
                    match track {
                        RemoteTrack::Audio(_) => Some(pub_.sid().to_string()),
                        _ => None,
                    }
                })
                .collect();

            serde_json::to_string(&sids).unwrap_or_else(|_| "[]".into())
        },
    );

    rust_to_jstring(&mut env, &result)
        .unwrap_or_else(|_| JObject::null().into())
        .into_raw()
}

/// Create an audio stream from a remote participant's audio track.
/// Returns an opaque stream handle for use with LkAudioStream methods.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkRoom_nativeCreateAudioStream(
    mut env: JNIEnv,
    _class: JClass,
    room_ptr: jlong,
    participant_identity: JString,
    track_sid: JString,
) -> jlong {
    catch_panic(
        0i64,
        || {
            let state = unsafe { &*(room_ptr as *const RoomState) };
            let identity_str: String = jstring_to_rust(&mut env, &participant_identity)
                .unwrap_or_default();
            let track_sid_str: String = jstring_to_rust(&mut env, &track_sid)
                .unwrap_or_default();

            let guard = state.room.lock();
            let room = match guard.as_ref() {
                Some(r) => r,
                None => {
                    throw_runtime_exception(&mut env, "room not connected");
                    return 0;
                }
            };

            let identity: ParticipantIdentity = identity_str.clone().into();
            let participant = match room.remote_participants().get(&identity) {
                Some(p) => p.clone(),
                None => {
                    throw_runtime_exception(&mut env, &format!("participant '{}' not found", identity_str));
                    return 0;
                }
            };

            let tsid: TrackSid = match track_sid_str.clone().try_into() {
                Ok(s) => s,
                Err(_) => {
                    throw_runtime_exception(&mut env, &format!("invalid track_sid '{}'", track_sid_str));
                    return 0;
                }
            };

            let publication = match participant.get_track_publication(&tsid) {
                Some(p) => p,
                None => {
                    throw_runtime_exception(&mut env, &format!("track '{}' not found", track_sid_str));
                    return 0;
                }
            };

            let remote_track = match publication.track() {
                Some(t) => t,
                None => {
                    throw_runtime_exception(&mut env, "track not subscribed yet");
                    return 0;
                }
            };

            match remote_track {
                RemoteTrack::Audio(audio_track) => {
                    let native = libwebrtc::audio_stream::native::NativeAudioStream::new(
                        audio_track.rtc_track(),
                        48_000,
                        1,
                    );
                    let stream_handle = AudioStreamHandle::new(native);
                    let ptr = Box::into_raw(Box::new(stream_handle)) as jlong;
                    log::info!(
                        "[JNI Room] createAudioStream: participant={}, track={}, stream={}",
                        identity_str,
                        track_sid_str,
                        ptr
                    );
                    ptr
                }
                _ => {
                    throw_runtime_exception(&mut env, "track is not audio");
                    0
                }
            }
        },
    )
}

/// Destroy the room and free all resources.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkRoom_nativeDestroy(
    mut env: JNIEnv,
    _class: JClass,
    room_ptr: jlong,
) {
    catch_panic((), || {
        if room_ptr == 0 {
            return;
        }
        let state = unsafe { Box::from_raw(room_ptr as *mut RoomState) };
        let runtime = get_runtime();

        // Disconnect if connected
        let room = state.room.lock().clone();
        if let Some(r) = room {
            runtime.block_on(async {
                let _ = r.close().await;
            });
        }
        log::info!("[JNI Room] destroyed: ptr={}", room_ptr);
    });
}

// ============================================================
// Video/Audio Track Publish & Subscribe (Phase V-3)
// ============================================================

/// Publish a local video track to the room.
///
/// `video_source_ptr` - pointer to VideoSourceState (from LkVideoSource)
/// `track_name` - display name for the track
/// Returns the published track SID as a string.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkRoom_nativePublishVideoTrack(
    mut env: JNIEnv,
    _class: JClass,
    room_ptr: jlong,
    video_source_ptr: jlong,
    track_name: JString,
) -> jstring {
    let result: String = catch_panic(String::new(), || {
        use livekit::options::{TrackPublishOptions, VideoCodec};
        use livekit::prelude::*;
        use livekit::webrtc::video_source::RtcVideoSource;

        if room_ptr == 0 || video_source_ptr == 0 {
            throw_runtime_exception(&mut env, "room or video source not initialized");
            return String::new();
        }

        let state = unsafe { &*(room_ptr as *const RoomState) };
        let vs_state = unsafe { &*(video_source_ptr as *const crate::video_source::VideoSourceState) };
        let name: String = jstring_to_rust(&mut env, &track_name).unwrap_or_else(|_| "camera".into());

        let guard = state.room.lock();
        let room = match guard.as_ref() {
            Some(r) => r.clone(),
            None => {
                throw_runtime_exception(&mut env, "room not connected");
                return String::new();
            }
        };
        drop(guard);

        // Create local video track from the NativeVideoSource
        let rtc_source = RtcVideoSource::Native(vs_state.source.clone());
        let local_track = LocalVideoTrack::create_video_track(&name, rtc_source);

        let options = TrackPublishOptions {
            video_codec: VideoCodec::H264,
            source: TrackSource::Camera,
            simulcast: false,
            ..Default::default()
        };

        let runtime = get_runtime();
        let publication = runtime.block_on(async {
            room.local_participant()
                .publish_track(LocalTrack::Video(local_track), options)
                .await
        });

        match publication {
            Ok(pub_) => {
                let sid = pub_.sid().to_string();
                log::info!("[JNI Room] publishVideoTrack OK: name={}, sid={}", name, sid);
                sid
            }
            Err(e) => {
                throw_runtime_exception(&mut env, &format!("publish video failed: {}", e));
                String::new()
            }
        }
    });

    rust_to_jstring(&mut env, &result)
        .unwrap_or_else(|_| JObject::null().into())
        .into_raw()
}

/// Publish a local audio track to the room.
///
/// `audio_source_ptr` - pointer to a NativeAudioSource (from LkAudioSource)
/// `track_name` - display name for the track
/// Returns the published track SID.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkRoom_nativePublishAudioTrack(
    mut env: JNIEnv,
    _class: JClass,
    room_ptr: jlong,
    audio_source_ptr: jlong,
    track_name: JString,
) -> jstring {
    let result: String = catch_panic(String::new(), || {
        use livekit::options::TrackPublishOptions;
        use livekit::prelude::*;
        use livekit::webrtc::audio_source::RtcAudioSource;
        use livekit::webrtc::audio_source::native::NativeAudioSource;

        if room_ptr == 0 || audio_source_ptr == 0 {
            throw_runtime_exception(&mut env, "room or audio source not initialized");
            return String::new();
        }

        let state = unsafe { &*(room_ptr as *const RoomState) };
        let native_source = unsafe { &*(audio_source_ptr as *const NativeAudioSource) };
        let name: String = jstring_to_rust(&mut env, &track_name).unwrap_or_else(|_| "microphone".into());

        let guard = state.room.lock();
        let room = match guard.as_ref() {
            Some(r) => r.clone(),
            None => {
                throw_runtime_exception(&mut env, "room not connected");
                return String::new();
            }
        };
        drop(guard);

        let rtc_source = RtcAudioSource::Native(native_source.clone());
        let local_track = LocalAudioTrack::create_audio_track(&name, rtc_source);

        let options = TrackPublishOptions {
            source: TrackSource::Microphone,
            ..Default::default()
        };

        let runtime = get_runtime();
        let publication = runtime.block_on(async {
            room.local_participant()
                .publish_track(LocalTrack::Audio(local_track), options)
                .await
        });

        match publication {
            Ok(pub_) => {
                let sid = pub_.sid().to_string();
                log::info!("[JNI Room] publishAudioTrack OK: name={}, sid={}", name, sid);
                sid
            }
            Err(e) => {
                throw_runtime_exception(&mut env, &format!("publish audio failed: {}", e));
                String::new()
            }
        }
    });

    rust_to_jstring(&mut env, &result)
        .unwrap_or_else(|_| JObject::null().into())
        .into_raw()
}

/// Unpublish a track by its SID.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkRoom_nativeUnpublishTrack(
    mut env: JNIEnv,
    _class: JClass,
    room_ptr: jlong,
    track_sid: JString,
) {
    catch_panic((), || {
        use livekit::prelude::*;

        if room_ptr == 0 {
            return;
        }

        let state = unsafe { &*(room_ptr as *const RoomState) };
        let sid_str: String = jstring_to_rust(&mut env, &track_sid).unwrap_or_default();

        let guard = state.room.lock();
        let room = match guard.as_ref() {
            Some(r) => r.clone(),
            None => return,
        };
        drop(guard);

        let tsid: TrackSid = match sid_str.clone().try_into() {
            Ok(s) => s,
            Err(_) => {
                throw_runtime_exception(&mut env, &format!("invalid track sid: {}", sid_str));
                return;
            }
        };

        let runtime = get_runtime();
        runtime.block_on(async {
            if let Err(e) = room.local_participant().unpublish_track(&tsid).await {
                log::error!("[JNI Room] unpublishTrack failed: {}", e);
            } else {
                log::info!("[JNI Room] unpublishTrack OK: sid={}", sid_str);
            }
        });
    });
}

/// Get remote video track SIDs for a participant.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkRoom_nativeGetRemoteVideoTrackSids(
    mut env: JNIEnv,
    _class: JClass,
    room_ptr: jlong,
    participant_identity: JString,
) -> jstring {
    let result = catch_panic(
        "[]".to_string(),
        || {
            use livekit::prelude::*;

            let state = unsafe { &*(room_ptr as *const RoomState) };
            let identity_str: String = jstring_to_rust(&mut env, &participant_identity)
                .unwrap_or_default();

            let guard = state.room.lock();
            let room = match guard.as_ref() {
                Some(r) => r,
                None => return "[]".to_string(),
            };

            let identity: ParticipantIdentity = identity_str.clone().into();
            let participant = match room.remote_participants().get(&identity) {
                Some(p) => p.clone(),
                None => return "[]".to_string(),
            };

            let sids: Vec<String> = participant
                .track_publications()
                .values()
                .filter_map(|pub_| {
                    let track = pub_.track()?;
                    match track {
                        RemoteTrack::Video(_) => Some(pub_.sid().to_string()),
                        _ => None,
                    }
                })
                .collect();

            serde_json::to_string(&sids).unwrap_or_else(|_| "[]".into())
        },
    );

    rust_to_jstring(&mut env, &result)
        .unwrap_or_else(|_| JObject::null().into())
        .into_raw()
}

/// Mute or unmute the local camera (video track).
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkRoom_nativeSetCameraMuted(
    mut env: JNIEnv,
    _class: JClass,
    room_ptr: jlong,
    muted: jni::sys::jboolean,
) {
    catch_panic((), || {
        use livekit::prelude::*;

        if room_ptr == 0 {
            return;
        }

        let state = unsafe { &*(room_ptr as *const RoomState) };
        let guard = state.room.lock();
        let room = match guard.as_ref() {
            Some(r) => r.clone(),
            None => return,
        };
        drop(guard);

        let is_muted = muted != 0;
        let runtime = get_runtime();
        runtime.spawn(async move {
            let participant = room.local_participant();
            for pub_ in participant.track_publications().values() {
                if pub_.kind() == TrackKind::Video {
                    if is_muted {
                        pub_.mute();
                    } else {
                        pub_.unmute();
                    }
                }
            }
            log::info!("[JNI Room] setCameraMuted: {}", is_muted);
        });
    });
}

/// Mute or unmute the local microphone (audio track).
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkRoom_nativeSetMicrophoneMuted(
    mut env: JNIEnv,
    _class: JClass,
    room_ptr: jlong,
    muted: jni::sys::jboolean,
) {
    catch_panic((), || {
        use livekit::prelude::*;

        if room_ptr == 0 {
            return;
        }

        let state = unsafe { &*(room_ptr as *const RoomState) };
        let guard = state.room.lock();
        let room = match guard.as_ref() {
            Some(r) => r.clone(),
            None => return,
        };
        drop(guard);

        let is_muted = muted != 0;
        let runtime = get_runtime();
        runtime.spawn(async move {
            let participant = room.local_participant();
            for pub_ in participant.track_publications().values() {
                if pub_.kind() == TrackKind::Audio {
                    if is_muted {
                        pub_.mute();
                    } else {
                        pub_.unmute();
                    }
                }
            }
            log::info!("[JNI Room] setMicrophoneMuted: {}", is_muted);
        });
    });
}
