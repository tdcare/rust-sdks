import { hilog } from '@kit.PerformanceAnalysisKit';
import { common } from '@kit.AbilityKit';
import { router } from '@kit.ArkUI';
import { LkRoom, LkLocalAudioTrack, LkLocalVideoTrack, LkRemoteVideoTrack, LkRemoteAudioTrack } from 'liblivekit.so';
import { AudioCaptureService } from '../services/AudioCaptureService';
import { VideoCaptureService } from '../services/VideoCaptureService';
import { AudioRenderService } from '../services/AudioRenderService';
import { VideoRenderService } from '../services/VideoRenderService';
import { requestPermissions } from '../utils/PermissionHelper';
import { RrtCallManager, RrtCallListener } from '../rrt/RrtCallManager';
import { RrtLevel, CallContextDto, CallInitiateResponse } from '../data/dto/Dtos';
import { SessionManager } from '../data/session/SessionManager';

const TAG = 'RoomPage';

// Local declarations mirroring the Rust napi(object) types from liblivekit.so.
// napi-ohos does not export #[napi(object)] structs as named module members,
// so we declare them here as ArkTS interfaces.
interface LkParticipantInfo {
  identity: string;
  sid: string;
  name: string;
  metadata: string;
  isLocal: boolean;
}

interface LkTrackInfo {
  sid: string;
  name: string;
  kind: string;
  source: string;
}

interface LkPublicationInfo {
  sid: string;
  name: string;
  kind: string;
  isSubscribed: boolean;
}

interface LkRoomEvent {
  eventType: string;
  participant: LkParticipantInfo | null;
  track: LkTrackInfo | null;
  publication: LkPublicationInfo | null;
  data: Uint8Array | null;
  topic: string | null;
  connectionState: string | null;
  reason: string | null;
  metadata: string | null;
}

@Entry
@Component
struct RoomPage {
  @State private url: string = 'wss://livekit.tdcare.cn';
  @State private token: string = 'eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.eyJleHAiOjE3ODA3ODYzMTcsImlzcyI6IkFQSWE3cndKaGlRdkVuNCIsIm5iZiI6MTc4MDc2NDcxNywic3ViIjoib2hvc19kZXZpY2VfMDIiLCJuYW1lIjoiIiwidmlkZW8iOnsicm9vbUNyZWF0ZSI6ZmFsc2UsInJvb21MaXN0IjpmYWxzZSwicm9vbVJlY29yZCI6ZmFsc2UsInJvb21BZG1pbiI6ZmFsc2UsInJvb21Kb2luIjp0cnVlLCJyb29tIjoicnJ0X2NhbGxfNzQ2NzIyNzM1MjE2NzA5NjIzMiIsImRlc3RpbmF0aW9uUm9vbSI6IiIsImNhblB1Ymxpc2giOnRydWUsImNhblN1YnNjcmliZSI6dHJ1ZSwiY2FuUHVibGlzaERhdGEiOnRydWUsImNhblB1Ymxpc2hTb3VyY2VzIjpbXSwiY2FuVXBkYXRlT3duTWV0YWRhdGEiOmZhbHNlLCJpbmdyZXNzQWRtaW4iOmZhbHNlLCJoaWRkZW4iOmZhbHNlLCJyZWNvcmRlciI6ZmFsc2V9LCJzaXAiOnsiYWRtaW4iOmZhbHNlLCJjYWxsIjpmYWxzZX0sInNoYTI1NiI6IiIsIm1ldGFkYXRhIjoiIiwiYXR0cmlidXRlcyI6e30sInJvb21Db25maWciOm51bGx9.kj8GZArhMRQS-UUE6kGi98wIOj0BrpHcH6Z8VEtRlbc';
  @State private connected: boolean = false;
  @State private connecting: boolean = false;
  @State private micEnabled: boolean = true;
  @State private camEnabled: boolean = true;
  @State private statusText: string = 'Disconnected';
  @State private remoteParticipants: string[] = [];
  @State private hasRemoteVideo: boolean = false;
  @State private cameraResolution: string = '';

  // ─── 快反发起模式 ───
  @State private isInitiateMode: boolean = false;
  @State private showLevelSelect: boolean = false;
  @State private levels: RrtLevel[] = [];
  @State private callInitiateResult: CallInitiateResponse | null = null;
  @State private initiatingCall: boolean = false;
  @State private currentCallId: string = '';
  @State private currentLivekitRoom: string = '';
  @State private currentTargets: string[] = [];

  // 通话计时
  private callSeconds: number = 0;
  private timerInterval: number = 0;

