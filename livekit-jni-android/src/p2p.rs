//! P2P WebRTC engine — JNI wrapper around [`p2p::WebRtcEngine`].
//!
//! Mirrors `livekit-napi-ohos/src/p2p.rs` (LkSwcEngine) for Android.
//! All complex types cross the JNI boundary as JSON strings.
//!
//! Kotlin class: `cn.tdcare.smartward.rust.p2p.SwcEngine`

use std::sync::Arc;

use jni::objects::{JByteArray, JClass, JObject, JString};
use jni::sys::{jbyteArray, jint, jlong, jstring};
use jni::JNIEnv;
use parking_lot::Mutex;
use p2p::{
    AecConfig, EngineEvent, IceCandidate, P2pConfig, PeerHandle, RtcAudioTrack, SessionDescription,
    WebRtcEngine,
};

use libwebrtc::audio_stream::native::NativeAudioStream;

use crate::audio_stream::AudioStreamHandle;
use crate::{catch_panic, get_runtime, jstring_to_rust, rust_to_jstring, throw_runtime_exception};

// ============================================================
// Engine state — stored as opaque pointer (jlong)
// ============================================================

/// Internal engine state shared across JNI calls.
struct EngineState {
    runtime: Arc<tokio::runtime::Runtime>,
    engine: Mutex<WebRtcEngine>,
}

impl EngineState {
    fn new() -> Self {
        let runtime = get_runtime();
        Self {
            runtime,
            engine: Mutex::new(WebRtcEngine::new()),
        }
    }
}

// ============================================================
// JNI Methods — cn.tdcare.smartward.rust.p2p.SwcEngine
// ============================================================

/// Create a new SwcEngine instance.
/// Returns an opaque handle (pointer) to be passed to all other methods.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_p2p_SwcEngine_nativeNew(
    mut env: JNIEnv,
    _class: JClass,
) -> jlong {
    crate::init_logger();
    catch_panic(
        0i64,
        || {
            let state = Box::new(EngineState::new());
            let ptr = Box::into_raw(state) as jlong;
            log::info!("[JNI] SwcEngine.nativeNew: handle={}", ptr);
            ptr
        },
    )
}

/// Destroy the engine and free all resources.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_p2p_SwcEngine_nativeDestroy(
    mut env: JNIEnv,
    _class: JClass,
    engine_ptr: jlong,
) {
    catch_panic((), || {
        if engine_ptr == 0 {
            return;
        }
        let state = unsafe { Box::from_raw(engine_ptr as *mut EngineState) };
        let _guard = state.runtime.enter();
        state.engine.lock().shutdown();
        log::info!("[JNI] SwcEngine.nativeDestroy: handle={}", engine_ptr);
    });
}

/// Create a P2P PeerConnection.
///
/// `config_json` - JSON P2pConfig, pass "{}" for defaults.
/// Returns connection handle (> 0), or 0 on error.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_p2p_SwcEngine_nativeCreateP2p(
    mut env: JNIEnv,
    _class: JClass,
    engine_ptr: jlong,
    config_json: JString,
) -> jlong {
    catch_panic(
        0i64,
        || {
            let state = unsafe { &*(engine_ptr as *const EngineState) };
            let _guard = state.runtime.enter();

            let config_str: String = jstring_to_rust(&mut env, &config_json)
                .unwrap_or_else(|_| "{}".to_string());

            let config: P2pConfig = if config_str.is_empty() || config_str == "{}" {
                P2pConfig::default()
            } else {
                match serde_json::from_str(&config_str) {
                    Ok(c) => c,
                    Err(e) => {
                        throw_runtime_exception(&mut env, &format!("invalid config JSON: {}", e));
                        return 0;
                    }
                }
            };

            let mut engine = state.engine.lock();
            let handle = engine.create_p2p_connection(&config);
            let h = handle.as_u64() as jlong;
            log::info!("[JNI] createP2p: engine={}, handle={}", engine_ptr, h);
            h
        },
    )
}

