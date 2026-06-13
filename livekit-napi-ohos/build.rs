extern crate napi_build_ohos;

fn main() {
    napi_build_ohos::setup();

    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("ohos") {
        // Compile the small hilog shim as part of the cdylib so we can
        // forward log records to OH_LOG_Print without a Rust C-variadic call.
        cc::Build::new()
            .file("src/hilog_shim.c")
            .flag_if_supported("-Wno-unused-parameter")
            .compile("livekit_hilog_shim");
        println!("cargo:rerun-if-changed=src/hilog_shim.c");

        // Link OHOS hilog NDK so OH_LOG_Print resolves at link time.
        println!("cargo:rustc-link-lib=hilog_ndk.z");
    }
}
