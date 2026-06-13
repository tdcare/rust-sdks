import { preferences } from '@kit.ArkData';
import { ApiClient } from '../api/ApiClient';

/**
 * 会话管理 — 持久化登录状态和用户信息。
 * OHOS 使用 preferences 替代 Android SharedPreferences。
 */
export class SessionManager {
  private static readonly PREF_NAME = 'rrtphone_session';
  private static readonly KEY_USER_NAME = 'user_name';
  private static readonly KEY_DEPARTMENT_ID = 'department_id';
  private static readonly KEY_DEPARTMENT_NAME = 'department_name';
  private static readonly KEY_USER_ID = 'user_id';

  private static prefs: preferences.Preferences | null = null;
  private static context: Context | null = null;

  static async init(ctx: Context): Promise<void> {
    SessionManager.context = ctx;
    SessionManager.prefs = await preferences.getPreferences(ctx, SessionManager.PREF_NAME);
  }

  private static ensurePrefs(): preferences.Preferences {
    if (!SessionManager.prefs) {
      throw new Error('SessionManager 未初始化，请先调用 init()');
    }
    return SessionManager.prefs;
  }

  static async getUserName(): Promise<string> {
    const p = SessionManager.ensurePrefs();
    return (await p.get(SessionManager.KEY_USER_NAME, '')) as string;
  }

  static async setUserName(value: string): Promise<void> {
    const p = SessionManager.ensurePrefs();
    await p.put(SessionManager.KEY_USER_NAME, value);
    await p.flush();
  }

  static async getUserId(): Promise<string> {
    const p = SessionManager.ensurePrefs();
    return (await p.get(SessionManager.KEY_USER_ID, '')) as string;
  }

  static async setUserId(value: string): Promise<void> {
    const p = SessionManager.ensurePrefs();
    await p.put(SessionManager.KEY_USER_ID, value);
    await p.flush();
  }

  static async getDepartmentId(): Promise<string> {
    const p = SessionManager.ensurePrefs();
    return (await p.get(SessionManager.KEY_DEPARTMENT_ID, '')) as string;
  }

  static async setDepartmentId(value: string): Promise<void> {
    const p = SessionManager.ensurePrefs();
    await p.put(SessionManager.KEY_DEPARTMENT_ID, value);
    await p.flush();
  }

  static async getDepartmentName(): Promise<string> {
    const p = SessionManager.ensurePrefs();
    return (await p.get(SessionManager.KEY_DEPARTMENT_NAME, '')) as string;
  }

  static async setDepartmentName(value: string): Promise<void> {
    const p = SessionManager.ensurePrefs();
    await p.put(SessionManager.KEY_DEPARTMENT_NAME, value);
    await p.flush();
  }

  static get isLoggedIn(): boolean {
    return ApiClient.isLoggedIn;
  }

  static async logout(): Promise<void> {
    ApiClient.clearToken();
    ApiClient.rebuildApi();
    const p = SessionManager.ensurePrefs();
    await p.clear();
    await p.flush();
  }
}
