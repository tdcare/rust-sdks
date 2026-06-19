# LiveKit OHOS Demo

Complete audio/video communication demo for OpenHarmony using LiveKit Rust SDK.

## Prerequisites

- DevEco Studio 5.0+
- OpenHarmony SDK 5.0 (API 12)
- liblivekit.so built with `ohrs build` from the livekit-napi-ohos crate

## Setup

1. Build liblivekit.so:
   ```bash
   cd ../../livekit-napi-ohos
   ohrs build
   cp dist/arm64-v8a/liblivekit.so ../examples/ohos-livekit-app/libs/arm64-v8a/
   ```

2. Open this project in DevEco Studio

3. Connect an OHOS device and run

## Features

- Room connection/disconnection
- Real-time audio capture (microphone) and playback
- Real-time video capture (camera) and rendering
- Remote participant audio/video subscription
- Mute/unmute controls
