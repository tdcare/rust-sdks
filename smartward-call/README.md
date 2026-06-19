# smartward-call

SmartWard 呼叫引擎 — 纯 WebRTC P2P/SFU 双模通信库。不包含任何 MQTT/信令逻辑，信令由上层 (Android/OHOS) 自行处理。

## 架构定位

```
上层 (Android / OHOS)
  ├── MQTT 信令 (Topic 路由、消息序列化)
  ├── 会话编排 (CallSessionManager)
  └── UI 交互
       │
       │ FFI: push SDP/ICE in, poll events out
       ▼
smartward-call (本 crate)
  ├── WebRtcEngine   — 统一入口
  ├── P2pManager     — PeerConnection 直连 (科内)
  ├── SfuManager     — LiveKit SFU (跨科室/广播)
  └── SessionRouter  — P2P/SFU 决策 (纯函数)
       │
       ▼
libwebrtc / livekit  (rust-sdks workspace)
```

## 构建

```bash
cargo build -p smartward-call
```

## API 概览

### P2P (科内点对点)

```rust
let mut engine = WebRtcEngine::new();

// 1. 创建连接
let handle = engine.create_p2p_connection(&P2pConfig::default());

// 2. 创建 Offer → 上层通过 MQTT 发送
let offer = engine.create_offer(handle).unwrap();

// 3. 收到远端 Answer → 设置远端 SDP
engine.set_remote_sdp(handle, &answer_sdp).unwrap();

// 4. 轮询事件
for event in engine.poll_events() {
    match event {
        EngineEvent::P2pIceCandidate { handle, candidate } => {
            // → MQTT 发送 ICE
        }
        EngineEvent::P2pStateChange { handle, state } => {
            // → 更新 UI
        }
        _ => {}
    }
}
```

### SFU (跨科室 / 广播)

```rust
// 1. 创建 SFU 会话
let sfu = engine.create_sfu_session(SfuConfig {
    url: "ws://livekit-server:7880".into(),
    room_name: "consult-wardA-wardB".into(),
    participant_identity: "nurse-ns001".into(),
    token: "livekit-jwt-token".into(),
});

// 2. 连接 LiveKit 房间
engine.connect_sfu(sfu).unwrap();
```

### 路由决策

```rust
// 科内 → P2P
SessionRouter::resolve("wardA", "wardA", false)  // P2P

// 跨科室 → SFU
SessionRouter::resolve("wardA", "wardB", false)  // SFU

// 广播 → SFU
SessionRouter::resolve("wardA", "wardA", true)   // SFU
```

## 运行测试

```bash
cargo test -p smartward-call
```

## 平台支持

| 平台 | P2P 后端 | SFU 后端 |
|------|---------|---------|
| Android | `libwebrtc` (C++ webrtc-sys) | `livekit` |
| OHOS | `libwebrtc` (纯 Rust rtc) | `livekit` |
| Desktop | `libwebrtc` (C++ webrtc-sys) | `livekit` |

`libwebrtc` 通过 `#[cfg(target_env = "ohos")]` 自动切换后端，本 crate 无需任何平台相关代码。