  private room: LkRoom | null = null;
  private audioCapture: AudioCaptureService = new AudioCaptureService();
  private videoCapture: VideoCaptureService = (() => { const vc = new VideoCaptureService(640, 480); globalThis.videoCaptureService = vc; return vc; })();


  
  private audioRender: AudioRenderService = new AudioRenderService();
  private videoRender: VideoRenderService = new VideoRenderService();
  private localAudioTrack: LkLocalAudioTrack | null = null;
  private localVideoTrack: LkLocalVideoTrack | null = null;

  // Track which SIDs we are already rendering to avoid duplicate starts
  // caused by double-fired trackSubscribed events (LiveKit SDK quirk).
  private renderingVideoTrackSid: string = '';
  private renderingAudioTrackSid: string = '';

  // Debounce timers: when a track is unsubscribed, don't stop rendering
  // immediately — wait 500ms in case it's a transient ICE flap that will
  // be followed by an immediate re-subscribe.
  private videoStopDebounceTimer: number = 0;
  private audioStopDebounceTimer: number = 0;

  // Remote video XComponent
  private remoteXController: XComponentController = new XComponentController();

  // Local preview XComponent
  private localXController: XComponentController = new XComponentController();
  private localPreviewSurfaceId: string = '';
  private localPreviewSurfaceResolve: ((surfaceId: string) => void) | null = null;

  aboutToAppear(): void {
    hilog.info(0x0000, TAG, 'RoomPage aboutToAppear');

    // When the render loop exits on its own (stream ended / errors), clear
    // our SID tracking so that subsequent track re-subscriptions restart it.
    this.videoRender.setOnRenderStopped(() => {
      hilog.info(0x0000, TAG, 'Render loop stopped — clearing SID tracking');
      this.renderingVideoTrackSid = '';
    });

    // 读取路由参数
    const params = router.getParams() as Record<string, Object>;
    if (params) {
      const mode = params['mode'] as string;
      if (mode === 'initiate') {
        this.isInitiateMode = true;
        this.showLevelSelect = true;
        this.statusText = '正在加载级别列表...';
        hilog.info(0x0000, TAG, '快反发起模式: 显示级别选择');
        this.loadLevels();
        return;
      }
    }

    // 非发起模式：demo 自动连接
    setTimeout(() => {
      hilog.info(0x0000, TAG, 'Auto-connect triggered');
      this.handleConnect();
    }, 2000);
  }

  /**
   * 加载快反级别列表（对齐 Android RrtCallActivity.loadLevels）
   */
  private loadLevels(): void {
    const mgr = RrtCallManager.getInstance();
    const cached = mgr.getCachedCallRules();
    if (cached.length > 0) {
      this.levels = cached.sort((a, b) => a.priority - b.priority);
      this.statusText = '请选择快反级别';
      return;
    }
    mgr.fetchCallRules().then((rules: RrtLevel[]) => {
      if (rules.length > 0) {
        this.levels = rules.sort((a, b) => a.priority - b.priority);
        this.statusText = '请选择快反级别';
      } else {
        this.statusText = '无可用快反级别';
      }
    });
  }

  /**
   * 用户选择了级别（对齐 Android onLevelSelected）
   */
  private async onLevelSelected(levelCode: string): Promise<void> {
    this.showLevelSelect = false;
    this.initiatingCall = true;
    this.statusText = '正在发起呼叫...';

    try {
      const callManager = RrtCallManager.getInstance();
      const result = await callManager.initiateCall(levelCode);
      if (!result) {
        this.statusText = '发起呼叫失败';
        this.initiatingCall = false;
        return;
      }

      this.callInitiateResult = result;
      this.currentCallId = result.callId;
      this.currentLivekitRoom = result.livekitRoom;
      this.currentTargets = result.targets;

      // 更新 LiveKit URL
      if (result.livekitUrl.length > 0) {
        this.url = result.livekitUrl;
      }

      hilog.info(0x0000, TAG,
        `呼叫已创建: callId=${result.callId}, room=${result.livekitRoom}, targets=${result.targets}`);

      // 生成 LiveKit Token 并加入房间
      await this.connectToRrtRoom();

    } catch (e) {
      hilog.error(0x0000, TAG, `发起呼叫异常: ${JSON.stringify(e)}`);
      this.statusText = `发起呼叫失败: ${e}`;
      this.initiatingCall = false;
    }
  }

  /**
   * 连接快反 LiveKit 房间
   */
  private async connectToRrtRoom(): Promise<void> {
    try {
      const callManager = RrtCallManager.getInstance();
      const deviceSn = `ohos_${this.currentCallId}`;
      const room = this.currentLivekitRoom;

      // 使用后端返回的 LiveKit URL，fallback 到配置中的
      if (this.callInitiateResult && this.callInitiateResult.livekitUrl.length > 0) {
        this.url = this.callInitiateResult.livekitUrl;
      } else {
        this.url = callManager.getLivekitUrl() || 'wss://livekit.tdcare.cn';
      }

      // 从后端或本地获取 LiveKit Token（对齐 rrt-web getLiveKitToken）
      this.statusText = '正在获取Token...';
      this.token = await callManager.getLiveKitToken(room, deviceSn, deviceSn);
      hilog.info(0x0000, TAG, `Token acquired for room=${room}`);

      // 使用 RoomPage 现有的连接逻辑
      await this.handleConnect();

    } catch (e) {
      hilog.error(0x0000, TAG, `连接快反房间异常: ${JSON.stringify(e)}`);
      this.statusText = `连接失败: ${e}`;
      this.initiatingCall = false;
    }
  }

