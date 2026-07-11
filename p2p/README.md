# p2p

SmartWard P2P 通信库 — 纯 WebRTC PeerConnection 直连。不包含任何 MQTT/信令逻辑，信令由上层 (Android/OHOS) 自行处理。

## 架构定位

```
上层 (Android / OHOS)
  ├── MQTT 信令 (Topic 路由、消息序列化)
  ├── 会话编排 (CallSessionManager)
  └── UI 交互
       │
       │ FFI: push SDP/ICE in, poll events out
       ▼
p2p (本 crate)
  ├── WebRtcEngine   — 统一入口
  └── P2pManager     — PeerConnection 直连
       │
       ▼
libwebrtc  (rust-sdks workspace)
```

## 构建

```bash
cargo build -p p2p
```

## API 概览

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

## 运行测试

```bash
cargo test -p p2p
```

## 平台支持

| 平台 | P2P 后端 |
|------|---------|
| Android | `libwebrtc` (C++ webrtc-sys) |
| OHOS | `libwebrtc` (纯 Rust rtc) |
| Desktop | `libwebrtc` (C++ webrtc-sys) |

`libwebrtc` 通过 `#[cfg(target_env = "ohos")]` 自动切换后端，本 crate 无需任何平台相关代码。
