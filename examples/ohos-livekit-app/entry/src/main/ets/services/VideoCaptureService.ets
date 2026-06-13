/**
 * Video capture service using OHOS Camera Kit for real camera integration.
 *
 * Architecture:
 *   Camera → Session ┬→ PreviewOutput → XComponent (local preview)
 *                     └→ PreviewOutput → ImageReceiver → NV21 → I420 → LkVideoSource
 *
 * ImageReceiver is created with JPEG format. Frames arrive as JPEG bitstream,
 * are decoded to RGBA via OHOS createPixelMap, then converted to I420 before
 * being pushed to LkVideoSource.
 */

import { hilog } from '@kit.PerformanceAnalysisKit';
import { camera } from '@kit.CameraKit';
import { image } from '@kit.ImageKit';
import { common } from '@kit.AbilityKit';
import { BusinessError } from '@kit.BasicServicesKit';
import { LkVideoSource } from 'liblivekit.so';

const TAG = 'VideoCapture';

export interface VideoCaptureConfig {
  width?: number;       // Default 640
  height?: number;      // Default 480
  frameRate?: number;   // Default 30
  cameraPosition?: 'front' | 'back'; // Default 'front'
}

export class VideoCaptureService {
  private context: common.Context | null = null;
  private config: Required<VideoCaptureConfig>;

  // Camera components
  private cameraManager: camera.CameraManager | null = null;
  private cameraInput: camera.CameraInput | null = null;
  private previewOutput: camera.PreviewOutput | null = null;
  private frameCaptureOutput: camera.PreviewOutput | null = null;
  private session: camera.Session | null = null;

  // Frame capture
  private imageReceiver: image.ImageReceiver | null = null;

  // LiveKit video source
  private source: LkVideoSource;

  // State
  private running: boolean = false;
  private initialized: boolean = false;
  private actualWidth: number = 0;
  private actualHeight: number = 0;
  private frameCount: number = 0;
  private lastFrameLogTime: number = 0;

  // Error counters for silent failure detection
  private compFailCount: number = 0;
  private byteBufEmptyCount: number = 0;
  private captureFailCount: number = 0;
  private decodeFailCount: number = 0;

  // Frame throttling — camera outputs ~30fps but encoding handles ~10-15fps
  private lastProcessedTime: number = 0;
  private frameSkipCount: number = 0;

  // Lifecycle recovery: detect foreground changes
  private lastForegroundTick: number = 0;
  private readonly FRAME_INTERVAL_MS: number = 70; // ~14fps

  constructor(width: number = 640, height: number = 480) {
    this.config = {
      width,
      height,
      frameRate: 30,
      cameraPosition: 'front',
    };
    this.source = new LkVideoSource(width, height);
  }

  getSource(): LkVideoSource {
    return this.source;
  }

  isRunning(): boolean {
    return this.running;
  }

  getActualWidth(): number {
    return this.actualWidth;
  }

  getActualHeight(): number {
    return this.actualHeight;
  }

  isInitialized(): boolean {
    return this.initialized;
  }

