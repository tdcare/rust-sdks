import { hilog } from '@kit.PerformanceAnalysisKit';
import { LkVideoStream, LkRemoteVideoTrack } from 'liblivekit.so';

const TAG = 'VideoRender';

/**
 * High-performance video render service using NativeWindow Surface.
 *
 * Instead of converting I420 to RGBA in JS and creating PixelMaps,
 * this service passes a Surface ID to the native layer which writes
 * decoded frames directly to the NativeWindow buffer via OHOS NDK APIs.
 *
 * Usage:
 * 1. Call `setSurfaceId(id, w, h)` once the XComponent surface is ready.
 * 2. Call `start(track)` when a remote video track is subscribed.
 * 3. The render loop calls `renderToSurface()` in a tight async loop.
 *
 * Return value semantics of renderToSurface():
 *  1 = frame rendered successfully
 *  0 = stream ended (no more frames) → stop loop
 * -1 = frame received but not rendered (no surface yet) → continue loop
 */
export class VideoRenderService {
  private stream: LkVideoStream | null = null;
  private renderLoopVersion: number = 0; // incremented on each start/stop to kill stale loops
  private surfaceId: string = '';
  private surfaceWidth: number = 1280;
  private surfaceHeight: number = 720;
  private pendingTrack: LkRemoteVideoTrack | null = null;

  // Diagnostics
  private frameCount: number = 0;
  private skipCount: number = 0;
  private errorCount: number = 0;
  private lastDiagTimeMs: number = 0;

  // Callback invoked when the render loop ends so that callers can
  // clear their track-tracking state (e.g. renderingVideoTrackSid).
  private onRenderStoppedCallback: (() => void) | null = null;

  setOnRenderStopped(cb: () => void): void {
    this.onRenderStoppedCallback = cb;
  }

  /**
   * Returns true if a render loop is currently active.
   */
  private isRunning(): boolean {
    return this.renderLoopVersion > 0;
  }

  /**
   * Set the XComponent surface ID for direct native rendering.
   * Can be called before or after `start()`.
   */
  setSurfaceId(surfaceId: string, width: number, height: number): void {
    this.surfaceId = surfaceId;
    this.surfaceWidth = width;
    this.surfaceHeight = height;
    hilog.info(0x0000, TAG, 'Surface ID set: %{public}s (%{public}dx%{public}d)',
      surfaceId, width, height);

    // If stream already exists, bind the surface immediately
    if (this.stream) {
      const ok: boolean = this.stream.setSurface(surfaceId, width, height);
      hilog.info(0x0000, TAG, 'Surface bound to existing stream: %{public}s', ok ? 'OK' : 'FAILED');
    }

    // If we have a pending track and the loop hasn't started, start it now
    if (this.pendingTrack && !this.isRunning()) {
      hilog.info(0x0000, TAG, 'Surface arrived, starting deferred render for pending track');
      const track: LkRemoteVideoTrack = this.pendingTrack;
      this.pendingTrack = null;
      this.start(track).catch((err: Object) => {
        hilog.error(0x0000, TAG, 'Deferred start failed: %{public}s', String(err));
      });
    }
  }

  async start(track: LkRemoteVideoTrack): Promise<void> {
    // Bump version to kill any stale render loop BEFORE creating new stream.
    // This ensures old loops see their version is stale and exit, rather than
    // stealing the new stream when `this.stream` is reassigned.
    if (this.isRunning()) {
      hilog.info(0x0000, TAG, 'Already running — stopping old stream before restart');
      this.renderLoopVersion++; // kill old loop
      if (this.stream) {
        this.stream.close();
        this.stream = null;
      }
      // Yield to allow the old loop's pending `await` to observe the version change
      await new Promise<void>((resolve) => { setTimeout(() => { resolve(); }, 10); });
    }

    // If surface is not yet available, defer start until surface arrives
    if (this.surfaceId === '') {
      hilog.info(0x0000, TAG, 'Surface not ready yet, deferring render start');
      this.pendingTrack = track;
      return;
    }

    try {
      this.stream = LkVideoStream.fromTrack(track);
      this.renderLoopVersion++; // assign new version for the new loop
      this.frameCount = 0;
      this.skipCount = 0;
      this.errorCount = 0;
      this.lastDiagTimeMs = Date.now();

      // Bind surface
      if (this.surfaceId !== '' && this.stream) {
        const ok: boolean = this.stream.setSurface(this.surfaceId, this.surfaceWidth, this.surfaceHeight);
        hilog.info(0x0000, TAG, 'Surface bound on start: %{public}s', ok ? 'OK' : 'FAILED');
      }

      hilog.info(0x0000, TAG,
        'Video render started (Surface mode), version=%{public}d', this.renderLoopVersion);
      this.renderLoop(this.renderLoopVersion);
    } catch (err) {
      this.renderLoopVersion = 0;
      hilog.error(0x0000, TAG, 'Failed to start video render: %{public}s', JSON.stringify(err));
      throw new Error(`Failed to start video render: ${JSON.stringify(err)}`);
    }
  }

