//! Build script for `livekit-libwebrtc`.
//!
//! 在 OHOS target 下链接原生媒体编解码库（OH_AVCodec API），
//! 这些库由 OHOS NDK 提供，仅在编译目标为 `target_env = "ohos"` 时生效。
//! 对其他目标（Linux / macOS / Windows / iOS / Android）该脚本是 no-op，
//! 由 webrtc-sys 自身的构建逻辑处理 native libwebrtc 的链接。

fn main() {
    // 重新运行脚本的触发条件
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_ENV");

    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

    if target_env == "ohos" {
        // OHOS native media codec libraries（来自 OHOS NDK，运行时由系统提供）
        // 用于 OH_AVCodec / OH_VideoEncoder / OH_VideoDecoder / OH_AVFormat / OH_AVBuffer
        println!("cargo:rustc-link-lib=native_media_codecbase");
        println!("cargo:rustc-link-lib=native_media_core");
        println!("cargo:rustc-link-lib=native_media_venc");
        println!("cargo:rustc-link-lib=native_media_vdec");

        // libvpx 静态库（VP8/VP9 软件编解码回退）
        // 如果 `<libwebrtc>/../libvpx-build/` 目录存在，则将其加入 link search path
        // 并以静态方式链接 `libvpx`。当前未提供时跳过，依赖 webrtc-rs/rtc 自带实现。
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
        if !manifest_dir.is_empty() {
            let libvpx_path = std::path::Path::new(&manifest_dir).join("../libvpx-build");
            if libvpx_path.exists() {
                println!("cargo:rustc-link-search=native={}", libvpx_path.display());
                println!("cargo:rustc-link-lib=static=vpx");
                println!("cargo:rerun-if-changed={}", libvpx_path.display());
            }
        }
    }
}