  /**
   * Initialize the camera hardware.
   *
   * Must be called before start(). Opens the camera, creates the capture
   * session, and starts the camera preview.
   *
   * @param context   UIAbilityContext for CameraManager
   * @param previewSurfaceId  Optional XComponent surface ID for local preview display
   */
  async initialize(context: common.Context, previewSurfaceId?: string): Promise<boolean> {
    // Store for foreground recovery
    globalThis.storedContext = context;
    globalThis.storedPreviewSurfaceId = previewSurfaceId ?? '';
    this.context = context;

    try {
      hilog.info(0x0000, TAG, `Initializing camera, previewSurfaceId=${previewSurfaceId ?? 'none'}`);

      // 1. Get CameraManager
      this.cameraManager = camera.getCameraManager(context);

      // 2. Enumerate cameras
      const cameras: camera.CameraDevice[] = this.cameraManager.getSupportedCameras();
      if (cameras.length === 0) {
        hilog.error(0x0000, TAG, 'No cameras available');
        return false;
      }
      hilog.info(0x0000, TAG, `Found ${cameras.length} cameras`);

      // 3. Select camera by position
      const targetCamera: camera.CameraDevice = this.selectCamera(cameras);
      hilog.info(0x0000, TAG,
        `Selected camera: id=${targetCamera.cameraId}, position=${targetCamera.cameraPosition}`);

      // 4. Determine scene mode and get output capability.
      //    Prefer NORMAL_PHOTO for compatibility — NORMAL_VIDEO causes black
      //    preview on some devices (e.g. Mate X5). Falls back to NORMAL_VIDEO
      //    if the device doesn't support NORMAL_PHOTO.
      const supportedModes: camera.SceneMode[] = this.cameraManager.getSupportedSceneModes(targetCamera);
      let sceneMode: camera.SceneMode = camera.SceneMode.NORMAL_PHOTO;
      if (supportedModes.indexOf(camera.SceneMode.NORMAL_PHOTO) >= 0) {
        sceneMode = camera.SceneMode.NORMAL_PHOTO;
      } else if (supportedModes.indexOf(camera.SceneMode.NORMAL_VIDEO) >= 0) {
        sceneMode = camera.SceneMode.NORMAL_VIDEO;
      } else if (supportedModes.length > 0) {
        sceneMode = supportedModes[0];
      }
      hilog.info(0x0000, TAG, `Scene mode: ${sceneMode}`);

      const capability: camera.CameraOutputCapability =
        this.cameraManager.getSupportedOutputCapability(targetCamera, sceneMode);

      // 5. Select preview profile closest to target resolution
      const previewProfiles: camera.Profile[] = capability.previewProfiles;
      if (previewProfiles.length === 0) {
        hilog.error(0x0000, TAG, 'No preview profiles available');
        return false;
      }
      const previewProfile: camera.Profile = this.selectProfile(previewProfiles);
      this.actualWidth = previewProfile.size.width;
      this.actualHeight = previewProfile.size.height;
      hilog.info(0x0000, TAG, `Preview profile: ${this.actualWidth}x${this.actualHeight}`);

      // 6. Create and open camera input
      this.cameraInput = this.cameraManager.createCameraInput(targetCamera);
      await this.cameraInput.open();
      hilog.info(0x0000, TAG, 'Camera opened');

      // 7. Create local preview output (XComponent surface)
      if (previewSurfaceId !== undefined && previewSurfaceId.length > 0) {
        this.previewOutput = this.cameraManager.createPreviewOutput(previewProfile, previewSurfaceId);
        hilog.info(0x0000, TAG, `Preview output created, surfaceId=${previewSurfaceId}`);
      } else {
        hilog.info(0x0000, TAG, 'No preview surface, skipping local preview');
      }

      // 8. Create ImageReceiver + frame capture output
      await this.setupFrameCapture(previewProfile);

      // 9. Create capture session
      this.session = this.cameraManager.createSession(sceneMode);

      // 10. Configure session: add inputs and outputs
      this.session.beginConfig();
      this.session.addInput(this.cameraInput);

      if (this.previewOutput !== null) {
        this.session.addOutput(this.previewOutput);
        hilog.info(0x0000, TAG, 'Preview output added to session');
      }
      if (this.frameCaptureOutput !== null) {
        this.session.addOutput(this.frameCaptureOutput);
        hilog.info(0x0000, TAG, 'Frame capture output added to session');
      }

      await this.session.commitConfig();
      hilog.info(0x0000, TAG, 'Session configured');

      // 11. Recreate LkVideoSource with actual camera resolution BEFORE starting session.
      //     This ensures frames arriving immediately after session.start() are pushed
      //     to a correctly-sized source, avoiding buffer size mismatches.
      this.source = new LkVideoSource(this.actualWidth, this.actualHeight);
      hilog.info(0x0000, TAG,
        `LkVideoSource recreated with ${this.actualWidth}x${this.actualHeight}`);

      // 12. Set running=true BEFORE session.start() so the ImageReceiver callback
      //     starts processing frames as soon as the camera pipeline produces them.
      //     If we delay this until start(), the ImageReceiver buffer (capacity=2)
      //     fills up and can stall the camera pipeline on some devices (DT50 5G).
      this.running = true;
      hilog.info(0x0000, TAG, 'Frame processing enabled (running=true before session start)');

      // 13. Start session (camera begins producing frames — handled immediately)
      await this.session.start();
      hilog.info(0x0000, TAG, `Camera session started: ${this.actualWidth}x${this.actualHeight}`);

      this.initialized = true;
      return true;
    } catch (error) {
      const err: BusinessError = error as BusinessError;
      hilog.error(0x0000, TAG, `Camera init failed: ${err.message} (code=${err.code})`);
      await this.release();
      return false;
    }
  }