  build() {
    Column({ space: 12 }) {
      // Title
      Text(this.isInitiateMode ? 'RRT 快反通话' : 'LiveKit OHOS Demo')
        .fontSize(24)
        .fontWeight(FontWeight.Bold)
        .margin({ top: 20 })

      // Status
      Text(this.statusText)
        .fontSize(14)
        .fontColor(this.connected ? '#00AA00' : '#AA0000')

      // 显示呼叫信息
      if (this.isInitiateMode && this.currentCallId.length > 0) {
        Text(`呼叫ID: ${this.currentCallId}`)
          .fontSize(11)
          .fontColor('#888888')
        Text(`目标: ${this.currentTargets.join(', ')}`)
          .fontSize(11)
          .fontColor('#888888')
      }

      // 级别选择 (快反发起模式)
      if (this.showLevelSelect) {
        Column({ space: 12 }) {
          Text(this.statusText)
            .fontSize(18)
            .fontWeight(FontWeight.Medium)
            .margin({ top: 16, bottom: 16 })

          ForEach(this.levels, (level: RrtLevel) => {
            Button(level.levelName)
              .fontSize(18)
              .width('80%')
              .height(52)
              .margin({ top: 6, bottom: 6 })
              .onClick(() => { this.onLevelSelected(level.levelCode); })
          })

          Button('取消')
            .fontSize(16)
            .width('60%')
            .height(44)
            .backgroundColor('#999999')
            .margin({ top: 20 })
            .onClick(() => { router.back(); })
        }
        .width('100%')
        .padding(20)
        .alignItems(HorizontalAlign.Center)
      }

      // 发起中状态
      if (this.initiatingCall && !this.showLevelSelect && !this.connected) {
        Text(this.statusText)
          .fontSize(18)
          .fontColor('#FF9800')
          .margin({ top: 40 })
      }

      // Connection form (demo 模式)
      if (!this.connected && !this.showLevelSelect && !this.initiatingCall) {
        Column({ space: 8 }) {
          TextInput({ text: this.url, placeholder: 'Server URL (wss://...)' })
            .width('90%')
            .onChange((value: string) => { this.url = value; })

          TextInput({ text: this.token, placeholder: 'Access Token' })
            .width('90%')
            .type(InputType.Normal)
            .onChange((value: string) => { this.token = value; })

          Button(this.connecting ? 'Connecting...' : 'Connect')
            .width('60%')
            .enabled(!this.connecting)
            .onClick(() => { this.handleConnect(); })
        }
      }

      // Video area
      if (this.connected) {
        Column({ space: 8 }) {
          // Remote video (primary display via NativeWindow Surface)
          Text('Remote Video')
            .fontSize(12)
            .fontColor('#666666')
          Stack() {
            XComponent({
              id: 'remote-video-surface',
              type: XComponentType.SURFACE,
              controller: this.remoteXController,
            })
              .onLoad(() => {
                const surfaceId: string = this.remoteXController.getXComponentSurfaceId();
                hilog.info(0x0000, TAG,
                  'Remote XComponent surface ready: id=%{public}s', surfaceId);
                // Pass surface ID to native renderer (1280x720 = expected remote resolution)
                this.videoRender.setSurfaceId(surfaceId, 1280, 720);
              })
              .onDestroy(() => {
                hilog.info(0x0000, TAG, 'Remote XComponent surface destroyed');
              })
              .width('100%')
              .height('100%')

            // Placeholder text while waiting for video
            if (!this.hasRemoteVideo) {
              Column() {
                Text('Waiting for remote video...')
                  .fontColor('#999999')
                  .fontSize(14)
              }
              .width('100%')
              .height('100%')
              .justifyContent(FlexAlign.Center)
            }
          }
          .width('95%')
          .height(300)
          .backgroundColor('#000000')
          .borderRadius(8)

          // Local camera preview (small PIP-style)
          Text('Local Camera')
            .fontSize(12)
            .fontColor('#666666')
          Stack() {
            XComponent({
              id: 'local-preview-surface',
              type: XComponentType.SURFACE,
              controller: this.localXController,
            })
              .onLoad(() => {
                const surfaceId: string = this.localXController.getXComponentSurfaceId();
                hilog.info(0x0000, TAG,
                  'Local preview XComponent surface ready: id=%{public}s', surfaceId);
                this.localPreviewSurfaceId = surfaceId;
                // Resolve the promise if we're waiting for the surface
                if (this.localPreviewSurfaceResolve !== null) {
                  this.localPreviewSurfaceResolve(surfaceId);
                  this.localPreviewSurfaceResolve = null;
                }
              })
              .onDestroy(() => {
                hilog.info(0x0000, TAG, 'Local preview XComponent surface destroyed');
                this.localPreviewSurfaceId = '';
              })
              .width('100%')
              .height('100%')

            // Placeholder when camera is not ready
            if (this.cameraResolution.length === 0) {
              Column() {
                Text('Camera preview')
                  .fontColor('#666666')
                  .fontSize(11)
              }
              .width('100%')
              .height('100%')
              .justifyContent(FlexAlign.Center)
            }
          }
          // v38-fix: minimum 160x214 to avoid transparent Surface rendering on some OHOS devices
          .width('95%')
          .height(220)
          .backgroundColor('#000000')
          .borderRadius(8)

          if (this.cameraResolution.length > 0) {
            Text(`Camera: ${this.cameraResolution}`)
              .fontSize(11)
              .fontColor('#888888')
          }
        }

        // Controls
        Row({ space: 16 }) {
          Button(this.micEnabled ? 'Mute' : 'Unmute')
            .onClick(() => { this.toggleMic(); })

          Button(this.camEnabled ? 'Cam Off' : 'Cam On')
            .onClick(() => { this.toggleCam(); })

          Button('挂断')
            .fontColor(Color.White)
            .backgroundColor(Color.Red)
            .onClick(() => { this.handleHangup(); })
        }
        .margin({ top: 12 })

        // Participants
        if (this.remoteParticipants.length > 0) {
          Text('Participants:')
            .fontSize(14)
            .margin({ top: 8 })
          ForEach(this.remoteParticipants, (identity: string) => {
            Text(`  - ${identity}`)
              .fontSize(12)
          })
        }
      }
    }
    .width('100%')
    .height('100%')
    .padding(16)
  }

