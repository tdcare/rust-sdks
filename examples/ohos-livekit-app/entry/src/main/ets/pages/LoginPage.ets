import { webview } from '@kit.ArkWeb';
import { router } from '@kit.ArkUI';
import { resourceManager } from '@kit.LocalizationKit';
import { ApiClient } from '../data/api/ApiClient';
import { TokenStore } from '../data/security/TokenStore';
import { SessionManager } from '../data/session/SessionManager';
import { AuthRepository } from '../data/repository/AuthRepository';
import { RespVO } from '../data/dto/Dtos';
import { util } from '@kit.ArkTS';

const TAG = 'LoginPage';
const LOGIN_PAGE_PATH = 'auth-admin/ui/#/login';

/**
 * JS Bridge 接口 — 暴露给 WebView 中的 JavaScript 调用。
 * OHOS 使用 javaScriptProxy 替代 Android 的 addJavascriptInterface。
 */
class AuthBridgeController {
  private loginHandled: boolean = false;
  private onLoginSuccessCallback?: (accessToken: string, expiresAtMs: number,
    username: string, displayName: string) => void;
  private onLogoutCallback?: (reason: string) => void;

  setCallbacks(
    loginCb: (accessToken: string, expiresAtMs: number, username: string, displayName: string) => void,
    logoutCb: (reason: string) => void
  ): void {
    this.onLoginSuccessCallback = loginCb;
    this.onLogoutCallback = logoutCb;
  }

  isHandled(): boolean {
    return this.loginHandled;
  }

  setHandled(): void {
    this.loginHandled = true;
  }

  onLoginSuccess(accessToken: string, expiresAt: string, username: string, displayName: string): void {
    console.info(`${TAG}: JS→Native: onLoginSuccess(username=${username})`);
    this.loginHandled = true;
    const expiresAtMs = parseInt(expiresAt) || 0;
    if (this.onLoginSuccessCallback) {
      this.onLoginSuccessCallback(accessToken, expiresAtMs, username, displayName);
    }
  }

  onLogout(reason: string): void {
    console.warn(`${TAG}: JS→Native: onLogout(reason=${reason})`);
    if (this.onLogoutCallback) {
      this.onLogoutCallback(reason);
    }
  }
}

@Entry
@Component
struct LoginPage {
  @State message: string = '';
  controller: webview.WebviewController = new webview.WebviewController();
  authBridge: AuthBridgeController = new AuthBridgeController();
  authRepo: AuthRepository = new AuthRepository();
  private isFromLogout: boolean = false;

  aboutToAppear(): void {
    // 检查是否从退出登录跳转而来
    const params = router.getParams() as Record<string, Object>;
    if (params && params['logout'] === true) {
      this.isFromLogout = true;
      console.info(`${TAG}: 从退出登录跳转`);
    }

    this.authBridge.setCallbacks(
      (accessToken, expiresAtMs, username, displayName) => {
        this.onLoginSuccess(accessToken, expiresAtMs, username, displayName);
      },
      (reason) => {
        this.onLogout(reason);
      }
    );

    // 已登录且非退出 → 直接恢复 session
    if (ApiClient.isLoggedIn && !this.isFromLogout) {
      this.resumeSession();
    }
  }

  async resumeSession(): Promise<void> {
    try {
      if (ApiClient.token.length > 0) {
        const userId = await SessionManager.getUserId();
        if (userId.length === 0) {
          const userName = await SessionManager.getUserName();
          this.authRepo.parseAndSaveUserIdFromToken(ApiClient.token, userName);
        }
      }
      try {
        await this.authRepo.fetchAndSaveUserInfo();
      } catch (_) {
        console.warn(`${TAG}: 获取用户信息失败，但登录已成功`);
      }
      await this.refreshTokenAfterLogin();
      this.goHome();
    } catch (e) {
      console.warn(`${TAG}: 恢复 session 失败: ${e}`);
      this.goHome();
    }
  }

  async onLoginSuccess(accessToken: string, expiresAtMs: number,
    username: string, displayName: string): Promise<void> {
    console.info(`${TAG}: 登录成功: username=${username}`);

    this.controller.stop();

    // 存储 Token
    ApiClient.saveToken(accessToken);
    ApiClient.rebuildApi();

    // 存储到安全存储
    await TokenStore.save(accessToken, expiresAtMs, username, displayName);

    // 保存用户信息
    const dn = displayName.length > 0 ? displayName : username;
    await SessionManager.setUserName(dn);

    try {
      this.authRepo.parseAndSaveUserIdFromToken(accessToken, dn);
      try {
        await this.authRepo.fetchAndSaveUserInfo();
      } catch (_) {
        console.warn(`${TAG}: 获取用户信息失败`);
      }
      await this.refreshTokenAfterLogin();
    } catch (e) {
      console.warn(`${TAG}: fetchUserInfo 异常: ${e}`);
    }

    this.goHome();
  }

