import { abilityAccessCtrl, common, Permissions } from '@kit.AbilityKit';
import { hilog } from '@kit.PerformanceAnalysisKit';

const TAG = 'PermissionHelper';

export async function requestPermissions(context: common.UIAbilityContext): Promise<boolean> {
  const permissions: Array<Permissions> = [
    'ohos.permission.MICROPHONE',
    'ohos.permission.CAMERA'
  ];

  const atManager = abilityAccessCtrl.createAtManager();

  // Check if all permissions are already granted — if so, skip
  // the dialog to avoid OHOS redirecting to Settings on re-entry.
  try {
    let allPreGranted = true;
    for (const perm of permissions) {
      const grantStatus = atManager.checkAccessTokenSync(
        context.applicationInfo.accessTokenId, perm
      );
      if (grantStatus !== abilityAccessCtrl.GrantStatus.PERMISSION_GRANTED) {
        allPreGranted = false;
        break;
      }
    }
    if (allPreGranted) {
      hilog.info(0x0000, TAG, 'Permissions already granted, skipping dialog');
      return true;
    }
  } catch (e) {
    // checkAccessTokenSync may throw in some sandbox setups; fall through
    hilog.warn(0x0000, TAG, 'Pre-check failed, falling back to dialog: %{public}s',
      JSON.stringify(e));
  }

  try {
    const result = await atManager.requestPermissionsFromUser(context, permissions);
    const allGranted = result.authResults.every((r: number) => r === 0);
    if (!allGranted) {
      hilog.warn(0x0000, TAG, 'Some permissions were denied');
    }
    return allGranted;
  } catch (err) {
    hilog.error(0x0000, TAG, 'Permission request failed: %{public}s', JSON.stringify(err));
    return false;
  }
}