  /**
   * Wait for the local preview XComponent surface to become available.
   * Returns the surfaceId, or empty string if timed out.
   *
   * Uses both onLoad callback AND polling, because on some OHOS API 12
   * devices the onLoad event fires very late or not at all.
   */
  private async waitForLocalPreviewSurface(timeoutMs: number = 5000): Promise<string> {
    const startTime: number = Date.now();
    const pollIntervalMs: number = 200;

    while (Date.now() - startTime < timeoutMs) {
      // Check if the callback already resolved it
      if (this.localPreviewSurfaceId.length > 0) {
        return this.localPreviewSurfaceId;
      }

      // Poll getXComponentSurfaceId() directly in case onLoad didn't fire
      try {
        const surfaceId: string = this.localXController.getXComponentSurfaceId();
        if (surfaceId.length > 0) {
          hilog.info(0x0000, TAG,
            'Local preview surface discovered via polling: id=%{public}s', surfaceId);
          this.localPreviewSurfaceId = surfaceId;
          // Clear the pending resolve
          this.localPreviewSurfaceResolve = null;
          return surfaceId;
        }
      } catch (_e) {
        // getXComponentSurfaceId may throw if XComponent isn't ready yet
      }

      // Wait before polling again
      await new Promise<void>((resolve) => {
        setTimeout(() => { resolve(); }, pollIntervalMs);
      });
    }

    hilog.warn(0x0000, TAG, 'Local preview surface timeout (polling), proceeding without preview');
    this.localPreviewSurfaceResolve = null;
    return '';
  }

