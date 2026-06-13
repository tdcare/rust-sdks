import { AbilityConstant, UIAbility, Want } from '@kit.AbilityKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { window } from '@kit.ArkUI';
import { ApiClient } from '../data/api/ApiClient';
import { TokenStore } from '../data/security/TokenStore';
import { SessionManager } from '../data/session/SessionManager';

const TAG = 'EntryAbility';

export default class EntryAbility extends UIAbility {
  onCreate(want: Want, launchParam: AbilityConstant.LaunchParam): void {
    hilog.info(0x0000, TAG, 'onCreate');

    // 初始化数据层
    const ctx = this.context;
    ApiClient.init(ctx);
    TokenStore.init(ctx);
    SessionManager.init(ctx);
  }

  onDestroy(): void {
    hilog.info(0x0000, TAG, 'onDestroy');
  }

  onWindowStageCreate(windowStage: window.WindowStage): void {
    hilog.info(0x0000, TAG, 'onWindowStageCreate');
    windowStage.loadContent('pages/LoginPage', (err) => {
      if (err.code) {
        hilog.error(0x0000, TAG, 'Failed to load content: %{public}s', JSON.stringify(err));
        return;
      }
      hilog.info(0x0000, TAG, 'Content loaded successfully');
    });
  }

  onNewWant(want: Want, launchParam: AbilityConstant.LaunchParam): void {
    // Handle system re-dispatching Want (e.g. permission settings)
    hilog.info(0x0000, TAG, 'onNewWant want.uri=%{public}s want.abilityName=%{public}s',
      want.uri ?? '', want.abilityName ?? '');
    if (want.parameters && typeof want.parameters === 'object') {
      const keys = Object.keys(want.parameters).slice(0, 10);
      hilog.info(0x0000, TAG, 'onNewWant want.parameters keys: %{public}s', keys.join(','));
    }
  }

  onWindowStageDestroy(): void {}
    onForeground(): void {
    hilog.info(0x0000, TAG, 'onForeground');
    globalThis.foregroundTick = Date.now();
    const vc = globalThis.videoCaptureService as Record<string, Function> | undefined;
    if (vc && vc['restartCamera']) {
      setTimeout(() => {
        (vc['restartCamera'] as () => Promise<void>)().catch((e: Error) => {
          hilog.error(0x0000, TAG, 'Camera restart failed: %{public}s', e?.message ?? '');
        });
      }, 200);
    }
  }
  onBackground(): void { hilog.info(0x0000, TAG, "onBackground"); }
}
