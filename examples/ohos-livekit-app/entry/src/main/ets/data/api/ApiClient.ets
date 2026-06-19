import { http } from '@kit.NetworkKit';
import { preferences } from '@kit.ArkData';
import { TokenStore } from '../security/TokenStore';
import { TidengApi } from './TidengApi';

/**
 * HTTP 客户端单例 — OHOS 使用 @ohos.net.http 替代 Retrofit/OkHttp。
 * 负责 Token 管理、Authorization 头注入、401 拦截跳转。
 */
export class ApiClient {
  private static readonly PREF_NAME = 'rrtphone_api';
  private static readonly KEY_BASE_URL = 'base_url';
  private static readonly KEY_TOKEN = 'token';
  private static readonly DEFAULT_BASE_URL = 'https://tdtest.tdcare.cn:6115/';

  private static prefs: preferences.Preferences | null = null;
  private static api: TidengApi | null = null;

  static token: string = '';

  /** 401 未授权回调，由 HomePage 注册 */
  static onUnauthorized: (() => void) | null = null;

  static async init(ctx: Context): Promise<void> {
    ApiClient.prefs = await preferences.getPreferences(ctx, ApiClient.PREF_NAME);
    ApiClient.token = (await ApiClient.prefs.get(ApiClient.KEY_TOKEN, '')) as string;

    // 如果 preferences 中没有 token，从 TokenStore 恢复
    if (ApiClient.token.length === 0) {
      const encryptedToken = await TokenStore.getAccessToken();
      if (encryptedToken.length > 0) {
        ApiClient.token = encryptedToken;
        await ApiClient.prefs.put(ApiClient.KEY_TOKEN, encryptedToken);
        await ApiClient.prefs.flush();
      }
    }
  }

  static async getBaseUrl(): Promise<string> {
    if (ApiClient.prefs) {
      return (await ApiClient.prefs.get(ApiClient.KEY_BASE_URL,
        ApiClient.DEFAULT_BASE_URL)) as string;
    }
    return ApiClient.DEFAULT_BASE_URL;
  }

  static async setBaseUrl(url: string): Promise<void> {
    if (!ApiClient.prefs) {
      return;
    }
    await ApiClient.prefs.put(ApiClient.KEY_BASE_URL, url);
    await ApiClient.prefs.flush();
    ApiClient.rebuildApi();
  }

  static saveToken(newToken: string): void {
    ApiClient.token = newToken;
    if (ApiClient.prefs) {
      ApiClient.prefs.put(ApiClient.KEY_TOKEN, newToken);
      ApiClient.prefs.flush();
    }
    TokenStore.saveToken(newToken);
  }

  static clearToken(): void {
    ApiClient.token = '';
    if (ApiClient.prefs) {
      ApiClient.prefs.delete(ApiClient.KEY_TOKEN);
      ApiClient.prefs.flush();
    }
    TokenStore.clear();
  }

  static get isLoggedIn(): boolean {
    return ApiClient.token.length > 0;
  }

  static getApi(): TidengApi {
    if (!ApiClient.api) {
      ApiClient.api = new TidengApi();
    }
    return ApiClient.api;
  }

  static rebuildApi(): void {
    ApiClient.api = null;
  }

  /**
   * 构建通用 HTTP 请求，自动注入 Authorization 头
   */
  static async request<T>(method: http.RequestMethod, path: string,
    body?: Object, extraHeaders?: Record<string, string>): Promise<T> {
    const baseUrl = await ApiClient.getBaseUrl();
    const url = ApiClient.joinUrl(baseUrl, path);

    const httpRequest = http.createHttp();

    // Build headers object (ArkTS strict mode: no spread on object literals)
    const headers: Record<string, string> = {
      'Content-Type': 'application/json'
    };
    if (ApiClient.token.length > 0) {
      headers['Authorization'] = `Bearer ${ApiClient.token}`;
    }
    if (extraHeaders) {
      const keys = Object.keys(extraHeaders);
      for (let i = 0; i < keys.length; i++) {
        headers[keys[i]] = extraHeaders[keys[i]];
      }
    }

    const requestOptions: http.HttpRequestOptions = {
      method: method,
      header: headers,
      connectTimeout: 15000,
      readTimeout: 15000,
    };

    if (body && method !== http.RequestMethod.GET) {
      requestOptions.extraData = JSON.stringify(body);
    }

    try {
      const response = await httpRequest.request(url, requestOptions);
      const responseCode = response.responseCode;

      if (responseCode === 401 && ApiClient.token.length > 0) {
        const currentToken = ApiClient.token;
        ApiClient.clearToken();
        ApiClient.rebuildApi();
        if (ApiClient.onUnauthorized) {
          ApiClient.onUnauthorized();
        }
        console.warn(`ApiClient: 收到 401 (token=${currentToken.substring(0, 8)}...)，已清除并跳转登录页`);
        return null as T;
      }

      const result = JSON.parse(response.result as string) as T;
      return result;
    } catch (err) {
      const msg = (err instanceof Error) ? err.message : String(err);
      console.error(`ApiClient request failed: ${msg}`);
      throw new Error(msg);
    } finally {
      httpRequest.destroy();
    }
  }

  private static joinUrl(base: string, path: string): string {
    let result = base;
    if (result.endsWith('/')) {
      result = result.substring(0, result.length - 1);
    }
    if (!path.startsWith('/')) {
      result += '/';
    }
    result += path;
    return result;
  }
}