  private async handleConnect(): Promise<void> {
    if (!this.url || !this.token) {
      this.statusText = 'Please enter URL and token';
      return;
    }

    // Ensure clean state on reconnection: stop stale captures, then
    // properly await the old room's disconnect so UDP sockets are freed.
    if (this.connected || this.room) {
      hilog.warn(0x0000, TAG, 'handleConnect: cleaning up previous session before reconnect');
      await this.audioCapture.stop().catch(() => {});
      await this.videoCapture.stop().catch(() => {});
      await this.videoCapture.release().catch(() => {});
      await this.audioRender.stop().catch(() => {});
      await this.videoRender.stop().catch(() => {});
      try { await this.room?.disconnect(); } catch (_e) { /* ignore */ }
      this.room = null;
      this.connected = false;
      this.localAudioTrack = null;
      this.localVideoTrack = null;
      // Give the OS time to release UDP sockets before binding again.
      await new Promise<void>((r) => setTimeout(r, 1500));
    }

    this.connecting = true;
    this.statusText = 'Connecting...';

    try {
      const context = getContext(this) as common.UIAbilityContext;
      const granted = await requestPermissions(context);
      if (!granted) {
        this.statusText = 'Permissions denied';
        this.connecting = false;
        return;
      }

      // 1. Connect to LiveKit server
      hilog.info(0x0000, TAG, `Connecting to ${this.url}...`);
      this.room = new LkRoom();
      await this.room.connect(this.url, this.token, { autoSubscribe: true });
      hilog.info(0x0000, TAG, 'Room connected');
      hilog.info(0x0000, TAG, `Local participant: identity=${this.room.localParticipant?.identity}, sid=${this.room.localParticipant?.sid}`);

      // 2. Register event listener IMMEDIATELY after connect, BEFORE publishing.
      this.room.on((event: LkRoomEvent) => { this.handleRoomEvent(event); });
      hilog.info(0x0000, TAG, 'Event listener registered');

      // 3. Set connected=true EARLY to trigger XComponent UI rendering.
      //    The XComponent needs time to load before the camera can use its surface.
      this.connected = true;
      this.statusText = 'Starting camera...';
      hilog.info(0x0000, TAG, 'UI connected=true, XComponent loading...');

      // 4. Start audio capture (runs in parallel with XComponent loading)
      await this.audioCapture.start();
      hilog.info(0x0000, TAG, 'Audio capture started');

      // 5. Delay to let the UI framework render the XComponent.
      //    Longer delay (1000ms) for slower OHOS API 12 devices.
      await new Promise<void>((resolve) => {
        setTimeout(() => { resolve(); }, 1000);
      });
      hilog.info(0x0000, TAG, 'Post-UI-render delay complete, waiting for surface...');

      // 6. Wait for local preview XComponent surface (longer timeout: 5s)
      const previewSurfaceId: string = await this.waitForLocalPreviewSurface(5000);
      hilog.info(0x0000, TAG,
        `Local preview surface: ${previewSurfaceId.length > 0 ? previewSurfaceId : '(none)'}`);

      // 7. Initialize camera with preview surface
      const cameraOk: boolean = await this.videoCapture.initialize(context, previewSurfaceId);
      if (!cameraOk) {
        hilog.warn(0x0000, TAG, 'Camera initialization failed, video will not be available');
        this.statusText = 'Connected (camera failed)';
      } else {
        this.cameraResolution =
          `${this.videoCapture.getActualWidth()}x${this.videoCapture.getActualHeight()}`;
        hilog.info(0x0000, TAG, `Camera initialized: ${this.cameraResolution}`);
      }

      // 7. Start video capture (enables frame processing in ImageReceiver callback)
      try {
        await this.videoCapture.start();
        hilog.info(0x0000, TAG, 'Video capture started (camera)');
      } catch (videoErr) {
        hilog.warn(0x0000, TAG, 'Video capture start failed: %{public}s',
          videoErr instanceof Error ? videoErr.message : JSON.stringify(videoErr));
      }

      // 8. Create tracks from the capture sources
      const audioSource = this.audioCapture.getSource();
      hilog.info(0x0000, TAG, 'Creating audio track from source');
      const audioTrack: LkLocalAudioTrack =
        LkLocalAudioTrack.createTrack('microphone', audioSource);
      hilog.info(0x0000, TAG, `Audio track created: name=${audioTrack.name}, kind=${audioTrack.kind}`);

      const videoSource = this.videoCapture.getSource();
      const vw = this.videoCapture.getActualWidth();
      const vh = this.videoCapture.getActualHeight();
      hilog.info(0x0000, TAG, `Creating video track from source (${vw}x${vh})`);
      const videoTrack: LkLocalVideoTrack =
        LkLocalVideoTrack.createTrack('camera', videoSource);
      hilog.info(0x0000, TAG, `Video track created: name=${videoTrack.name}, kind=${videoTrack.kind}`);

      // Store references for mute/unmute control
      this.localAudioTrack = audioTrack;
      this.localVideoTrack = videoTrack;

      // 9. Publish tracks
      hilog.info(0x0000, TAG, 'Publishing audio track...');
      await this.room.localParticipant!.publishAudioTrack(audioTrack);
      hilog.info(0x0000, TAG, 'Audio track published OK');

      hilog.info(0x0000, TAG, 'Publishing video track (auto-detect codec, Camera source)...');
      await this.room.localParticipant!.publishVideoTrack(videoTrack);
      hilog.info(0x0000, TAG, 'Video track published OK');

      // 10. Check for already-subscribed remote video tracks
      this.checkExistingRemoteTracks();

      this.statusText = 'Connected';
      hilog.info(0x0000, TAG, 'Connected successfully');
    } catch (err) {
      const errMsg = err instanceof Error ? err.message : JSON.stringify(err);
      const errStack = err instanceof Error && err.stack ? err.stack : '';
      this.statusText = `Connection failed: ${errMsg}`;
      hilog.error(0x0000, TAG, 'Connect error detail: %{public}s', errMsg);
      if (errStack) {
        hilog.error(0x0000, TAG, 'Connect error stack: %{public}s', errStack);
      }
      // Clean up on failure
      this.connected = false;
      try {
        await this.videoCapture.release();
      } catch (_e) { /* ignore */ }
    } finally {
      this.connecting = false;
    }
  }

