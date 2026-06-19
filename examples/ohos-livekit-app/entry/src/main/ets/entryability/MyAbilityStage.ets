import { AbilityStage, Want } from '@kit.AbilityKit';
import { hilog } from '@kit.PerformanceAnalysisKit';

const TAG = 'MyAbilityStage';

export default class MyAbilityStage extends AbilityStage {
  onCreate(): void {
    hilog.info(0x0000, TAG, 'AbilityStage onCreate');
  }

  onAcceptWant(want: Want): string {
    hilog.info(0x0000, TAG, 'onAcceptWant: uri=%{public}s abilityName=%{public}s',
      want.uri ?? '', want.abilityName ?? '');
    // Force-launch EntryAbility even if the system wants to show
    // AppDetailAbility (permission settings redirect).
    if (!want.abilityName || want.abilityName === 'AppDetailAbility') {
      hilog.warn(0x0000, TAG, 'Redirecting AppDetailAbility → EntryAbility');
      return 'EntryAbility';
    }
    return want.abilityName ?? 'EntryAbility';
  }
}
