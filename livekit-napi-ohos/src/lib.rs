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

//! LiveKit NAPI bindings for OpenHarmony (OHOS) ArkTS.
//!
//! This crate exposes the LiveKit Rust SDK to ArkTS applications via
//! Node-API (N-API), using the `napi-ohos` framework. Build with
//! `ohrs build` or `cargo build --target aarch64-unknown-linux-ohos`.

#[allow(unused_imports)]
#[macro_use]
extern crate napi_derive_ohos;

use std::ffi::CString;
use std::sync::Once;

static LOGGER_INIT: Once = Once::new();

// Shim defined in `src/hilog_shim.c` (built via `cc` in `build.rs`). It
// invokes OH_LOG_Print(LOG_APP, level, 0, tag, "%{public}s", msg) so we
// avoid relying on Rust's C-variadic FFI ABI for this hot path.
#[cfg(target_env = "ohos")]
extern "C" {
    fn livekit_hilog_print(
        level: i32,
        tag: *const std::ffi::c_char,
        msg: *const std::ffi::c_char,
    ) -> i32;
}

#[cfg(not(target_env = "ohos"))]
unsafe fn livekit_hilog_print(
    _level: i32,
    _tag: *const std::ffi::c_char,
    _msg: *const std::ffi::c_char,
) -> i32 {
    0
}

const LOG_DEBUG: i32 = 3;
const LOG_INFO: i32 = 4;
const LOG_WARN: i32 = 5;
const LOG_ERROR: i32 = 6;

/// Custom `log::Log` implementation that forwards records to OHOS hilog.
struct OhosHilogLogger;

impl log::Log for OhosHilogLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Debug
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let level = match record.level() {
            log::Level::Error => LOG_ERROR,
            log::Level::Warn => LOG_WARN,
            log::Level::Info => LOG_INFO,
            log::Level::Debug | log::Level::Trace => LOG_DEBUG,
        };
        let tag = CString::new("LiveKitRust")
            .unwrap_or_else(|_| CString::new("LiveKit").expect("valid cstring"));
        let msg = CString::new(format!("[{}] {}", record.target(), record.args()))
            .unwrap_or_else(|_| CString::new("(log format error)").expect("valid cstring"));
        // SAFETY: The CStrings outlive the call and the C shim only reads
        // them as NUL-terminated UTF-8.
        unsafe {
            livekit_hilog_print(level, tag.as_ptr(), msg.as_ptr());
        }
    }

    fn flush(&self) {}
}

static LOGGER: OhosHilogLogger = OhosHilogLogger;

/// Initialize the Rust logger so output is forwarded to OHOS hilog.
pub(crate) fn init_logger() {
    LOGGER_INIT.call_once(|| {
        // Direct sanity-check call so we can confirm the FFI path is wired
        // up before going through the `log` crate machinery.
        let probe_tag = CString::new("LiveKitRust").expect("valid cstring");
        let probe_msg = CString::new("init_logger: hilog shim probe")
            .expect("valid cstring");
        // SAFETY: CStrings outlive the call.
        unsafe {
            livekit_hilog_print(LOG_INFO, probe_tag.as_ptr(), probe_msg.as_ptr());
        }
        let set_ok = log::set_logger(&LOGGER).is_ok();
        log::set_max_level(log::LevelFilter::Debug);
        let status_msg = CString::new(format!(
            "init_logger: log::set_logger ok={}, max_level=Debug",
            set_ok
        ))
        .unwrap_or_else(|_| CString::new("init_logger: status (cstring err)").expect("valid"));
        // SAFETY: see above.
        unsafe {
            livekit_hilog_print(LOG_INFO, probe_tag.as_ptr(), status_msg.as_ptr());
        }
        log::info!("livekit-napi-ohos: hilog logger initialized");
    });
}

pub mod room;
pub mod participant;
pub mod track;
pub mod audio_source;
pub mod video_source;
pub mod audio_stream;
pub mod video_stream;
pub mod events;
pub mod e2ee;
pub mod smartward;
pub mod data_track;
pub mod stats;
pub(crate) mod native_surface;