  /**
   * Start video capture (enable frame processing in ImageReceiver callback).
   * Must be called after initialize().
   */
  async start(): Promise<void> {
    if (!this.initialized) {
      hilog.warn(0x0000, TAG, 'Camera not initialized, cannot start');
      return;
    }
    if (this.running) {
      hilog.info(0x0000, TAG,
        'Video capture already running (was started in initialize)');
      return;
    }
    this.running = true;
    this.frameCount = 0;
    hilog.info(0x0000, TAG,
      `Video capture started (${this.actualWidth}x${this.actualHeight})`);
  }

  /**
   * Stop video capture (disable frame processing, stop camera session).
   */
  /**
   * Restart camera after screen-off/on lifecycle event.
   * Releases old resources and re-initializes the entire camera pipeline.
   */
  async restartCamera(): Promise<void> {
    hilog.info(0x0000, TAG, 'Restarting camera after foreground recovery');
    const ctx = globalThis.storedContext as common.Context | undefined;
    const surfaceId = (globalThis.storedPreviewSurfaceId as string) || undefined;
    if (!ctx) {
      hilog.error(0x0000, TAG, 'Cannot restart: no stored context');
      return;
    }
    await this.release();
    this.initialized = false;
    this.running = false;
    await this.initialize(ctx, surfaceId);
  }

  async stop(): Promise<void> {
    this.running = false;
    if (this.session !== null) {
      try {
        await this.session.stop();
        hilog.info(0x0000, TAG, 'Camera session stopped');
      } catch (e) {
        const err = e as BusinessError;
        hilog.warn(0x0000, TAG, `Session stop error: ${err.message}`);
      }
    }
    hilog.info(0x0000, TAG, 'Video capture stopped');
  }

  /**
   * Release all camera resources.
   */
  async release(): Promise<void> {
    this.running = false;
    this.initialized = false;

    if (this.session !== null) {
      try { await this.session.stop(); } catch (_e) { /* ignore */ }
      try { await this.session.release(); } catch (_e) { /* ignore */ }
      this.session = null;
    }
    if (this.cameraInput !== null) {
      try { await this.cameraInput.close(); } catch (_e) { /* ignore */ }
      this.cameraInput = null;
    }
    if (this.previewOutput !== null) {
      try { await this.previewOutput.release(); } catch (_e) { /* ignore */ }
      this.previewOutput = null;
    }
    if (this.frameCaptureOutput !== null) {
      try { await this.frameCaptureOutput.release(); } catch (_e) { /* ignore */ }
      this.frameCaptureOutput = null;
    }
    if (this.imageReceiver !== null) {
      try { await this.imageReceiver.release(); } catch (_e) { /* ignore */ }
      this.imageReceiver = null;
    }
    this.cameraManager = null;
    hilog.info(0x0000, TAG, 'All camera resources released');
  }

  // ================================================================
  // Camera selection & profile
  // ================================================================

  private selectCamera(cameras: camera.CameraDevice[]): camera.CameraDevice {
    const targetPosition: camera.CameraPosition = this.config.cameraPosition === 'front'
      ? camera.CameraPosition.CAMERA_POSITION_FRONT
      : camera.CameraPosition.CAMERA_POSITION_BACK;
    for (let i = 0; i < cameras.length; i++) {
      if (cameras[i].cameraPosition === targetPosition) {
        return cameras[i];
      }
    }
    hilog.warn(0x0000, TAG,
      `No ${this.config.cameraPosition} camera found, using first available`);
    return cameras[0];
  }

