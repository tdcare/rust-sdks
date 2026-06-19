#[cfg(target_family = "windows")]
mod windows;
#[cfg(target_family = "windows")]
pub use self::windows::ifaces;

#[cfg(all(target_family = "unix", not(target_env = "ohos")))]
mod unix;
#[cfg(all(target_family = "unix", not(target_env = "ohos")))]
pub use self::unix::ifaces;

// OpenHarmony stub implementation
#[cfg(target_env = "ohos")]
mod ohos;
#[cfg(target_env = "ohos")]
pub use self::ohos::ifaces;