/// Create an Offer SDP for a P2P connection.
/// Returns JSON string of SessionDescription.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_p2p_SwcEngine_nativeCreateOffer(
    mut env: JNIEnv,
    _class: JClass,
    engine_ptr: jlong,
    handle: jlong,
) -> jstring {
    let result = catch_panic(
        None::<String>,
        || {
            let state = unsafe { &*(engine_ptr as *const EngineState) };
            let _guard = state.runtime.enter();
            let mut engine = state.engine.lock();

            match engine.create_offer(PeerHandle::from(handle as u64)) {
                Ok(sdp) => Some(serde_json::to_string(&sdp).unwrap_or_else(|_| "{}".into())),
                Err(e) => {
                    log::error!("[JNI] createOffer error: {}", e);
                    throw_runtime_exception(&mut env, &format!("createOffer failed: {}", e));
                    None
                }
            }
        },
    );

    match result {
        Some(json) => rust_to_jstring(&mut env, &json)
            .unwrap_or_else(|_| JObject::null().into())
            .into_raw(),
        None => std::ptr::null_mut(),
    }
}

/// Create an Answer SDP (after receiving remote Offer).
///
/// `offer_json` - remote Offer JSON string.
/// Returns Answer JSON string.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_p2p_SwcEngine_nativeCreateAnswer(
    mut env: JNIEnv,
    _class: JClass,
    engine_ptr: jlong,
    handle: jlong,
    offer_json: JString,
) -> jstring {
    let result = catch_panic(
        None::<String>,
        || {
            let state = unsafe { &*(engine_ptr as *const EngineState) };
            let _guard = state.runtime.enter();

            let offer_str: String = jstring_to_rust(&mut env, &offer_json)
                .unwrap_or_else(|_| "{}".to_string());

            let offer: SessionDescription = match serde_json::from_str(&offer_str) {
                Ok(s) => s,
                Err(e) => {
                    throw_runtime_exception(&mut env, &format!("invalid offer JSON: {}", e));
                    return None;
                }
            };

            let mut engine = state.engine.lock();
            match engine.create_answer(PeerHandle::from(handle as u64), &offer) {
                Ok(answer) => {
                    Some(serde_json::to_string(&answer).unwrap_or_else(|_| "{}".into()))
                }
                Err(e) => {
                    log::error!("[JNI] createAnswer error: {}", e);
                    throw_runtime_exception(&mut env, &format!("createAnswer failed: {}", e));
                    None
                }
            }
        },
    );

    match result {
        Some(json) => rust_to_jstring(&mut env, &json)
            .unwrap_or_else(|_| JObject::null().into())
            .into_raw(),
        None => std::ptr::null_mut(),
    }
}

/// Set remote SDP for a P2P connection.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_p2p_SwcEngine_nativeSetRemoteSdp(
    mut env: JNIEnv,
    _class: JClass,
    engine_ptr: jlong,
    handle: jlong,
    sdp_json: JString,
) {
    catch_panic((), || {
        let state = unsafe { &*(engine_ptr as *const EngineState) };
        let _guard = state.runtime.enter();

        let sdp_str: String = jstring_to_rust(&mut env, &sdp_json)
            .unwrap_or_else(|_| "{}".to_string());

        let sdp: SessionDescription = match serde_json::from_str(&sdp_str) {
            Ok(s) => s,
            Err(e) => {
                throw_runtime_exception(&mut env, &format!("invalid SDP JSON: {}", e));
                return;
            }
        };

        let mut engine = state.engine.lock();
        if let Err(e) = engine.set_remote_sdp(PeerHandle::from(handle as u64), &sdp) {
            log::error!("[JNI] setRemoteSdp error: {}", e);
            throw_runtime_exception(&mut env, &format!("setRemoteSdp failed: {}", e));
        }
    });
}

/// Add a remote ICE candidate to a P2P connection.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_p2p_SwcEngine_nativeAddIceCandidate(
    mut env: JNIEnv,
    _class: JClass,
    engine_ptr: jlong,
    handle: jlong,
    candidate_json: JString,
) {
    catch_panic((), || {
        let state = unsafe { &*(engine_ptr as *const EngineState) };
        let _guard = state.runtime.enter();

        let candidate_str: String = jstring_to_rust(&mut env, &candidate_json)
            .unwrap_or_else(|_| "{}".to_string());

        let candidate: IceCandidate = match serde_json::from_str(&candidate_str) {
            Ok(c) => c,
            Err(e) => {
                throw_runtime_exception(&mut env, &format!("invalid candidate JSON: {}", e));
                return;
            }
        };

        let mut engine = state.engine.lock();
        if let Err(e) = engine.add_ice_candidate(PeerHandle::from(handle as u64), &candidate) {
            log::error!("[JNI] addIceCandidate error: {}", e);
            throw_runtime_exception(&mut env, &format!("addIceCandidate failed: {}", e));
        }
    });
}