  private selectProfile(profiles: camera.Profile[]): camera.Profile {
    let bestProfile: camera.Profile = profiles[0];
    let bestDiff: number = Number.MAX_VALUE;

    for (let i = 0; i < profiles.length; i++) {
      const p: camera.Profile = profiles[i];
      const diff: number =
        Math.abs(p.size.width - this.config.width) +
        Math.abs(p.size.height - this.config.height);
      if (diff < bestDiff) {
        bestDiff = diff;
        bestProfile = p;
      }
    }
    return bestProfile;
  }

  // ================================================================
  // ImageReceiver & frame capture
  // ================================================================

  /**
   * Create ImageReceiver and frame capture PreviewOutput.
   *
   * ImageReceiver uses JPEG format — camera outputs JPEG bitstream.
   * Frames are decoded to RGBA via OHOS createPixelMap, then
   * converted to I420 before being pushed to LkVideoSource.
   */
  private async setupFrameCapture(profile: camera.Profile): Promise<void> {
    if (this.cameraManager === null) return;

    try {
      const receiverSize: image.Size = { width: this.actualWidth, height: this.actualHeight };
      this.imageReceiver = image.createImageReceiver(receiverSize, image.ImageFormat.JPEG, 5);
      hilog.info(0x0000, TAG,
        `ImageReceiver created: ${this.actualWidth}x${this.actualHeight}, format=JPEG`);

      const receiverSurfaceId: string = await this.imageReceiver.getReceivingSurfaceId();
      hilog.info(0x0000, TAG, `ImageReceiver surfaceId=${receiverSurfaceId}`);

      this.frameCaptureOutput = this.cameraManager.createPreviewOutput(profile, receiverSurfaceId);
      hilog.info(0x0000, TAG, 'Frame capture PreviewOutput created');

      this.imageReceiver.on('imageArrival', () => {
        this.handleImageArrival();
      });
      hilog.info(0x0000, TAG, 'ImageReceiver callback registered');
    } catch (error) {
      const err: BusinessError = error as BusinessError;
      hilog.error(0x0000, TAG,
        `Frame capture setup failed: ${err.message} (code=${err.code})`);
      // Frame capture failure is non-fatal — local preview may still work
      this.imageReceiver = null;
      this.frameCaptureOutput = null;
    }
  }

