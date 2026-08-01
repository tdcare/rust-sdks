//! LiveKit JNI bindings for Android.
//!
//! This crate exposes the SmartWard P2P (SwcEngine) and SFU (LkRoom) engines
//! to Android Kotlin/Java via JNI. It mirrors the API surface of
//! `livekit-napi-ohos` for cross-platform consistency.
//!
//! Build with:
//! ```sh
//! cargo ndk -t arm64-v8a build --release -p livekit-jni-android
//! ```

use std::os::raw::c_void;
use std::sync::Once;

use jni::sys::{jint, JNI_VERSION_1_6};
use jni::JavaVM;

pub mod audio_source;
pub mod audio_stream;
pub mod native_window;
pub mod p2p;
pub mod room;
pub mod video_source;
pub mod video_stream;

// ============================================================
// Logger initialization
// ============================================================

static LOGGER_INIT: Once = Once::new();

/// Initialize Android logcat logger. Called once from JNI_OnLoad.
pub(crate) fn init_logger() {
    LOGGER_INIT.call_once(|| {
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Debug)
                .with_tag("LiveKitRust"),
        );
        log::info!("livekit-jni-android: logger initialized");
    });
}

// ============================================================
// Global Tokio runtime
// ============================================================

use std::sync::Arc;
use tokio::runtime::Runtime;

static mut GLOBAL_RUNTIME: Option<Arc<Runtime>> = None;
static RUNTIME_INIT: Once = Once::new();

/// Get or create the global Tokio runtime for async operations.
pub(crate) fn get_runtime() -> Arc<Runtime> {
    unsafe {
        RUNTIME_INIT.call_once(|| {
            let rt = Runtime::new().expect("Failed to create Tokio runtime");
            GLOBAL_RUNTIME = Some(Arc::new(rt));
            log::info!("livekit-jni-android: global Tokio runtime created");
        });
        GLOBAL_RUNTIME
            .as_ref()
            .expect("runtime not initialized")
            .clone()
    }
}

// ============================================================
// JNI_OnLoad — entry point when System.loadLibrary() is called
// ============================================================

/// JNI entry point. Called automatically when `System.loadLibrary("livekit_jni_android")`
/// is invoked from Kotlin/Java.
///
/// Initializes:
/// 1. Android logcat logger
/// 2. Global Tokio runtime (lazy, on first use)
#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn JNI_OnLoad(vm: JavaVM, _: *mut c_void) -> jint {
    init_logger();
    log::info!("livekit-jni-android: JNI_OnLoad called");

    // Store JVM reference for potential callback usage
    store_jvm(vm);

    JNI_VERSION_1_6
}

// ============================================================
// JVM storage for callbacks (optional, for future use)
// ============================================================

use std::sync::Mutex;

static JVM_REF: Mutex<Option<JavaVM>> = Mutex::new(None);

fn store_jvm(vm: JavaVM) {
    if let Ok(mut guard) = JVM_REF.lock() {
        *guard = Some(vm);
    }
}

/// Get the stored JVM reference (for creating JNI env on native threads).
#[allow(dead_code)]
pub(crate) fn get_jvm() -> Option<JavaVM> {
    JVM_REF.lock().ok().and_then(|g| {
        g.as_ref().and_then(|vm| unsafe { JavaVM::from_raw(vm.get_java_vm_pointer()) }.ok())
    })
}

// ============================================================
// JNI helper utilities
// ============================================================

/// Convert a JNI jstring to a Rust String.
pub(crate) fn jstring_to_rust(
    env: &mut jni::JNIEnv,
    s: &jni::objects::JString,
) -> Result<String, jni::errors::Error> {
    let java_str = env.get_string(s)?;
    Ok(java_str.into())
}

/// Create a JNI jstring from a Rust &str.
pub(crate) fn rust_to_jstring<'a>(
    env: &mut jni::JNIEnv<'a>,
    s: &str,
) -> Result<jni::objects::JString<'a>, jni::errors::Error> {
    env.new_string(s)
}

/// Throw a Java RuntimeException with the given message.
pub(crate) fn throw_runtime_exception(
    env: &mut jni::JNIEnv,
    msg: &str,
) {
    let _ = env.throw_new("java/lang/RuntimeException", msg);
}

/// Panic-safe wrapper for JNI methods. Catches Rust panics and logs them.
/// Returns `default` if a panic occurred.
pub(crate) fn catch_panic<F, T>(default: T, f: F) -> T
where
    F: FnOnce() -> T,
    T: Default,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(val) => val,
        Err(e) => {
            let msg = if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else {
                "Unknown Rust panic".to_string()
            };
            log::error!("JNI panic caught: {}", msg);
            default
        }
    }
}
