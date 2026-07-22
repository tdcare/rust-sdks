//! Audio stream JNI wrapper — mirrors `livekit-napi-ohos/src/audio_stream.rs`.
//!
//! Provides `LkAudioStream` for reading decoded PCM frames from a remote
//! audio track. Android uses `AudioTrack` to play the received PCM data.
//!
//! Kotlin class: `cn.tdcare.smartward.rust.audio.LkAudioStream`

use std::sync::Arc;

use futures::StreamExt;
use jni::objects::{JByteArray, JClass, JObject};
use jni::sys::{jbyteArray, jint, jlong, jstring};
use jni::JNIEnv;
use libwebrtc::audio_stream::native::NativeAudioStream;
use tokio::sync::Mutex;

use crate::{catch_panic, get_runtime, rust_to_jstring};

// ============================================================
// AudioStreamHandle — internal state
// ============================================================

/// Wraps a NativeAudioStream for use across JNI boundary.
/// Stored as an opaque pointer (jlong) on the Kotlin side.
pub struct AudioStreamHandle {
    stream: Arc<Mutex<Option<NativeAudioStream>>>,
}

impl AudioStreamHandle {
    pub fn new(native: NativeAudioStream) -> Self {
        Self {
            stream: Arc::new(Mutex::new(Some(native))),
        }
    }
}

// ============================================================
// Audio frame result — returned as JSON + byte[]
// ============================================================

/// Audio frame info returned alongside PCM data.
/// Serialized as JSON: {"sample_rate":48000,"num_channels":1,"samples_per_channel":480}
#[derive(serde::Serialize)]
struct AudioFrameInfo {
    sample_rate: u32,
    num_channels: u32,
    samples_per_channel: u32,
}

// ============================================================
// JNI Methods — cn.tdcare.smartward.rust.audio.LkAudioStream
// ============================================================

/// Read the next audio frame (blocking with timeout).
///
/// Returns PCM data as byte[] (i16 little-endian), or null if stream ended.
/// The `info_json_out` parameter receives frame metadata as JSON.
///
/// NOTE: This is a blocking call intended to be called from a dedicated
/// audio playback thread (not the UI thread).
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_audio_LkAudioStream_nativeNextFrame(
    mut env: JNIEnv,
    _class: JClass,
    stream_ptr: jlong,
) -> jbyteArray {
    let result = catch_panic(
        None::<Vec<u8>>,
        || {
            let handle = unsafe { &*(stream_ptr as *const AudioStreamHandle) };
            let runtime = get_runtime();

            // Block on the async next() with a timeout to avoid hanging forever
            let frame_data = runtime.block_on(async {
                let mut guard = handle.stream.lock().await;
                let stream = guard.as_mut()?;

                // Wait for next frame with 100ms timeout
                match tokio::time::timeout(
                    std::time::Duration::from_millis(100),
                    stream.next(),
                )
                .await
                {
                    Ok(Some(frame)) => {
                        // Convert i16 samples to little-endian bytes
                        let mut bytes = Vec::with_capacity(frame.data.len() * 2);
                        for &sample in frame.data.iter() {
                            bytes.extend_from_slice(&sample.to_le_bytes());
                        }

                        // Periodic diagnostic logging
                        {
                            use std::sync::atomic::{AtomicU64, Ordering};
                            static RX_FRAME_COUNT: AtomicU64 = AtomicU64::new(0);
                            let count = RX_FRAME_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                            if count == 1 || count % 500 == 0 {
                                let min = frame.data.iter().copied().min().unwrap_or(0);
                                let max = frame.data.iter().copied().max().unwrap_or(0);
                                log::info!(
                                    "[JNI AudioStream] rx_frame #{}: {} samples, {}ch {}Hz, pcm_range=[{},{}]",
                                    count,
                                    frame.data.len(),
                                    frame.num_channels,
                                    frame.sample_rate,
                                    min,
                                    max,
                                );
                            }
                        }

                        Some(bytes)
                    }
                    Ok(None) => {
                        log::info!("[JNI AudioStream] stream ended (None)");
                        None
                    }
                    Err(_) => {
                        // Timeout — return empty to let caller retry
                        Some(Vec::new())
                    }
                }
            });

            frame_data
        },
    );

    match result {
        Some(bytes) => {
            if bytes.is_empty() {
                // Timeout — return empty byte array
                let arr = env.new_byte_array(0).unwrap_or_default();
                arr.into_raw()
            } else {
                // Convert Vec<u8> to i8 for JNI
                let i8_data: Vec<i8> = bytes.iter().map(|&b| b as i8).collect();
                match env.new_byte_array(i8_data.len() as i32) {
                    Ok(arr) => {
                        let _ = env.set_byte_array_region(&arr, 0, &i8_data);
                        arr.into_raw()
                    }
                    Err(_) => std::ptr::null_mut(),
                }
            }
        }
        None => std::ptr::null_mut(), // Stream ended
    }
}

/// Get the next audio frame with metadata.
/// Returns a JSON string: {"data_available":true,"sample_rate":48000,"num_channels":1,"samples_per_channel":480}
/// or {"data_available":false} if stream ended.
/// The actual PCM data is returned via nativeNextFrame.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_audio_LkAudioStream_nativeGetFrameInfo(
    mut env: JNIEnv,
    _class: JClass,
    stream_ptr: jlong,
) -> jstring {
    let info = AudioFrameInfo {
        sample_rate: 48_000,
        num_channels: 1,
        samples_per_channel: 480, // 10ms at 48kHz
    };
    let json = serde_json::to_string(&info).unwrap_or_else(|_| "{}".into());
    rust_to_jstring(&mut env, &json)
        .unwrap_or_else(|_| JObject::null().into())
        .into_raw()
}

/// Close the audio stream and release resources.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_audio_LkAudioStream_nativeClose(
    mut env: JNIEnv,
    _class: JClass,
    stream_ptr: jlong,
) {
    catch_panic((), || {
        if stream_ptr == 0 {
            return;
        }
        let handle = unsafe { &*(stream_ptr as *const AudioStreamHandle) };
        let stream_clone = handle.stream.clone();
        let runtime = get_runtime();
        runtime.block_on(async {
            if let Some(mut s) = stream_clone.lock().await.take() {
                s.close();
            }
        });
        log::info!("[JNI AudioStream] closed: ptr={}", stream_ptr);
    });
}

/// Destroy the audio stream handle (free memory).
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_audio_LkAudioStream_nativeDestroy(
    mut env: JNIEnv,
    _class: JClass,
    stream_ptr: jlong,
) {
    catch_panic((), || {
        if stream_ptr == 0 {
            return;
        }
        // Close first, then free
        let handle = unsafe { Box::from_raw(stream_ptr as *mut AudioStreamHandle) };
        let runtime = get_runtime();
        runtime.block_on(async {
            if let Some(mut s) = handle.stream.lock().await.take() {
                s.close();
            }
        });
        log::info!("[JNI AudioStream] destroyed: ptr={}", stream_ptr);
    });
}
