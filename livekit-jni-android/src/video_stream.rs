//! Video stream JNI wrapper — mirrors `livekit-napi-ohos/src/video_stream.rs`.
//!
//! Provides `LkVideoStream` for receiving decoded video frames from a remote
//! participant's video track and rendering them to an Android Surface.
//!
//! Kotlin class: `cn.tdcare.smartward.rust.sfu.LkVideoStream`

use std::sync::Arc;

use futures::{FutureExt, StreamExt};
use jni::objects::{JByteArray, JClass, JObject, JString};
use jni::sys::{jbyteArray, jint, jlong};
use jni::JNIEnv;
use livekit::webrtc::video_stream::native::NativeVideoStream;
use tokio::sync::Mutex;

use crate::native_window::AndroidSurfaceRenderer;
use crate::{catch_panic, get_runtime, jstring_to_rust, rust_to_jstring, throw_runtime_exception};

// ============================================================
// VideoStreamState — internal state stored as opaque pointer
// ============================================================

pub struct VideoStreamState {
    stream: Arc<Mutex<Option<NativeVideoStream>>>,
    renderer: Arc<std::sync::Mutex<AndroidSurfaceRenderer>>,
}

impl VideoStreamState {
    fn new(stream: NativeVideoStream) -> Self {
        Self {
            stream: Arc::new(Mutex::new(Some(stream))),
            renderer: Arc::new(std::sync::Mutex::new(AndroidSurfaceRenderer::new())),
        }
    }
}

// ============================================================
// JNI Methods — cn.tdcare.smartward.rust.sfu.LkVideoStream
// ============================================================

/// Create a video stream from a remote video track.
///
/// `room_ptr` - RoomState pointer from LkRoom
/// `participant_identity` - remote participant identity string
/// `track_sid` - video track SID string
///
/// Returns a VideoStreamState pointer, or 0 on failure.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkVideoStream_nativeFromTrack(
    mut env: JNIEnv,
    _class: JClass,
    room_ptr: jlong,
    participant_identity: JString,
    track_sid: JString,
) -> jlong {
    // Extract strings before catch_panic to avoid borrow conflict
    let identity_str: String = jstring_to_rust(&mut env, &participant_identity)
        .unwrap_or_default();
    let track_sid_str: String = jstring_to_rust(&mut env, &track_sid)
        .unwrap_or_default();

    catch_panic(0i64, || {
        use livekit::prelude::*;

        let room_state = unsafe { &*(room_ptr as *const crate::room::RoomState) };

        let guard = room_state.room.lock();
        let room = match guard.as_ref() {
            Some(r) => r,
            None => {
                log::error!("[JNI VideoStream] room not connected");
                return 0;
            }
        };

        let identity: ParticipantIdentity = identity_str.clone().into();
        let participant = match room.remote_participants().get(&identity) {
            Some(p) => p.clone(),
            None => {
                log::error!("[JNI VideoStream] participant '{}' not found", identity_str);
                return 0;
            }
        };

        let tsid: TrackSid = match track_sid_str.clone().try_into() {
            Ok(s) => s,
            Err(_) => {
                log::error!("[JNI VideoStream] invalid track_sid '{}'", track_sid_str);
                return 0;
            }
        };

        let publication = match participant.get_track_publication(&tsid) {
            Some(p) => p,
            None => {
                log::error!("[JNI VideoStream] track '{}' not found", track_sid_str);
                return 0;
            }
        };

        let remote_track = match publication.track() {
            Some(t) => t,
            None => {
                log::error!("[JNI VideoStream] track not subscribed yet");
                return 0;
            }
        };

        match remote_track {
            RemoteTrack::Video(video_track) => {
                let rtc_track = video_track.rtc_track();
                let native_stream = NativeVideoStream::new(rtc_track);
                let state = VideoStreamState::new(native_stream);
                let ptr = Box::into_raw(Box::new(state)) as jlong;
                log::info!(
                    "[JNI VideoStream] fromTrack: participant={}, track={}, ptr={}",
                    identity_str, track_sid_str, ptr
                );
                ptr
            }
            _ => {
                log::error!("[JNI VideoStream] track is not video");
                0
            }
        }
    })
}

/// Bind an Android Surface for direct rendering.
///
/// `surface` - android.view.Surface jobject
/// Returns true on success.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkVideoStream_nativeSetSurface(
    mut env: JNIEnv,
    _class: JClass,
    stream_ptr: jlong,
    surface: JObject,
    width: jint,
    height: jint,
) -> jint {
    // Extract raw pointers before catch_panic to avoid borrow conflict
    let env_ptr = env.get_native_interface() as *mut std::ffi::c_void;
    let surface_ptr = surface.as_raw() as *mut std::ffi::c_void;

    catch_panic(0, || {
        if stream_ptr == 0 {
            return 0;
        }
        let state = unsafe { &*(stream_ptr as *const VideoStreamState) };

        let mut renderer = match state.renderer.lock() {
            Ok(r) => r,
            Err(_) => {
                log::error!("[JNI VideoStream] renderer lock poisoned");
                return 0;
            }
        };

        let ok = unsafe {
            renderer.set_surface(env_ptr, surface_ptr, width as u32, height as u32)
        };

        if ok { 1 } else { 0 }
    })
}

