import { preferences } from '@kit.ArkData';

/**
 * Token 安全存储。
 * OHOS 中无 EncryptedSharedPreferences 等价物，使用应用沙箱内的 preferences 存储。
 * Token 存储在应用私有目录下，依赖系统沙箱保护数据安全。
 */
export class TokenStore {
  private static readonly PREF_NAME = 'rrtphone_token_store';
  private static readonly KEY_ACCESS_TOKEN = 'access_token';
  private static readonly KEY_EXPIRES_AT = 'token_expires_at';
  private static readonly KEY_USERNAME = 'username';
  private static readonly KEY_DISPLAY_NAME = 'display_name';

  private static prefs: preferences.Preferences | null = null;

  static async init(ctx: Context): Promise<void> {
    TokenStore.prefs = await preferences.getPreferences(ctx, TokenStore.PREF_NAME);
  }

  private static ensurePrefs(): preferences.Preferences {
    if (!TokenStore.prefs) {
      throw new Error('TokenStore 未初始化，请先调用 init()');
    }
    return TokenStore.prefs;
  }

  static async save(accessToken: string, expiresAtMs: number,
    username: string, displayName: string): Promise<void> {
    const p = TokenStore.ensurePrefs();
    await p.put(TokenStore.KEY_ACCESS_TOKEN, accessToken);
    await p.put(TokenStore.KEY_EXPIRES_AT, expiresAtMs);
    await p.put(TokenStore.KEY_USERNAME, username);
    await p.put(TokenStore.KEY_DISPLAY_NAME, displayName);
    await p.flush();
  }

  static async saveToken(accessToken: string): Promise<void> {
    const p = TokenStore.ensurePrefs();
    await p.put(TokenStore.KEY_ACCESS_TOKEN, accessToken);
    await p.flush();
  }

  static async saveExpiresAt(expiresAtMs: number): Promise<void> {
    const p = TokenStore.ensurePrefs();
    await p.put(TokenStore.KEY_EXPIRES_AT, expiresAtMs);
    await p.flush();
  }

  static async getAccessToken(): Promise<string> {
    const p = TokenStore.ensurePrefs();
    return (await p.get(TokenStore.KEY_ACCESS_TOKEN, '')) as string;
  }

  static async getExpiresAt(): Promise<number> {
    const p = TokenStore.ensurePrefs();
    return (await p.get(TokenStore.KEY_EXPIRES_AT, 0)) as number;
  }

  static async getUsername(): Promise<string> {
    const p = TokenStore.ensurePrefs();
    return (await p.get(TokenStore.KEY_USERNAME, '')) as string;
  }

  static async getDisplayName(): Promise<string> {
    const p = TokenStore.ensurePrefs();
    return (await p.get(TokenStore.KEY_DISPLAY_NAME, '')) as string;
  }

  static async isLoggedIn(): Promise<boolean> {
    const token = await TokenStore.getAccessToken();
    return token.length > 0;
  }

  static async clear(): Promise<void> {
    const p = TokenStore.ensurePrefs();
    await p.clear();
    await p.flush();
  }
}