/// Attach local audio track to a P2P connection.
/// Creates NativeAudioSource → RtcAudioTrack → add_track to PeerConnection.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_p2p_SwcEngine_nativeAttachP2pAudio(
    mut env: JNIEnv,
    _class: JClass,
    engine_ptr: jlong,
    handle: jlong,
) {
    catch_panic((), || {
        let state = unsafe { &*(engine_ptr as *const EngineState) };
        let _guard = state.runtime.enter();

        let mut engine = state.engine.lock();
        if let Err(e) = engine.attach_p2p_audio(PeerHandle::from(handle as u64)) {
            log::error!("[JNI] attachP2pAudio error: {}", e);
            throw_runtime_exception(&mut env, &format!("attachP2pAudio failed: {}", e));
        } else {
            log::info!("[JNI] attachP2pAudio: handle={}", handle);
        }
    });
}

/// Set AEC configuration for a P2P connection's audio source.
/// Must be called after nativeAttachP2pAudio.
/// `config_json` — JSON string with AEC parameters (see AecConfig struct).
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_p2p_SwcEngine_nativeSetP2pAecConfig(
    mut env: JNIEnv,
    _class: JClass,
    engine_ptr: jlong,
    handle: jlong,
    config_json: JString,
) {
    let json_str = jstring_to_rust(&mut env, &config_json).unwrap_or_default();
    let config: AecConfig = serde_json::from_str(&json_str).unwrap_or_default();

    catch_panic((), || {
        let state = unsafe { &*(engine_ptr as *const EngineState) };
        let _guard = state.runtime.enter();

        let engine = state.engine.lock();
        if let Err(e) = engine.set_p2p_aec_config(PeerHandle::from(handle as u64), &config) {
            log::error!("[JNI] setP2pAecConfig error: {}", e);
        } else {
            log::info!("[JNI] setP2pAecConfig: handle={}, delay={}ms, pre_gain={:.2}, post_gain={:.2}, ns_level={}",
                handle, config.stream_delay_ms, config.capture_pre_gain, config.capture_post_gain, config.ns_level);
        }
    });
}

/// Push PCM audio frame to P2P audio source.
///
/// `data` - byte[] of i16 PCM samples (little-endian)
/// `sample_rate` - e.g. 48000
/// `channels` - e.g. 1
/// `samples_per_channel` - e.g. 480 (10ms at 48kHz)
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_p2p_SwcEngine_nativePushP2pAudioFrame(
    mut env: JNIEnv,
    _class: JClass,
    engine_ptr: jlong,
    handle: jlong,
    data: JByteArray,
    sample_rate: jint,
    channels: jint,
    samples_per_channel: jint,
) {
    // Extract byte array from JNI before catch_panic to avoid borrow conflict
    let byte_len = env.get_array_length(&data).unwrap_or(0) as usize;
    if byte_len == 0 {
        return;
    }
    let mut bytes = vec![0i8; byte_len];
    if env.get_byte_array_region(&data, 0, &mut bytes).is_err() {
        return;
    }

    catch_panic((), || {
        let state = unsafe { &*(engine_ptr as *const EngineState) };
        let _guard = state.runtime.enter();

        // Convert bytes to i16 samples (little-endian)
        let i16_data: Vec<i16> = bytes
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0] as u8, c[1] as u8]))
            .collect();

        let engine = state.engine.lock();
        if let Err(e) = engine.push_p2p_audio_frame(
            PeerHandle::from(handle as u64),
            &i16_data,
            sample_rate as u32,
            channels as u32,
            samples_per_channel as u32,
        ) {
            log::error!("[JNI] pushP2pAudioFrame error: {}", e);
        }
    });
}