/// Await the next frame and render it to the bound Surface.
///
/// This is a BLOCKING call — must be called from a dedicated render thread.
///
/// Returns:
/// - 1  if a frame was successfully rendered
/// - 0  if the stream ended (no more frames)
/// - -1 if a frame was received but could not be rendered
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkVideoStream_nativeRenderToSurface(
    mut env: JNIEnv,
    _class: JClass,
    stream_ptr: jlong,
) -> jint {
    catch_panic(-1, || {
        if stream_ptr == 0 {
            return 0;
        }
        let state = unsafe { &*(stream_ptr as *const VideoStreamState) };
        let runtime = get_runtime();

        // Block on async: wait for next frame
        let result = runtime.block_on(async {
            let mut guard = state.stream.lock().await;
            let stream = match guard.as_mut() {
                Some(s) => s,
                None => return 0i32, // stream ended
            };

            // Wait for at least one frame
            let Some(mut frame) = stream.next().await else {
                log::info!("[JNI VideoStream] renderToSurface: stream ended");
                return 0i32;
            };

            // Frame-skipping: drain queued frames, render only the latest
            let mut drained: u32 = 0;
            loop {
                match stream.next().now_or_never() {
                    Some(Some(newer)) => {
                        frame = newer;
                        drained += 1;
                    }
                    _ => break,
                }
            }

            if drained > 0 {
                log::debug!("[JNI VideoStream] skipped {} stale frames", drained);
            }

            // Extract I420 data
            let width = frame.buffer.width();
            let height = frame.buffer.height();

            let Some(i420) = frame.buffer.as_i420() else {
                log::warn!("[JNI VideoStream] non-I420 frame {}x{}, skipping", width, height);
                return -1i32;
            };

            let (y, u, v) = i420.data();
            let mut i420_buf = Vec::with_capacity(y.len() + u.len() + v.len());
            i420_buf.extend_from_slice(y);
            i420_buf.extend_from_slice(u);
            i420_buf.extend_from_slice(v);

            // Render to surface
            let rendered = match state.renderer.lock() {
                Ok(mut renderer) => renderer.render_i420(&i420_buf, width, height),
                Err(_) => false,
            };

            if rendered { 1i32 } else { -1i32 }
        });

        result
    })
}

/// Get the next video frame as I420 byte array (software rendering fallback).
///
/// Returns a JSON string: {"data_base64": "...", "width": N, "height": N, "rotation": N}
/// or null if stream ended.
///
/// For performance, prefer nativeRenderToSurface() over this method.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkVideoStream_nativeNextFrame(
    mut env: JNIEnv,
    _class: JClass,
    stream_ptr: jlong,
) -> jbyteArray {
    let result: Option<Vec<u8>> = catch_panic(None, || {
        if stream_ptr == 0 {
            return None;
        }
        let state = unsafe { &*(stream_ptr as *const VideoStreamState) };
        let runtime = get_runtime();

        runtime.block_on(async {
            let mut guard = state.stream.lock().await;
            let stream = match guard.as_mut() {
                Some(s) => s,
                None => return None,
            };

            let frame = stream.next().await?;
            let i420 = frame.buffer.as_i420()?;
            let (y, u, v) = i420.data();

            let mut data = Vec::with_capacity(y.len() + u.len() + v.len());
            data.extend_from_slice(y);
            data.extend_from_slice(u);
            data.extend_from_slice(v);
            Some(data)
        })
    });

    match result {
        Some(data) => {
            let jbytes = env.byte_array_from_slice(&data).unwrap_or_else(|_| JObject::null().into());
            jbytes.into_raw()
        }
        None => std::ptr::null_mut(),
    }
}

/// Get frame dimensions for the last received frame.
/// Returns JSON: {"width": N, "height": N}
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkVideoStream_nativeGetFrameInfo(
    mut env: JNIEnv,
    _class: JClass,
    stream_ptr: jlong,
) -> jni::sys::jstring {
    let info = catch_panic("{}".to_string(), || {
        if stream_ptr == 0 {
            return "{}".to_string();
        }
        // Return default info — actual dimensions come with each frame
        r#"{"width":640,"height":480}"#.to_string()
    });

    rust_to_jstring(&mut env, &info)
        .unwrap_or_else(|_| JObject::null().into())
        .into_raw()
}

/// Close the video stream.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkVideoStream_nativeClose(
    mut env: JNIEnv,
    _class: JClass,
    stream_ptr: jlong,
) {
    catch_panic((), || {
        if stream_ptr == 0 {
            return;
        }
        let state = unsafe { &*(stream_ptr as *const VideoStreamState) };
        let runtime = get_runtime();

        runtime.block_on(async {
            if let Some(mut s) = state.stream.lock().await.take() {
                s.close();
            }
        });

        // Release surface
        if let Ok(mut renderer) = state.renderer.lock() {
            renderer.release_window();
        }

        log::info!("[JNI VideoStream] closed: ptr={}", stream_ptr);
    });
}

/// Destroy the video stream and free all resources.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkVideoStream_nativeDestroy(
    mut env: JNIEnv,
    _class: JClass,
    stream_ptr: jlong,
) {
    catch_panic((), || {
        if stream_ptr == 0 {
            return;
        }
        let state = unsafe { Box::from_raw(stream_ptr as *mut VideoStreamState) };
        let runtime = get_runtime();

        runtime.block_on(async {
            if let Some(mut s) = state.stream.lock().await.take() {
                s.close();
            }
        });

        log::info!("[JNI VideoStream] destroyed: ptr={}", stream_ptr);
    });
}
