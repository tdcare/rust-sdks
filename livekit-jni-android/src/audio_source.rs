//! JNI bridge for NativeAudioSource — SFU audio capture/publish.
//!
//! Mirrors OHOS `livekit-napi-ohos/src/audio_source.rs`.
//! Provides creation, frame pushing, AEC, and destruction of a
//! `NativeAudioSource` that feeds into LiveKit room audio tracks.

use std::borrow::Cow;

use jni::objects::{JByteArray, JClass};
use jni::sys::{jint, jlong};
use jni::JNIEnv;

use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::AudioSourceOptions;

use crate::{catch_panic, get_runtime, throw_runtime_exception};

/// Create a new NativeAudioSource, returning a heap pointer.
///
/// # Parameters
/// - `sample_rate` – e.g. 48000
/// - `num_channels` – e.g. 1
/// - `queue_size_ms` – internal buffer size (multiple of 10), e.g. 20
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkAudioSource_nativeNew(
    mut env: JNIEnv,
    _class: JClass,
    sample_rate: jint,
    num_channels: jint,
    queue_size_ms: jint,
) -> jlong {
    catch_panic(0i64, || {
        let qms = if queue_size_ms <= 0 { 20 } else { queue_size_ms } as u32;
        if qms % 10 != 0 {
            throw_runtime_exception(&mut env, "queue_size_ms must be a multiple of 10");
            return 0i64;
        }

        let options = AudioSourceOptions {
            echo_cancellation: false,
            noise_suppression: false,
            auto_gain_control: true,
        };

        let source = NativeAudioSource::new(
            options,
            sample_rate as u32,
            num_channels as u32,
            qms,
        );

        let boxed = Box::new(source);
        let ptr = Box::into_raw(boxed) as jlong;
        log::info!(
            "[JNI] LkAudioSource created: rate={}, ch={}, queue={}ms, ptr={:#x}",
            sample_rate, num_channels, qms, ptr
        );
        ptr
    })
}

/// Push a PCM audio frame (i16 LE interleaved) into the source.
///
/// # Parameters
/// - `ptr` – pointer from nativeNew
/// - `data` – byte[] of i16 PCM samples
/// - `sample_rate` – must match constructor
/// - `channels` – must match constructor
/// - `samples_per_channel` – e.g. 480 (10ms at 48kHz)
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkAudioSource_nativePushFrame(
    mut env: JNIEnv,
    _class: JClass,
    ptr: jlong,
    data: JByteArray,
    sample_rate: jint,
    channels: jint,
    samples_per_channel: jint,
) {
    let byte_len = env.get_array_length(&data).unwrap_or(0) as usize;
    if byte_len == 0 || ptr == 0 {
        return;
    }
    let mut bytes = vec![0i8; byte_len];
    if env.get_byte_array_region(&data, 0, &mut bytes).is_err() {
        return;
    }

    catch_panic((), || {
        let source = unsafe { &*(ptr as *const NativeAudioSource) };

        // Reinterpret i8 bytes as i16 samples (little-endian ARM/x86)
        let samples_count = byte_len / 2;
        let i16_data: Vec<i16> = bytes
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0] as u8, c[1] as u8]))
            .collect();

        let frame = AudioFrame {
            data: Cow::Owned(i16_data),
            sample_rate: sample_rate as u32,
            num_channels: channels as u32,
            samples_per_channel: samples_per_channel as u32,
        };

        let runtime = get_runtime();
        runtime.block_on(async {
            if let Err(e) = source.capture_frame(&frame).await {
                log::error!("[JNI] capture_frame error: {:?}", e);
            }
        });
    });
}

/// Clear any buffered samples that have not yet been encoded.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkAudioSource_nativeClearBuffer(
    _env: JNIEnv,
    _class: JClass,
    ptr: jlong,
) {
    if ptr == 0 {
        return;
    }
    catch_panic((), || {
        let source = unsafe { &*(ptr as *const NativeAudioSource) };
        source.clear_buffer();
    });
}

/// Initialize software AEC (sonora WebRTC AEC3 + NS + AGC).
/// Must be called before pushFrame(). Idempotent.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkAudioSource_nativeInitAec(
    _env: JNIEnv,
    _class: JClass,
    ptr: jlong,
) {
    if ptr == 0 {
        return;
    }
    catch_panic((), || {
        let source = unsafe { &*(ptr as *const NativeAudioSource) };
        source.init_aec();
        log::info!("[JNI] LkAudioSource AEC initialized");
    });
}

/// Push far-end reference audio frame for AEC echo cancellation.
///
/// # Parameters
/// - `ptr` – pointer from nativeNew
/// - `data` – byte[] of i16 PCM samples (mono, 48kHz)
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkAudioSource_nativePushReferenceFrame(
    mut env: JNIEnv,
    _class: JClass,
    ptr: jlong,
    data: JByteArray,
) {
    let byte_len = env.get_array_length(&data).unwrap_or(0) as usize;
    if byte_len == 0 || ptr == 0 || byte_len % 2 != 0 {
        return;
    }
    let mut bytes = vec![0i8; byte_len];
    if env.get_byte_array_region(&data, 0, &mut bytes).is_err() {
        return;
    }

    catch_panic((), || {
        let source = unsafe { &*(ptr as *const NativeAudioSource) };
        let i16_data: Vec<i16> = bytes
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0] as u8, c[1] as u8]))
            .collect();
        source.push_reference_frame(&i16_data);
    });
}

/// Destroy the NativeAudioSource and free memory.
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn Java_cn_tdcare_smartward_rust_sfu_LkAudioSource_nativeDestroy(
    _env: JNIEnv,
    _class: JClass,
    ptr: jlong,
) {
    if ptr == 0 {
        return;
    }
    catch_panic((), || {
        unsafe {
            let _ = Box::from_raw(ptr as *mut NativeAudioSource);
        }
        log::info!("[JNI] LkAudioSource destroyed: ptr={:#x}", ptr);
    });
}
