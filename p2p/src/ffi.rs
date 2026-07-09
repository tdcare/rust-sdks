//! C-compatible FFI layer for p2p.
//!
//! All functions are `extern "C"` and safe to call from any language
//! with C FFI support (JNI for Android, NAPI for OHOS, etc.).
//!
//! # Memory ownership rules
//!
//! - Engine pointer: caller owns, must free with [`swc_engine_drop`].
//! - String outputs: caller must free with [`swc_free_string`].
//! - JSON is used for all complex types across FFI boundary.
//!
//! # Return code convention
//!
//! - `0`       → success
//! - negative  → error (caller may check `error_msg` output param)

use std::ffi::{c_char, CStr, CString};
use std::ptr;

use crate::{
    EngineEvent, IceCandidate, P2pConfig, SessionDescription, WebRtcEngine,
};

// ============================================================
// Helpers
// ============================================================

/// Convert a C string pointer to a Rust &str.
/// Returns empty string if pointer is null.
unsafe fn cstr_to_str<'a>(ptr: *const c_char) -> &'a str {
    if ptr.is_null() {
        return "";
    }
    // SAFETY: caller guarantees ptr is a valid NUL-terminated C string
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .unwrap_or("")
}

/// Allocate a CString and return ownership via *mut c_char.
/// Write NULL to out_ptr if the string is None.
fn write_string_out(out_ptr: *mut *mut c_char, s: Option<String>) {
    if out_ptr.is_null() {
        return;
    }
    // SAFETY: caller guarantees out_ptr is a valid pointer
    match s {
        Some(v) => {
            let cs = CString::new(v).unwrap_or_else(|_| CString::new("").unwrap());
            unsafe { *out_ptr = cs.into_raw(); }
        }
        None => unsafe { *out_ptr = ptr::null_mut() },
    }
}

/// Set error output and return error code.
fn set_error(out_ptr: *mut *mut c_char, msg: &str) -> i32 {
    if !out_ptr.is_null() {
        let cs = CString::new(msg).unwrap_or_else(|_| CString::new("unknown error").unwrap());
        unsafe { *out_ptr = cs.into_raw(); }
    }
    -1
}

// ============================================================
// Engine lifecycle
// ============================================================

/// Create a new WebRTC engine instance.
///
/// Returns an opaque pointer. Must be freed with [`swc_engine_drop`].
#[no_mangle]
pub extern "C" fn swc_engine_new() -> *mut WebRtcEngine {
    let engine = Box::new(WebRtcEngine::new());
    Box::into_raw(engine)
}

/// Destroy a WebRTC engine instance.
///
/// Passing NULL is safe (no-op).
#[no_mangle]
pub extern "C" fn swc_engine_drop(engine: *mut WebRtcEngine) {
    if engine.is_null() {
        return;
    }
    // SAFETY: caller guarantees engine was created by swc_engine_new
    let _ = unsafe { Box::from_raw(engine) };
}

// ============================================================
// P2P API
// ============================================================

/// Create a P2P PeerConnection.
///
/// - `config_json`: JSON representation of [`P2pConfig`] (e.g. `{"ice_servers":[...]}`).
///   Pass NULL or `"{}"` to use defaults.
///
/// Returns the connection handle (> 0), or 0 on error (check error_msg).
#[no_mangle]
pub extern "C" fn swc_p2p_create(
    engine: *mut WebRtcEngine,
    config_json: *const c_char,
    error_msg: *mut *mut c_char,
) -> u64 {
    if engine.is_null() {
        set_error(error_msg, "engine is null");
        return 0;
    }
    // SAFETY: caller guarantees engine is a valid pointer from swc_engine_new
    let engine = unsafe { &mut *engine };

    let config_str = unsafe { cstr_to_str(config_json) };
    let config: P2pConfig = if config_str.is_empty() || config_str == "{}" {
        P2pConfig::default()
    } else {
        match serde_json::from_str(config_str) {
            Ok(c) => c,
            Err(e) => {
                set_error(error_msg, &format!("invalid config JSON: {}", e));
                return 0;
            }
        }
    };

    let handle = engine.create_p2p_connection(&config);
    handle.0
}