  /**
   * Check for already-subscribed remote video tracks.
   * When autoSubscribe=true, tracks may be subscribed during connect()
   * before the event listener is registered.
   */
  private checkExistingRemoteTracks(): void {
    if (!this.room) return;

    const participants: string[] = this.room.getRemoteParticipantIdentities();
    hilog.info(0x0000, TAG, 'Checking existing remote participants: %{public}d', participants.length);

    for (const identity of participants) {
      // Check existing video tracks
      const videoTrackSids: string[] = this.room!.getRemoteVideoTrackSids(identity);
      hilog.info(0x0000, TAG, 'Participant %{public}s has %{public}d video tracks',
        identity, videoTrackSids.length);
      for (const sid of videoTrackSids) {
        hilog.info(0x0000, TAG, 'Found existing remote video track: participant=%{public}s sid=%{public}s',
          identity, sid);
        this.startRemoteVideoRender(identity, sid);
      }

      // Check existing audio tracks
      const audioTrackSids: string[] = this.room!.getRemoteAudioTrackSids(identity);
      hilog.info(0x0000, TAG, 'Participant %{public}s has %{public}d audio tracks',
        identity, audioTrackSids.length);
      for (const sid of audioTrackSids) {
        hilog.info(0x0000, TAG, 'Found existing remote audio track: participant=%{public}s sid=%{public}s',
          identity, sid);
        this.startRemoteAudioRender(identity, sid);
      }
    }
  }

  /**
   * Start rendering a remote video track by participant identity and track SID.
   */
  private startRemoteVideoRender(participantIdentity: string, trackSid: string): void {
    if (!this.room) return;

    // Deduplicate: skip if we're already rendering this exact track.
    // The LiveKit SDK occasionally fires trackSubscribed twice for the same
    // track during reconnections, which would crash the current render loop.
    if (this.renderingVideoTrackSid === trackSid) {
      hilog.info(0x0000, TAG,
        'Video track %{public}s already rendering, skipping duplicate', trackSid);
      return;
    }
    this.renderingVideoTrackSid = trackSid;

    const nativeTrack: LkRemoteVideoTrack | null = this.room.getRemoteVideoTrack(
      participantIdentity, trackSid
    );
    if (nativeTrack) {
      hilog.info(0x0000, TAG, 'Starting remote video render (Surface mode) for track: %{public}s from %{public}s',
        trackSid, participantIdentity);
      this.hasRemoteVideo = true;
      this.videoRender.start(nativeTrack).catch((err: Error) => {
        hilog.error(0x0000, TAG, 'Failed to start video render: %{public}s', err.message);
      });
    } else {
      this.renderingVideoTrackSid = '';
      hilog.error(0x0000, TAG,
        'Native video track not found for participant=%{public}s sid=%{public}s',
        participantIdentity, trackSid);
    }
  }

  /**
   * Start rendering a remote audio track by participant identity and track SID.
   */
  private startRemoteAudioRender(participantIdentity: string, trackSid: string): void {
    if (!this.room) return;

    // Deduplicate: skip if we're already rendering this exact track.
    if (this.renderingAudioTrackSid === trackSid) {
      hilog.info(0x0000, TAG,
        'Audio track %{public}s already rendering, skipping duplicate', trackSid);
      return;
    }
    this.renderingAudioTrackSid = trackSid;

    const nativeTrack: LkRemoteAudioTrack | null = this.room.getRemoteAudioTrack(
      participantIdentity, trackSid
    );
    if (nativeTrack) {
      hilog.info(0x0000, TAG, 'Starting remote audio render for track: %{public}s from %{public}s',
        trackSid, participantIdentity);
      this.audioRender.start(nativeTrack).catch((err: Error) => {
        hilog.error(0x0000, TAG, 'Failed to start audio render: %{public}s', err.message);
      });
    } else {
      this.renderingAudioTrackSid = '';
      hilog.error(0x0000, TAG,
        'Native audio track not found for participant=%{public}s sid=%{public}s',
        participantIdentity, trackSid);
    }
  }