  private async renderLoop(version: number): Promise<void> {
    let consecutiveDrains: number = 0;
    const MAX_DRAIN_RETRIES: number = 60; // ~2s at 30fps

    while (this.renderLoopVersion === version && this.stream) {
      // Check version again after each await — start() may have bumped it
      try {
        // renderToSurface() returns:
        //  1 = frame rendered OK
        //  0 = stream ended (no more frames)
        // -1 = frame received but not rendered (no surface / format issue)
        const result: number = await this.stream.renderToSurface();

        // Re-check version: if start() was called while we were awaiting,
        // our version is stale and we must exit immediately.
        if (this.renderLoopVersion !== version) {
          hilog.info(0x0000, TAG,
            'Render loop v%{public}d superseded by v%{public}d, exiting',
            version, this.renderLoopVersion);
          break;
        }

        if (result === 0) {
          consecutiveDrains++;
          // During ICE reconnections, the native parser may briefly report
          // "stream ended" even though frames will resume shortly. Give the
          // stream a grace period before truly declaring it dead.
          if (consecutiveDrains <= MAX_DRAIN_RETRIES) {
            if (consecutiveDrains <= 3 || consecutiveDrains % 10 === 0) {
              hilog.info(0x0000, TAG,
                'Drain mode: %{public}d/%{public}d consecutive zeros, waiting for recovery...',
                consecutiveDrains, MAX_DRAIN_RETRIES);
            }
            // Brief delay to avoid a tight spin loop
            await new Promise<void>((resolve) => { setTimeout(() => { resolve(); }, 33); });
            continue;
          }
          // Grace period exhausted — stream truly ended
          if (this.renderLoopVersion === version) {
            hilog.info(0x0000, TAG, 'Stream ended (renderToSurface returned 0, drain exhausted)');
            // Notify caller so it can clear its SID tracking
            if (this.onRenderStoppedCallback) {
              this.onRenderStoppedCallback();
            }
          }
          break;
        } else if (result === 1) {
          // Frame rendered successfully — reset drain counter
          if (consecutiveDrains > 0) {
            hilog.info(0x0000, TAG, 'Stream recovered after %{public}d drains', consecutiveDrains);
          }
          consecutiveDrains = 0;
          this.frameCount++;
        } else {
          // result === -1: frame received but not rendered (no surface yet)
          consecutiveDrains = 0; // -1 is not a drain
          this.skipCount++;
          if (this.skipCount <= 5 || this.skipCount % 30 === 0) {
            hilog.info(0x0000, TAG,
              'Frame skipped (no surface or format issue), skipCount=%{public}d',
              this.skipCount);
          }
        }

        this.logDiagnostics();
      } catch (err) {
        this.errorCount++;
        if (this.renderLoopVersion === version) {
          hilog.error(0x0000, TAG, 'Render loop error: %{public}s',
            err instanceof Error ? err.message : JSON.stringify(err));
          if (this.errorCount > 50) {
            hilog.error(0x0000, TAG, 'Too many errors, stopping render loop');
            break;
          }
        } else {
          break;
        }
      }
    }
    hilog.info(0x0000, TAG,
      'Render loop ended: rendered=%{public}d, skipped=%{public}d, errors=%{public}d',
      this.frameCount, this.skipCount, this.errorCount);
    // If the loop ended on its own (not superseded by a newer version),
    // notify the caller so it can clear its track-tracking state.
    if (this.renderLoopVersion === version && this.onRenderStoppedCallback) {
      this.onRenderStoppedCallback();
    }
  }

  /** Log diagnostics every 3 seconds. */
  private logDiagnostics(): void {
    const now: number = Date.now();
    if (now - this.lastDiagTimeMs >= 3000) {
      const elapsed: number = (now - this.lastDiagTimeMs) / 1000;
      const fps: number = elapsed > 0 ? this.frameCount / elapsed : 0;
      hilog.info(0x0000, TAG,
        'Diag: rendered=%{public}d, skipped=%{public}d, errors=%{public}d, ~%{public}d fps',
        this.frameCount, this.skipCount, this.errorCount, Math.round(fps));
      // Reset for next interval
      this.frameCount = 0;
      this.skipCount = 0;
      this.lastDiagTimeMs = now;
    }
  }

  async stop(): Promise<void> {
    this.renderLoopVersion = 0; // kill all loops
    this.pendingTrack = null;
    if (this.stream) {
      this.stream.close();
      this.stream = null;
    }
    hilog.info(0x0000, TAG, 'Video render stopped');
  }
}