/// Create an Offer SDP for a P2P connection.
///
/// Returns 0 on success. On error, returns -1 and sets `error_msg`.
/// `sdp_json_out` receives a JSON string of [`SessionDescription`].
/// Caller must free with [`swc_free_string`].
#[no_mangle]
pub extern "C" fn swc_p2p_create_offer(
    engine: *mut WebRtcEngine,
    handle: u64,
    sdp_json_out: *mut *mut c_char,
    error_msg: *mut *mut c_char,
) -> i32 {
    if engine.is_null() {
        return set_error(error_msg, "engine is null");
    }
    let engine = unsafe { &mut *engine };
    let ph = crate::PeerHandle(handle);

    match engine.create_offer(ph) {
        Ok(sdp) => {
            let json = serde_json::to_string(&sdp).unwrap_or_else(|_| "{}".into());
            write_string_out(sdp_json_out, Some(json));
            0
        }
        Err(e) => set_error(error_msg, &e.to_string()),
    }
}

/// Set remote SDP for a P2P connection.
///
/// `sdp_json`: JSON representation of [`SessionDescription`].
/// Returns 0 on success, -1 on error.
#[no_mangle]
pub extern "C" fn swc_p2p_set_remote_sdp(
    engine: *mut WebRtcEngine,
    handle: u64,
    sdp_json: *const c_char,
    error_msg: *mut *mut c_char,
) -> i32 {
    if engine.is_null() {
        return set_error(error_msg, "engine is null");
    }
    let engine = unsafe { &mut *engine };
    let ph = crate::PeerHandle(handle);

    let sdp: SessionDescription = match serde_json::from_str(unsafe { cstr_to_str(sdp_json) }) {
        Ok(s) => s,
        Err(e) => return set_error(error_msg, &format!("invalid SDP JSON: {}", e)),
    };

    match engine.set_remote_sdp(ph, &sdp) {
        Ok(()) => 0,
        Err(e) => set_error(error_msg, &e.to_string()),
    }
}

/// Add a remote ICE candidate to a P2P connection.
///
/// `candidate_json`: JSON representation of [`IceCandidate`].
/// Returns 0 on success, -1 on error.
#[no_mangle]
pub extern "C" fn swc_p2p_add_ice_candidate(
    engine: *mut WebRtcEngine,
    handle: u64,
    candidate_json: *const c_char,
    error_msg: *mut *mut c_char,
) -> i32 {
    if engine.is_null() {
        return set_error(error_msg, "engine is null");
    }
    let engine = unsafe { &mut *engine };
    let ph = crate::PeerHandle(handle);

    let candidate: IceCandidate =
        match serde_json::from_str(unsafe { cstr_to_str(candidate_json) }) {
            Ok(c) => c,
            Err(e) => return set_error(error_msg, &format!("invalid candidate JSON: {}", e)),
        };

    match engine.add_ice_candidate(ph, &candidate) {
        Ok(()) => 0,
        Err(e) => set_error(error_msg, &e.to_string()),
    }
}

/// Close a P2P connection.
#[no_mangle]
pub extern "C" fn swc_p2p_close(engine: *mut WebRtcEngine, handle: u64) {
    if engine.is_null() {
        return;
    }
    let engine = unsafe { &mut *engine };
    engine.close_p2p(crate::PeerHandle(handle));
}

// ============================================================
// Events
// ============================================================

/// Poll all pending events from the engine.
///
/// Returns a JSON array string of [`EngineEvent`] items, or NULL on error.
/// The caller must free the returned string with [`swc_free_string`].
/// An empty array `"[]"` is returned when there are no events.
#[no_mangle]
pub extern "C" fn swc_poll_events(engine: *mut WebRtcEngine) -> *mut c_char {
    if engine.is_null() {
        return ptr::null_mut();
    }
    let engine = unsafe { &mut *engine };
    let events: Vec<EngineEvent> = engine.poll_events();
    let json = serde_json::to_string(&events).unwrap_or_else(|_| "[]".into());
    CString::new(json)
        .unwrap_or_else(|_| CString::new("[]").unwrap())
        .into_raw()
}

// ============================================================
// Utility
// ============================================================

/// Free a string previously returned by any `swc_*` function.
///
/// Passing NULL is safe (no-op).
#[no_mangle]
pub extern "C" fn swc_free_string(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    // SAFETY: caller guarantees s was allocated by a swc_* function
    let _ = unsafe { CString::from_raw(s) };
}