  /**
   * Handle ImageReceiver frame arrival.
   *
   * Flow:
   * 1. readNextImage() to acquire the image
   * 2. getComponent(JPEG) → raw NV21 data (OHOS convention)
   * 3. Push NV21 to LkVideoSource.captureFrameNv21() → I420
   */
  private handleImageArrival(): void {
    // Check for foreground recovery (screen-off → screen-on)
    const currentTick = (globalThis.foregroundTick as number) || 0;
    if (currentTick > this.lastForegroundTick) {
      this.lastForegroundTick = currentTick;
      hilog.info(0x0000, TAG, 'Foreground detected, checking camera...');
      // Schedule camera restart to avoid blocking imageArrival callback
      setTimeout(() => {
        this.restartCamera().catch((e: Error) => {
          hilog.error(0x0000, TAG, 'Camera restart failed: %{public}s', e?.message ?? String(e));
        });
      }, 100);
      return; // Skip processing this frame — camera will restart soon
    }
    if (this.imageReceiver === null || !this.running) {
      if (this.frameCount === 0) {
        hilog.info(0x0000, TAG,
          `imageArrival fired but blocked: receiver=${this.imageReceiver !== null}, running=${this.running}`);
      }
      return;
    }

    this.imageReceiver.readNextImage((readErr: BusinessError, img: image.Image) => {
      if (readErr || img === undefined) {
        if (this.frameCount <= 5) {
          hilog.warn(0x0000, TAG,
            `readNextImage failed: ${readErr?.message ?? 'undefined'}`);
        }
        return;
      }

      this.frameCount++;

      // Frame throttling
      const now: number = Date.now();
      if (now - this.lastProcessedTime < this.FRAME_INTERVAL_MS) {
        this.frameSkipCount++;
        img.release();
        return;
      }
      this.lastProcessedTime = now;

      const timestampUs: number = now * 1000;

      // Periodic stats
      if (now - this.lastFrameLogTime >= 3000) {
        const processed: number = this.frameCount - this.frameSkipCount;
        hilog.info(0x0000, TAG,
          `Frame stats: captured=${this.frameCount}, skipped=${this.frameSkipCount}, processed=${processed}, ` +
          `compFail=${this.compFailCount}, bufEmpty=${this.byteBufEmptyCount}, captureFail=${this.captureFailCount}`);
        this.lastFrameLogTime = now;
      }

      // Get raw NV21 data (OHOS convention: JPEG format → actual NV21 data)
      img.getComponent(image.ComponentType.JPEG, (compErr: BusinessError, imgComponent: image.Component) => {
        if (compErr || imgComponent === undefined) {
          this.compFailCount++;
          if (this.frameCount <= 5 || this.compFailCount % 30 === 1) {
            hilog.warn(0x0000, TAG,
              `Frame #${this.frameCount}: getComponent failed (total=${this.compFailCount}): ${compErr?.message ?? 'undefined'}`);
          }
          img.release();
          return;
        }

        if (!imgComponent.byteBuffer) {
          this.byteBufEmptyCount++;
          if (this.frameCount <= 5 || this.byteBufEmptyCount % 30 === 1) {
            hilog.warn(0x0000, TAG,
              `Frame #${this.frameCount}: byteBuffer empty (total=${this.byteBufEmptyCount})`);
          }
          img.release();
          return;
        }

        const stride: number = imgComponent.rowStride;
        const imgWidth: number = img.size.width;
        const imgHeight: number = img.size.height;

        if (this.frameCount <= 3) {
          const rawLen: number = imgComponent.byteBuffer.byteLength;
          hilog.info(0x0000, TAG,
            `Frame #${this.frameCount}: ${imgWidth}x${imgHeight}, stride=${stride}, bufLen=${rawLen}`);
        }

        // Process asynchronously to avoid blocking the imageArrival callback
        const nv21Buffer: ArrayBuffer = imgComponent.byteBuffer;
        const rotation: number = this.config.cameraPosition === 'front' ? 270 : 90;
          try {
          const rawData: Uint8Array = new Uint8Array(nv21Buffer);
          // Detect RGBA data: stride == width * 4 means 4 bytes per pixel (R,G,B,A).
          // Some OHOS devices output RGBA under ImageFormat.JPEG instead of NV21.
          if (stride === imgWidth * 4) {
            if (this.frameCount <= 3) {
              hilog.info(0x0000, TAG,
                `Frame #${this.frameCount}: RGBA detected (stride=${stride}=${imgWidth}*4), converting to I420`);
            }
            this.source.captureFrameRgba(rawData, imgWidth, imgHeight, timestampUs);
            // RGBA natural orientation differs from NV21; pre-rotate 90° CW
            // so the pipeline's built-in 90° CCW brings it back to upright.
          } else {
            this.source.captureFrameNv21(rawData, imgWidth, imgHeight, stride, timestampUs, rotation);
          }
          } catch (captureErr) {
          this.captureFailCount++;
          let errStr: string = '?';
          try {
            errStr = typeof captureErr === 'object' ? JSON.stringify(captureErr) : String(captureErr);
          } catch (_e2) { /* ignore */ }
          hilog.warn(0x0000, TAG,
            `CAPERR#${this.captureFailCount}: w=${imgWidth} h=${imgHeight} s=${stride} e=${errStr}`);
          } finally {
          img.release();
          }
      });
    });
  }