  private handleRoomEvent(event: LkRoomEvent): void {
    switch (event.eventType) {
      case 'dataReceived': {
        const topic = event.topic ?? '';
        const len = event.data ? event.data.byteLength : 0;
        hilog.info(0x0000, TAG, 'Data received: topic=%{public}s len=%{public}d', topic, len);
        break;
      }
      case 'participantConnected': {
        const identity = event.participant ? event.participant.identity : '';
        const sid = event.participant ? event.participant.sid : '';
        hilog.info(0x0000, TAG,
          'Participant connected: identity=%{public}s sid=%{public}s', identity, sid);
        if (identity) {
          this.remoteParticipants = [...this.remoteParticipants, identity];
        }
        break;
      }
      case 'participantDisconnected': {
        const identity = event.participant ? event.participant.identity : '';
        hilog.info(0x0000, TAG, 'Participant disconnected: %{public}s', identity);
        if (identity) {
          this.remoteParticipants =
            this.remoteParticipants.filter((p: string) => p !== identity);
        }
        break;
      }
      case 'trackSubscribed': {
        const pId = event.participant ? event.participant.identity : '?';
        const tKind = event.track ? event.track.kind : '?';
        const tSid = event.track ? event.track.sid : '?';
        const tName = event.track ? event.track.name : '?';
        hilog.info(0x0000, TAG,
          'Track subscribed: participant=%{public}s kind=%{public}s sid=%{public}s name=%{public}s',
          pId, tKind, tSid, tName);

        // Start video rendering for remote video tracks (Surface mode)
        if (tKind === 'video' && event.track && event.participant) {
          // Cancel any pending debounce stop — track is back before timeout expired
          if (this.videoStopDebounceTimer) {
            clearTimeout(this.videoStopDebounceTimer);
            this.videoStopDebounceTimer = 0;
            hilog.info(0x0000, TAG, 'Cancelled pending video stop (track %{public}s re-subscribed)', tSid);
          }
          this.startRemoteVideoRender(event.participant.identity, event.track.sid);
        }

        // Start audio rendering for remote audio tracks
        if (tKind === 'audio' && event.track && event.participant) {
          if (this.audioStopDebounceTimer) {
            clearTimeout(this.audioStopDebounceTimer);
            this.audioStopDebounceTimer = 0;
            hilog.info(0x0000, TAG, 'Cancelled pending audio stop (track %{public}s re-subscribed)', tSid);
          }
          this.startRemoteAudioRender(event.participant.identity, event.track.sid);
        }
        break;
      }
      case 'trackUnsubscribed': {
        const pId = event.participant ? event.participant.identity : '?';
        const tKind = event.track ? event.track.kind : '?';
        const tSid = event.track ? event.track.sid : '';
        hilog.info(0x0000, TAG,
          'Track unsubscribed: participant=%{public}s kind=%{public}s sid=%{public}s', pId, tKind, tSid);

        // Stop video rendering when video track is unsubscribed
        // Debounced: wait 500ms in case this is a transient ICE flap followed by immediate re-subscribe
        if (tKind === 'video') {
          if (this.videoStopDebounceTimer) {
            clearTimeout(this.videoStopDebounceTimer);
          }
          this.videoStopDebounceTimer = setTimeout(() => {
            hilog.info(0x0000, TAG, 'Stopping remote video render (debounced)');
            this.videoRender.stop();
            this.hasRemoteVideo = false;
            this.renderingVideoTrackSid = '';
            this.videoStopDebounceTimer = 0;
          }, 500);
        }

        // Stop audio rendering when audio track is unsubscribed
        if (tKind === 'audio') {
          if (this.audioStopDebounceTimer) {
            clearTimeout(this.audioStopDebounceTimer);
          }
          this.audioStopDebounceTimer = setTimeout(() => {
            hilog.info(0x0000, TAG, 'Stopping remote audio render (debounced)');
            this.audioRender.stop();
            this.renderingAudioTrackSid = '';
            this.audioStopDebounceTimer = 0;
          }, 500);
        }
        break;
      }
      case 'trackPublished': {
        const pId = event.participant ? event.participant.identity : '?';
        const tKind = event.track ? event.track.kind : '?';
        const pubSid = event.publication ? event.publication.sid : '?';
        hilog.info(0x0000, TAG,
          'Remote track published: participant=%{public}s kind=%{public}s sid=%{public}s',
          pId, tKind, pubSid);
        break;
      }
      case 'trackUnpublished': {
        const pId = event.participant ? event.participant.identity : '?';
        const tKind = event.track ? event.track.kind : '?';
        hilog.info(0x0000, TAG,
          'Remote track unpublished: participant=%{public}s kind=%{public}s', pId, tKind);
        break;
      }
      case 'activeSpeakersChanged': {
        const speakers = event.participant ? event.participant.identity : 'none';
        hilog.info(0x0000, TAG, 'Active speakers: %{public}s', speakers);
        break;
      }
      case 'connectionStateChanged': {
        const state = event.connectionState ?? '';
        hilog.info(0x0000, TAG, 'Connection state: %{public}s', state);
        // Auto-restart renders if reconnecting delivers subscribed tracks
        if (state === 'connected') {
          this.checkExistingRemoteTracks();
        }
        break;
      }
      case 'reconnected': {
        hilog.warn(0x0000, TAG, 'Reconnected - DTLS key exchange should be complete');
        // Delay 2 seconds to let DTLS/SRTP fully establish before checking remote tracks
        setTimeout(() => {
          hilog.warn(0x0000, TAG, 'Reconnected delay done, checking remote tracks');
          this.checkExistingRemoteTracks();
        }, 2000);
        break;
      }
      case 'disconnected': {
        const reason = event.reason ?? '';
        hilog.warn(0x0000, TAG, 'Disconnected: %{public}s, resetting state', reason);
        // Reset state so UI reflects reality
        this.connected = false;
        this.remoteParticipants = [];
        this.hasRemoteVideo = false;
        this.renderingVideoTrackSid = '';
        this.renderingAudioTrackSid = '';
        // Clear debounce timers
        if (this.videoStopDebounceTimer) { clearTimeout(this.videoStopDebounceTimer); this.videoStopDebounceTimer = 0; }
        if (this.audioStopDebounceTimer) { clearTimeout(this.audioStopDebounceTimer); this.audioStopDebounceTimer = 0; }
        // Stop render + capture services
        this.audioRender.stop().catch((e: Error) =>
          hilog.warn(0x0000, TAG, 'Audio render stop on disconnect: %{public}s', e.message));
        this.videoRender.stop().catch((e: Error) =>
          hilog.warn(0x0000, TAG, 'Video render stop on disconnect: %{public}s', e.message));
        this.audioCapture.stop().catch((e: Error) =>
          hilog.warn(0x0000, TAG, 'Audio capture stop on disconnect: %{public}s', e.message));
        this.videoCapture.stop().catch((e: Error) =>
          hilog.warn(0x0000, TAG, 'Video capture stop on disconnect: %{public}s', e.message));
        this.videoCapture.release().catch((_e: Error) => { /* ignore */ });
        this.room?.disconnect().catch((_e: Error) => { /* ignore */ });
        this.room = null;
        this.localAudioTrack = null;
        this.localVideoTrack = null;
        this.statusText = `Disconnected: ${reason}`;
        break;
      }
      default:
        hilog.info(0x0000, TAG, 'Room event: %{public}s', event.eventType);
        break;
    }
  }

