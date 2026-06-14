import { audio } from '@kit.AudioKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { LkAudioSource } from 'liblivekit.so';

const TAG = 'AudioCapture';
const SAMPLE_RATE = 48000;
const CHANNELS = 1;
const FRAME_SIZE_MS = 10;
const SAMPLES_PER_FRAME = SAMPLE_RATE * FRAME_SIZE_MS / 1000; // 480
const BUFFER_SIZE = SAMPLES_PER_FRAME * CHANNELS * 2; // 960 bytes (i16)

export class AudioCaptureService {
  private capturer: audio.AudioCapturer | null = null;
  private source: LkAudioSource;
  private running: boolean = false;

  constructor() {
    this.source = new LkAudioSource(SAMPLE_RATE, CHANNELS);
  }

  getSource(): LkAudioSource {
    return this.source;
  }

  isRunning(): boolean {
    return this.running;
  }

  async start(): Promise<void> {
    hilog.info(0x0000, TAG, 'start() called, running=%{public}s', String(this.running));
    if (this.running) {
      hilog.warn(0x0000, TAG, 'start() skipped: already running');
      return;
    }

    const capturerOptions: audio.AudioCapturerOptions = {
      streamInfo: {
        samplingRate: audio.AudioSamplingRate.SAMPLE_RATE_48000,
        channels: audio.AudioChannel.CHANNEL_1,
        sampleFormat: audio.AudioSampleFormat.SAMPLE_FORMAT_S16LE,
        encodingType: audio.AudioEncodingType.ENCODING_TYPE_RAW
      },
      capturerInfo: {
        source: audio.SourceType.SOURCE_TYPE_MIC,
        capturerFlags: 0
      }
    };

    try {
      hilog.info(0x0000, TAG, 'Creating AudioCapturer...');
      this.capturer = await audio.createAudioCapturer(capturerOptions);
      hilog.info(0x0000, TAG, 'AudioCapturer created, starting...');
      await this.capturer.start();
      this.running = true;
      hilog.info(0x0000, TAG, 'Audio capture started, starting readLoop...');
      this.readLoop().catch((err: Error) => {
        hilog.error(0x0000, TAG, 'readLoop crashed: %{public}s', err?.message ?? JSON.stringify(err));
      });
    } catch (err) {
      hilog.error(0x0000, TAG, 'Failed to start audio capture: %{public}s', JSON.stringify(err));
      throw new Error(`Failed to start audio capture: ${JSON.stringify(err)}`);
    }
  }

  private async readLoop(): Promise<void> {
    let consecutiveErrors = 0;
    let frameCount = 0;
    let emptyCount = 0;
    let recreateCount = 0;
    hilog.info(0x0000, TAG, 'readLoop entered (non-blocking + 3s timeout, buffer=%{public}d bytes)', BUFFER_SIZE);
    while (this.running) {
      // Recreate capturer if we don't have one or after too many errors.
      if (!this.capturer) {
        const ok = await this.tryCreateCapturerUnsafe();
        if (!ok) {
          hilog.error(0x0000, TAG, 'readLoop: capturer recreate failed, sleeping 2s...');
          await new Promise<void>((r) => setTimeout(r, 2000));
          continue;
        }
        recreateCount++;
        consecutiveErrors = 0;
        hilog.info(0x0000, TAG, 'readLoop: capturer (re)created (#%{public}d)', recreateCount);
      }

      try {
        // Non-blocking read: returns immediately with available data
        // or empty buffer. Tight poll loop ensures we catch mic data
        // as soon as it becomes available.
        const capturer = this.capturer!;
                const rawBuffer: ArrayBuffer = await Promise.race([
          capturer.read(BUFFER_SIZE, false),
          new Promise<ArrayBuffer>((resolve) => setTimeout(() => resolve(new ArrayBuffer(0)), 3000))
        ]);
        if (rawBuffer && rawBuffer.byteLength > 0) {
          const buffer: ArrayBuffer = rawBuffer.slice(0);
          const actualSamples = Math.floor(buffer.byteLength / (CHANNELS * 2));
          await this.source.captureFrame(new Uint8Array(buffer), SAMPLE_RATE, CHANNELS, actualSamples);
          consecutiveErrors = 0;
          emptyCount = 0;
          frameCount++;
          // Log first frame + every 100 frames for diagnostics
          if (frameCount === 1 || frameCount % 100 === 0) {
            const bytes = new Uint8Array(buffer);
            const headStr = Array.from(bytes.slice(0, 8)).map(b => b.toString(16).padStart(2, '0')).join(',');
            hilog.info(0x0000, TAG, 'Audio frame #%{public}d: %{public}d bytes, %{public}d samples, head=[%{public}s]',
              frameCount, buffer.byteLength, actualSamples, headStr);
          }
        } else {
          // No data ready yet — tight poll to catch next frame quickly.
          emptyCount++;
          if (emptyCount === 1 || emptyCount % 500 === 0) {
            hilog.info(0x0000, TAG, 'readLoop: no data ready (empty count=%{public}d)', emptyCount);
          }
          // 1ms yield: tight enough for 10ms audio frames.
          await new Promise<void>((r) => setTimeout(r, 1));
        }
      } catch (err) {
        if (!this.running) break;
        consecutiveErrors++;
        if (consecutiveErrors <= 3 || consecutiveErrors % 100 === 0) {
          hilog.error(0x0000, TAG, 'Capture error (#%{public}d): %{public}s',
            consecutiveErrors, JSON.stringify(err));
        }
        // After 10 consecutive errors, release the broken capturer and retry.
        if (consecutiveErrors >= 10) {
          hilog.warn(0x0000, TAG, 'readLoop: %{public}d errors, releasing capturer for retry...', consecutiveErrors);
          await this.releaseCapturerUnsafe();
          consecutiveErrors = 0;
          await new Promise<void>((r) => setTimeout(r, 500));
          // Loop will recreate via the top check.
        } else {
          await new Promise<void>((r) => setTimeout(r, 20));
        }
      }
    }
  }

  private async tryCreateCapturerUnsafe(): Promise<boolean> {
    try {
      const capturerOptions: audio.AudioCapturerOptions = {
        streamInfo: {
          samplingRate: audio.AudioSamplingRate.SAMPLE_RATE_48000,
          channels: audio.AudioChannel.CHANNEL_1,
          sampleFormat: audio.AudioSampleFormat.SAMPLE_FORMAT_S16LE,
          encodingType: audio.AudioEncodingType.ENCODING_TYPE_RAW
        },
        capturerInfo: {
          source: audio.SourceType.SOURCE_TYPE_MIC,
          capturerFlags: 0
        }
      };
      this.capturer = await audio.createAudioCapturer(capturerOptions);
      await this.capturer.start();
      return true;
    } catch (err) {
      hilog.error(0x0000, TAG, 'tryCreateCapturer failed: %{public}s', JSON.stringify(err));
      return false;
    }
  }

  private async releaseCapturerUnsafe(): Promise<void> {
    if (!this.capturer) return;
    try {
      await this.capturer.stop();
      await this.capturer.release();
    } catch (err) {
      hilog.warn(0x0000, TAG, 'releaseCapturer error: %{public}s', JSON.stringify(err));
    }
    this.capturer = null;
  }

  async stop(): Promise<void> {
    this.running = false;
    if (this.capturer) {
      try {
        await this.capturer.stop();
        await this.capturer.release();
      } catch (err) {
        hilog.warn(0x0000, TAG, 'Stop error: %{public}s', JSON.stringify(err));
      }
      this.capturer = null;
    }
    hilog.info(0x0000, TAG, 'Audio capture stopped');
  }
}