  /**
   * Convert RGBA → I420 (Y+U+V planar) with BT.601 color matrix.
   *
   * Y  = 0.257*R + 0.504*G + 0.098*B + 16   (full swing)
   * U  = -0.148*R - 0.291*G + 0.439*B + 128
   * V  = 0.439*R - 0.368*G - 0.071*B + 128
   *
   * Chroma is subsampled 4:2:0 (2×2 pixel blocks → 1 U,V sample).
   */
  private rgbaToI420(rgba: Uint8Array, width: number, height: number): Uint8Array {
    const w: number = width;
    const h: number = height;
    const cw: number = (w + 1) >> 1; // chroma width
    const ch: number = (h + 1) >> 1; // chroma height
    const ySize: number = w * h;
    const uSize: number = cw * ch;
    const dst: Uint8Array = new Uint8Array(ySize + uSize * 2);

    for (let row: number = 0; row < h; row++) {
      const isChromaRow: boolean = (row & 1) === 0;
      for (let col: number = 0; col < w; col++) {
        const rgbaOff: number = (row * w + col) << 2; // * 4
        const r: number = rgba[rgbaOff];
        const g: number = rgba[rgbaOff + 1];
        const b: number = rgba[rgbaOff + 2];

        // Y (luma): BT.601, full swing
        let yy: number = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
        if (yy > 255) { yy = 255; } else if (yy < 0) { yy = 0; }
        dst[row * w + col] = yy;

        // U, V (chroma): subsample 4:2:0 — use top-left pixel of each 2×2 block
        if (isChromaRow && (col & 1) === 0) {
          let u: number = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
          let v: number = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
          if (u > 255) { u = 255; } else if (u < 0) { u = 0; }
          if (v > 255) { v = 255; } else if (v < 0) { v = 0; }

          const uvOff: number = ySize + (row >> 1) * cw + (col >> 1);
          dst[uvOff] = u;
          dst[uvOff + uSize] = v;
        }
      }
    }

    return dst;
  }

  /**
   * Rotate I420 (Y+U+V planar) 90° clockwise.
   *
   * Input W×H becomes output H×W. Chroma planes are also rotated.
   * This is needed when the camera's RGBA natural orientation differs
   * from NV21, and the pipeline's fixed 90° CCW rotation must be compensated.
   */
  private rotateI42090CW(i420: Uint8Array, width: number, height: number): Uint8Array {
    const w: number = width;
    const h: number = height;
    const halfW: number = (w + 1) >> 1;
    const halfH: number = (h + 1) >> 1;
    const newW: number = h;      // output width = source height
    const newH: number = w;      // output height = source width
    const newHalfW: number = halfH;
    const newHalfH: number = halfW;

    const ySize: number = w * h;
    const uSize: number = halfW * halfH;
    const dst: Uint8Array = new Uint8Array(i420.length);

    // Rotate Y plane 90° CW: output[y'][x'] = source[(h-1-x') * w + y']
    for (let dstRow: number = 0; dstRow < newH; dstRow++) {
      for (let dstCol: number = 0; dstCol < newW; dstCol++) {
        const srcRow: number = h - 1 - dstCol;
        const srcCol: number = dstRow;
        dst[dstRow * newW + dstCol] = i420[srcRow * w + srcCol];
      }
    }

    // Rotate U plane 90° CW
    const uOff: number = ySize;
    const uDstOff: number = newW * newH;
    for (let dstRow: number = 0; dstRow < newHalfH; dstRow++) {
      for (let dstCol: number = 0; dstCol < newHalfW; dstCol++) {
        const srcRow: number = halfH - 1 - dstCol;
        const srcCol: number = dstRow;
        dst[uDstOff + dstRow * newHalfW + dstCol] = i420[uOff + srcRow * halfW + srcCol];
      }
    }

    // Rotate V plane 90° CW
    const vOff: number = ySize + uSize;
    const vDstOff: number = newW * newH + newHalfW * newHalfH;
    for (let dstRow: number = 0; dstRow < newHalfH; dstRow++) {
      for (let dstCol: number = 0; dstCol < newHalfW; dstCol++) {
        const srcRow: number = halfH - 1 - dstCol;
        const srcCol: number = dstRow;
        dst[vDstOff + dstRow * newHalfW + dstCol] = i420[vOff + srcRow * halfW + srcCol];
      }
    }

    return dst;
  }

}
