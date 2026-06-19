# rust-sdks 项目架构与功能全景

## 项目信息
- 路径: D:\tdcare\livekit\rust-sdks
- 仓库: https://github.com/livekit/rust-sdks
- License: Apache-2.0
- 类型: Cargo workspace (resolver=2)
- 目标平台: Windows/macOS/Linux/iOS/Android/HarmonyOS

## Workspace 成员

| Crate | 版本 | 用途 | 路径 |
|-------|------|------|------|
| livekit | 0.7.42 | 主 SDK | livekit/ |
| livekit-api | 0.4.24 | 服务端 SDK | livekit-api/ |
| livekit-protocol | 0.7.7 | Protobuf 协议 | livekit-protocol/ |
| livekit-runtime | 0.4.0 | 运行时抽象 | livekit-runtime/ |
| livekit-datatrack | 0.1.7 | 数据轨道 | livekit-datatrack/ |
| livekit-wakeword | - | 唤醒词检测 | livekit-wakeword/ |
| livekit-ffi | 0.12.60 | C FFI 绑定 | livekit-ffi/ |
| livekit-uniffi | - | UniFFI 绑定 | livekit-uniffi/ |
| livekit-napi-ohos | - | HarmonyOS 绑定 | livekit-napi-ohos/ |
| libwebrtc | 0.3.34 | WebRTC 封装 | libwebrtc/ |
| webrtc-sys | 0.3.32 | WebRTC FFI | webrtc-sys/ |
| rtc-patched | 0.20.0-alpha.1 | webrtc-rs fork | rtc-patched/ |
| yuv-sys | 0.3.14 | libyuv FFI | yuv-sys/ |
| soxr-sys | 0.1.3 | soxr 重采样 | soxr-sys/ |
| imgproc | 0.3.19 | 图像处理 | imgproc/ |
| device-info | 0.1.1 | 设备信息 | device-info/ |

## 架构层次

```
livekit (main SDK)
  +-- room/        : Room/RoomSession/Options/DataStream/E2EE/DataTrack/RPC
  +-- track/       : AudioTrack/VideoTrack/Local*/Remote*
  +-- participant/ : LocalParticipant/RemoteParticipant
  +-- publication/ : LocalTrackPublication/RemoteTrackPublication
  +-- rtc_engine/  : RtcEngine/SignalClient/PeerTransport/RtcSession
  +-- platform_audio/ : PlatformAudio (ADM管理)
  +-- utils/       : Observer/Promise/Debouncer/TakeCell/TtlMap/TxQueue
  +-- plugin.rs    : Audio filter plugin system

livekit-api (server SDK)
  +-- access_token.rs   : JWT Token 生成/验证
  +-- services/         : Twirp REST API (Room/Egress/Ingress/SIP/AgentDispatch/Connector)
  +-- signal_client/    : WebSocket 信令客户端
  +-- webhooks.rs       : Webhook 接收验证
  +-- http_client.rs    : HTTP 客户端抽象

livekit-runtime
  +-- tokio/        : tokio runtime
  +-- async_std/    : async-std runtime
  +-- dispatcher/   : 全局 dispatcher

livekit-protocol
  基于 prost 生成的 Protobuf 类型

rtc-patched (独立 workspace)
  +-- rtc/          : webrtc-rs 核心
  +-- rtc-media/    : 媒体轨道
  +-- rtc-datachannel/
  +-- rtc-ice/rtc-dtls/rtc-sctp/
  +-- rtc-rtp/rtc-rtcp/rtc-srtp/
  +-- rtc-sdp/rtc-stun/rtc-turn/
  +-- rtc-mdns/rtc-interceptor/

examples/
  +-- basic_room/          : PlatformAudio, WAV playback
  +-- basic_data_track/    : DataTrack
  +-- basic_text_stream/   : TextStream
  +-- encrypted_text_stream/ : E2EE + TextStream
  +-- rpc/                 : RPC 完整示例
  +-- agent_dispatch/      : Agent 调度
  +-- local_audio/         : 本地音频
  +-- local_video/         : 本地视频
  +-- play_from_disk/      : 文件播放
  +-- save_to_disk/        : 保存到磁盘
  +-- screensharing/       : 屏幕共享
  +-- send_bytes/          : 字节流发送
  +-- data_track_benchmark/ : 性能测试
  +-- api/                 : 服务端 API
  +-- webhooks/            : Webhooks
  +-- mobile/              : Android/iOS
  +-- wgpu_room/           : egui/wgpu 桌面
  +-- ohos-livekit-app/    : HarmonyOS 端到端应用