/// Push reference frame for AEC (echo cancellation).
///
/// `data` - i16 PCM samples of the playback (reference) signal.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_p2p_SwcEngine_nativePushP2pReferenceFrame(
    mut env: JNIEnv,
    _class: JClass,
    engine_ptr: jlong,
    handle: jlong,
    data: JByteArray,
) {
    // Extract byte array from JNI before catch_panic to avoid borrow conflict
    let byte_len = env.get_array_length(&data).unwrap_or(0) as usize;
    if byte_len == 0 {
        return;
    }
    let mut bytes = vec![0i8; byte_len];
    if env.get_byte_array_region(&data, 0, &mut bytes).is_err() {
        return;
    }

    catch_panic((), || {
        let state = unsafe { &*(engine_ptr as *const EngineState) };
        let _guard = state.runtime.enter();

        let i16_data: Vec<i16> = bytes
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0] as u8, c[1] as u8]))
            .collect();

        let engine = state.engine.lock();
        engine.push_p2p_reference_frame(PeerHandle::from(handle as u64), &i16_data);
    });
}

/// Create an audio stream from a remote P2P audio track (for playback).
///
/// Call after receiving P2pRemoteTrack (kind=Audio) event from pollEvents().
/// Returns an opaque stream handle for use with audio_stream methods.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_p2p_SwcEngine_nativeCreateP2pAudioStream(
    mut env: JNIEnv,
    _class: JClass,
    engine_ptr: jlong,
    handle: jlong,
    track_id: JString,
) -> jlong {
    catch_panic(
        0i64,
        || {
            let state = unsafe { &*(engine_ptr as *const EngineState) };
            let _guard = state.runtime.enter();

            let track_id_str: String = jstring_to_rust(&mut env, &track_id)
                .unwrap_or_default();

            let engine = state.engine.lock();
            let rtc_track: RtcAudioTrack = match engine
                .take_p2p_remote_audio_track(PeerHandle::from(handle as u64), &track_id_str)
            {
                Some(t) => t,
                None => {
                    throw_runtime_exception(
                        &mut env,
                        &format!("remote audio track '{}' not found", track_id_str),
                    );
                    return 0;
                }
            };

            let native = NativeAudioStream::new(rtc_track, 48_000, 1);
            let stream_handle = AudioStreamHandle::new(native);
            let ptr = Box::into_raw(Box::new(stream_handle)) as jlong;
            log::info!(
                "[JNI] createP2pAudioStream: handle={}, track_id={}, stream={}",
                handle,
                track_id_str,
                ptr
            );
            ptr
        },
    )
}

/// Poll all pending P2P events.
/// Returns JSON array string of EngineEvent items, or "[]" if empty.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_p2p_SwcEngine_nativePollEvents(
    mut env: JNIEnv,
    _class: JClass,
    engine_ptr: jlong,
) -> jstring {
    let result = catch_panic(
        "[]".to_string(),
        || {
            let state = unsafe { &*(engine_ptr as *const EngineState) };
            let _guard = state.runtime.enter();
            let mut engine = state.engine.lock();
            let events: Vec<EngineEvent> = engine.poll_events();
            serde_json::to_string(&events).unwrap_or_else(|_| "[]".into())
        },
    );

    rust_to_jstring(&mut env, &result)
        .unwrap_or_else(|_| JObject::null().into())
        .into_raw()
}

/// Close a P2P connection.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_p2p_SwcEngine_nativeCloseP2p(
    mut env: JNIEnv,
    _class: JClass,
    engine_ptr: jlong,
    handle: jlong,
) {
    catch_panic((), || {
        let state = unsafe { &*(engine_ptr as *const EngineState) };
        let _guard = state.runtime.enter();
        let mut engine = state.engine.lock();
        engine.close_p2p(PeerHandle::from(handle as u64));
        log::info!("[JNI] closeP2p: handle={}", handle);
    });
}

/// Shutdown the engine — close all connections.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_p2p_SwcEngine_nativeShutdown(
    mut env: JNIEnv,
    _class: JClass,
    engine_ptr: jlong,
) {
    catch_panic((), || {
        let state = unsafe { &*(engine_ptr as *const EngineState) };
        let _guard = state.runtime.enter();
        let mut engine = state.engine.lock();
        engine.shutdown();
        log::info!("[JNI] shutdown: engine={}", engine_ptr);
    });
}
