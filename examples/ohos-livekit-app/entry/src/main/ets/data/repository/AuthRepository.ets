import { ApiClient } from '../api/ApiClient';
import { SessionManager } from '../session/SessionManager';
import { RespVO } from '../dto/Dtos';
import { util } from '@kit.ArkTS';

/**
 * 认证仓库 — Token 解析 / 用户信息 / 登出。
 * 登录本身由 WebView 接管，AuthRepository 不再负责发送 login 请求。
 */
export class AuthRepository {
  private get api() {
    return ApiClient.getApi();
  }

  /**
   * 从 JWT token 中解析用户信息并保存到 SessionManager。
   */
  parseAndSaveUserIdFromToken(token: string, username: string): void {
    try {
      const parts = token.split('.');
      if (parts.length >= 2) {
        const payloadBase64 = AuthRepository.base64UrlDecode(parts[1]);
        const payload = JSON.parse(payloadBase64) as Record<string, string>;

        const userId = payload['sub'] || payload['userId'] ||
          payload['user_id'] || payload['id'] || '';
        if (userId.length > 0) {
          SessionManager.setUserId(userId);
          console.info(`AuthRepository: 从 JWT 解析到 userId: ${userId}`);
        } else {
          SessionManager.setUserId(username);
          console.warn(`AuthRepository: JWT 中未找到 userId，使用 userName 作为 fallback: ${username}`);
        }

        const name = payload['name'] || payload['nickName'] || '';
        if (name.length > 0) {
          SessionManager.setUserName(name);
        }
      } else {
        SessionManager.setUserId(username);
        console.warn('AuthRepository: Token 非 JWT 格式，使用登录名作为 userId');
      }
    } catch (e) {
      SessionManager.setUserId(username);
      console.error(`AuthRepository: JWT 解析失败: ${e}, 使用 userName 作为 userId`);
    }
  }

  /**
   * 获取用户信息并保存到 SessionManager。
   */
  async fetchAndSaveUserInfo(): Promise<void> {
    const result = await this.api.getAuthenticationUser();
    if (RespVO.isOk(result as RespVO<Object>) && result.data) {
      const info = result.data;
      const userName = await SessionManager.getUserName();
      const userId = await SessionManager.getUserId();

      const newUserName = info.name || info.userName || userName;
      if (newUserName.length > 0) {
        await SessionManager.setUserName(newUserName);
      }
      if (info.id && info.id.length > 0) {
        await SessionManager.setUserId(info.id);
      }
      await SessionManager.setDepartmentId(info.departmentId || '');
      await SessionManager.setDepartmentName(info.departmentName || '');
    } else {
      throw new Error(result.msg || result.message || '获取用户信息失败');
    }
  }

  async logout(): Promise<void> {
    await SessionManager.logout();
  }

  /**
   * Base64 URL-Safe 解码
   */
  private static base64UrlDecode(input: string): string {
    // 补齐 = 和替换特殊字符
    let base64 = input.replace(/-/g, '+').replace(/_/g, '/');
    while (base64.length % 4 !== 0) {
      base64 += '=';
    }
    const textEncoder = new util.TextEncoder();
    const textDecoder = new util.TextDecoder('utf-8');
    // 通过 Uint8Array 手动解码 base64
    const binaryStr = AuthRepository.atobPolyfill(base64);
    const bytes = new Uint8Array(binaryStr.length);
    for (let i = 0; i < binaryStr.length; i++) {
      bytes[i] = binaryStr.charCodeAt(i);
    }
    return textDecoder.decodeWithStream(bytes, { stream: false });
  }

  /**
   * Base64 解码（polyfill，OHOS 中无全局 atob）
   */
  private static atobPolyfill(base64: string): string {
    const chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
    let result = '';
    let i = 0;
    const clean = base64.replace(/[^A-Za-z0-9+/=]/g, '');
    while (i < clean.length) {
      const idx1 = chars.indexOf(clean[i++]);
      const idx2 = chars.indexOf(clean[i++]);
      const idx3 = chars.indexOf(clean[i++]);
      const idx4 = chars.indexOf(clean[i++]);

      const c1 = (idx1 << 2) | (idx2 >> 4);
      const c2 = ((idx2 & 15) << 4) | (idx3 >> 2);
      const c3 = ((idx3 & 3) << 6) | idx4;

      result += String.fromCharCode(c1);
      if (idx3 !== 64) {
        result += String.fromCharCode(c2);
      }
      if (idx4 !== 64) {
        result += String.fromCharCode(c3);
      }
    }
    return result;
  }
}
