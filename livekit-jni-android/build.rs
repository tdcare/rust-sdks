//! Build script for livekit-jni-android.
//!
//! Links required Android system libraries that are not automatically
//! pulled in by the webrtc-sys dependency:
//! - `android` — for ANativeWindow_fromSurface, ANativeWindow_lock, etc.
//! - `log` — for __android_log_print (used by android_logger)
//! - `GLESv2` — for OpenGL ES (video rendering fallback)
//! - `mediandk` — for AImageReader (future use)

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if target_os == "android" {
        // ANativeWindow APIs (native_window.rs)
        println!("cargo:rustc-link-lib=dylib=android");
        // Android logging
        println!("cargo:rustc-link-lib=dylib=log");
        // OpenGL ES 2.0 (video rendering)
        println!("cargo:rustc-link-lib=dylib=GLESv2");
        // Media NDK (AImageReader — future proofing)
        println!("cargo:rustc-link-lib=dylib=mediandk");

        // Static libvpx (VP8 codec) — prebuilt with generic-gnu + fPIC。
        // 按目标架构选择预编译目录：
        //   - aarch64 (arm64-v8a): libvpx-build（OHOS/Android aarch64 通用）
        //   - armv7 (armeabi-v7a): libvpx-build-armv7（build-libvpx-android-armv7.sh 构建）
        let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
        let libvpx_dir_name = if target_arch == "arm" {
            "libvpx-build-armv7"
        } else {
            "libvpx-build"
        };
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let libvpx_dir = std::path::Path::new(&manifest_dir)
            .join("..")
            .join(libvpx_dir_name);
        if libvpx_dir.exists() {
            println!("cargo:rustc-link-search=native={}", libvpx_dir.display());
            println!("cargo:rustc-link-lib=static=vpx");
        }
    }

    // Re-run if this script changes
    println!("cargo:rerun-if-changed=build.rs");
}
