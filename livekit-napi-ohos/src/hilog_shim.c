// Tiny C shim around OH_LOG_Print so the Rust side doesn't need to perform
// a C variadic call (which has fragile ABI guarantees across Rust versions
// on aarch64). The shim exposes a fixed-arity entry point for emitting one
// %{public}s-formatted message at a given log level.

#include <hilog/log.h>
#include <stdio.h>
#include <unistd.h>

int livekit_hilog_print(int level, const char *tag, const char *msg) {
    // Mirror to stderr so we can confirm the call path even if hilog routing
    // for our domain/tag combination is somehow filtered out.
    fprintf(stderr, "[%s] %s\n", tag, msg);
    fflush(stderr);
    // Use a non-zero domain (0x3200 is a commonly used app range) and
    // LOG_APP so the record shows up under the standard app log type.
    return OH_LOG_Print(LOG_APP, (LogLevel)level, 0x3200, tag, "%{public}s", msg);
}
