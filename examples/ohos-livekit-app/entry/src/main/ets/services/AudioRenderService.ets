import { audio } from '@kit.AudioKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { LkAudioStream, LkRemoteAudioTrack } from 'liblivekit.so';

const TAG = 'AudioRender';

export class AudioRenderService {
  private renderer: audio.AudioRenderer | null = null;
  private stream: LkAudioStream | null = null;
  private running: boolean = false;

  async start(track: LkRemoteAudioTrack): Promise<void> {
    if (this.running) return;
    // Set running flag BEFORE any await to prevent re-entrancy race.
    // Without this, a second call can slip in during createAudioRenderer.
    this.running = true;

    try {
      this.stream = LkAudioStream.fromTrack(track);

      const rendererOptions: audio.AudioRendererOptions = {
        streamInfo: {
          samplingRate: audio.AudioSamplingRate.SAMPLE_RATE_48000,
          channels: audio.AudioChannel.CHANNEL_1,
          sampleFormat: audio.AudioSampleFormat.SAMPLE_FORMAT_S16LE,
          encodingType: audio.AudioEncodingType.ENCODING_TYPE_RAW
        },
        rendererInfo: {
          content: audio.ContentType.CONTENT_TYPE_SPEECH,
          usage: audio.StreamUsage.STREAM_USAGE_VOICE_COMMUNICATION,
          rendererFlags: 0
        }
      };

      this.renderer = await audio.createAudioRenderer(rendererOptions);
      await this.renderer.start();
      hilog.info(0x0000, TAG, 'Audio renderer started');
      this.writeLoop();
    } catch (err) {
      this.running = false;  // reset on failure so retry is possible
      hilog.error(0x0000, TAG, 'Failed to start audio renderer: %{public}s', JSON.stringify(err));
      throw new Error(`Failed to start audio renderer: ${JSON.stringify(err)}`);
    }
  }

  private async writeLoop(): Promise<void> {
    let writeCount = 0;
    let writeBytes = 0;
    while (this.running && this.stream && this.renderer) {
      try {
        const frame: ESObject = await this.stream.nextFrame();
        if (!this.running) break;
        if (!frame) {
          hilog.info(0x0000, TAG, 'Audio stream ended');
          break;
        }
        // Copy PCM data into a clean ArrayBuffer matching the exact byte length.
        // napi-ohos Buffer cast to Uint8Array may not have a properly sized
        // backing ArrayBuffer; explicitly copy into a fresh one.
        const data: Uint8Array = new Uint8Array(frame.data as ArrayBuffer);
        const bufLen = data.byteLength;
        const buf = new ArrayBuffer(bufLen);
        const dst = new Uint8Array(buf);
        dst.set(data);

        // Guard against renderer being replaced / released mid-write.
        const renderer = this.renderer;
        if (!this.running || !renderer) break;

        await renderer.write(buf);
        writeCount++;
        writeBytes += bufLen;
        if (writeCount === 1 || writeCount % 200 === 0) {
          const headBytes = dst.slice(0, 8);
          const headStr = Array.from(headBytes).map(b => b.toString(16).padStart(2, '0')).join(',');
          hilog.info(0x0000, TAG, 'writeLoop: #%{public}d, %{public}d bytes, head=[%{public}s], total=%{public}d bytes',
            writeCount, bufLen, headStr, writeBytes);
        }
      } catch (err) {
        if (this.running) {
          hilog.error(0x0000, TAG, 'Write error (cnt=%{public}d): %{public}s', writeCount, JSON.stringify(err));
        }
        break;
      }
    }
  }

  async stop(): Promise<void> {
    this.running = false;
    if (this.stream) {
      this.stream.close();
      this.stream = null;
    }
    if (this.renderer) {
      try {
        await this.renderer.stop();
        await this.renderer.release();
      } catch (err) {
        hilog.warn(0x0000, TAG, 'Renderer stop error: %{public}s', JSON.stringify(err));
      }
      this.renderer = null;
    }
    hilog.info(0x0000, TAG, 'Audio renderer stopped');
  }
}