  private toggleMic(): void {
    this.micEnabled = !this.micEnabled;
    hilog.info(0x0000, TAG, 'Mic %{public}s', this.micEnabled ? 'enabled' : 'muted');
    this.localAudioTrack?.setMuted(!this.micEnabled);
  }

  private toggleCam(): void {
    this.camEnabled = !this.camEnabled;
    hilog.info(0x0000, TAG, 'Camera %{public}s', this.camEnabled ? 'enabled' : 'disabled');
    this.localVideoTrack?.setMuted(!this.camEnabled);
  }

  /**
   * 挂断通话（快反模式）
   */
  private handleHangup(): void {
    if (this.isInitiateMode) {
      RrtCallManager.getInstance().hangup();
    }
    this.handleDisconnect();
    router.back();
  }

  private async handleDisconnect(): Promise<void> {
    // Stop render services first
    await this.videoRender.stop();
    await this.audioRender.stop();
    try {
      await this.audioCapture.stop();
    } catch (e) {
      hilog.warn(0x0000, TAG, 'Audio stop error: %{public}s', JSON.stringify(e));
    }
    try {
      await this.videoCapture.stop();
    } catch (e) {
      hilog.warn(0x0000, TAG, 'Video stop error: %{public}s', JSON.stringify(e));
    }

    // Release camera resources
    try {
      await this.videoCapture.release();
    } catch (e) {
      hilog.warn(0x0000, TAG, 'Video release error: %{public}s', JSON.stringify(e));
    }

    await this.room?.disconnect();
    this.room = null;
    this.localAudioTrack = null;
    this.localVideoTrack = null;
    this.connected = false;
    this.remoteParticipants = [];
    this.hasRemoteVideo = false;
    this.renderingVideoTrackSid = '';
    this.renderingAudioTrackSid = '';
    // Clear debounce timers
    if (this.videoStopDebounceTimer) { clearTimeout(this.videoStopDebounceTimer); this.videoStopDebounceTimer = 0; }
    if (this.audioStopDebounceTimer) { clearTimeout(this.audioStopDebounceTimer); this.audioStopDebounceTimer = 0; }
    this.cameraResolution = '';
    this.localPreviewSurfaceId = '';
    this.statusText = 'Disconnected';
    hilog.info(0x0000, TAG, 'Disconnected and capture stopped');
  }

}