  onLogout(reason: string): void {
    console.warn(`${TAG}: Token 刷新失败: ${reason}`);
    ApiClient.clearToken();
    this.loadLoginPage();
  }

  async refreshTokenAfterLogin(): Promise<void> {
    try {
      const token = ApiClient.token;
      if (token.length === 0) {
        return;
      }
      const api = ApiClient.getApi();
      const result = await api.refreshToken();
      if (RespVO.isOk(result as RespVO<Object>) && result.data && result.data.access_token.length > 0) {
        ApiClient.saveToken(result.data.access_token);
        ApiClient.rebuildApi();
        await TokenStore.saveExpiresAt(Date.now() + 7200000);
        console.info(`${TAG}: Token 刷新成功`);
      }
    } catch (e) {
      console.warn(`${TAG}: Token 刷新失败（非致命）: ${e}`);
    }
  }

  goHome(): void {
    router.replaceUrl({ url: 'pages/HomePage' });
  }

  loadLoginPage(): void {
    ApiClient.getBaseUrl().then((baseUrl: string) => {
      let url = baseUrl;
      if (url.endsWith('/')) {
        url = url.substring(0, url.length - 1);
      }
      url += '/' + LOGIN_PAGE_PATH;
      console.info(`${TAG}: 加载登录页: ${url}`);
      this.controller.loadUrl(url);
    });
  }

  /**
   * 注入 bridge.js 到 WebView。退出登录时先清除 localStorage 防止旧 token 触发自动登录。
   */
  async injectBridgeScript(): Promise<void> {
    try {
      // onPageBegin 已清除 localStorage，此处重置标记并注入 bridge.js
      if (this.isFromLogout) {
        this.isFromLogout = false;
        console.info(`${TAG}: 登录页已加载，重置退出标记`);
      }
      const context = getContext(this);
      const resMgr = context.resourceManager;
      const data = await resMgr.getRawFileContent('bridge.js');
      const textDecoder = new util.TextDecoder('utf-8');
      const script = textDecoder.decodeWithStream(data, { stream: false });
      this.controller.runJavaScript(script);
    } catch (e) {
      console.error(`${TAG}: 注入 bridge.js 失败: ${e}`);
    }
  }

  build() {
    Column() {
      // 顶部栏
      Row() {
        Text('RRT 快反登录')
          .fontSize(20)
          .fontWeight(FontWeight.Bold)
          .layoutWeight(1)
        Button('设置')
          .fontSize(14)
          .onClick(() => {
            router.pushUrl({ url: 'pages/SettingsPage' });
          })
      }
      .width('100%')
      .padding(16)

      // WebView 区域
      Web({ src: '', controller: this.controller })
          .width('100%')
          .height('100%')
          .javaScriptAccess(true)
          .domStorageAccess(true)
          .zoomAccess(false)
          .verticalScrollBarAccess(false)
          .horizontalScrollBarAccess(false)
          .javaScriptProxy({
            object: this.authBridge,
            name: 'AndroidBridge',
            methodList: ['onLoginSuccess', 'onLogout'],
            controller: this.controller
          })
          .onPageBegin((event) => {
            // 退出登录时：在页面脚本执行前清除 localStorage，防止 Vue 应用检测到旧 token
            if (this.isFromLogout) {
              try {
                this.controller.runJavaScript('localStorage.clear();');
                console.info(`${TAG}: onPageBegin 已清除 localStorage`);
              } catch (e) {
                console.warn(`${TAG}: onPageBegin 清除 localStorage 失败: ${e}`);
              }
            }
            console.info(`${TAG}: onPageBegin: ${event?.url}`);
          })
          .onPageEnd((event) => {
            console.info(`${TAG}: onPageEnd: ${event?.url}`);
            this.injectBridgeScript();
          })
          .onProgressChange((event) => {
            // 页面加载进度
          })
          .onErrorReceive((event) => {
            console.error(`${TAG}: WebView 错误: ${event?.error?.getErrorInfo()}`);
          })
        .width('100%')
        .layoutWeight(1)

      if (this.message.length > 0) {
        Text(this.message)
          .fontSize(14)
          .fontColor('#FF6600')
          .padding(10)
      }
    }
    .width('100%')
    .height('100%')
    .backgroundColor('#F5F5F5')
    .onAppear(() => {
      // 未登录时才加载 WebView 登录页
      if (!ApiClient.isLoggedIn) {
        this.loadLoginPage();
      }
    })
  }
}
