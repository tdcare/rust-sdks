import { MqttSignaling, MqttEventListener, MqttSignalingOptions } from './MqttSignaling';
import { ApiClient } from '../data/api/ApiClient';
import { SessionManager } from '../data/session/SessionManager';
import {
  RrtLevel, CallContextDto, CallInitiateRequest, CallInitiateResponse, RespVO
} from '../data/dto/Dtos';
import { cryptoFramework } from '@kit.CryptoArchitectureKit';
import { buffer } from '@kit.ArkTS';

const TAG = 'RrtCallManager';

/**
 * 初始化选项接口
 */
interface InitializeOptions {
  livekitUrl: string;
  deviceSn: string;
  brokerUrl: string;
  mqttUsername: string;
  mqttPassword: string;
  livekitApiKey?: string;
  livekitApiSecret?: string;
}

/**
 * RRT 呼叫事件监听器
 */
export class RrtCallListener {
  onInitialized: () => void = () => {};
  onMqttConnected: () => void = () => {};
  onMqttDisconnected: () => void = () => {};
  onReconnecting: () => void = () => {};
  onReconnected: () => void = () => {};
  onReconnectFailed: () => void = () => {};
  onMemberJoined: (memberSn: string, name?: string) => void = () => {};
  onMemberLeft: (memberSn: string) => void = () => {};
  onCallConnected: () => void = () => {};
  onCallEnded: () => void = () => {};
  onIncomingCall: (fromSn: string, roomId: string, msg: string,
    callId: string, livekitRoom: string, contextJson: string) => void = () => {};
  onError: (msg: string) => void = () => {};
}

/**
 * 快反呼叫管理器（OHOS 版本）
 * 基于 MqttSignaling 和 LiveKit LkRoom 的呼叫管理。
 * 替换 Android 端的 LiveKitCallManager。
 */
export class RrtCallManager {
  private static instance: RrtCallManager | null = null;

  signaling: MqttSignaling = new MqttSignaling();
  listener: RrtCallListener | null = null;

  private callRules: RrtLevel[] = [];
  private currentCallId: string = '';
  private currentRoomId: string = '';
  private currentTargets: string[] = [];
  private isCallActive: boolean = false;
  private livekitApiKey: string = '';
  private livekitApiSecret: string = '';
  private livekitUrl: string = '';

  private constructor() {
  }

  static getInstance(): RrtCallManager {
    if (!RrtCallManager.instance) {
      RrtCallManager.instance = new RrtCallManager();
    }
    return RrtCallManager.instance;
  }

  /**
   * 初始化 MQTT 信令
   */
  async initialize(option: InitializeOptions): Promise<void> {
    try {
      let userId = await SessionManager.getUserId();
      const userName = await SessionManager.getUserName();

      if (userId.length === 0) {
        console.warn(`${TAG}: userId 为空，使用 deviceSn 作为临时标识`);
        userId = option.deviceSn;
      }

      const mqttListener = new MqttEventListener();
      mqttListener.onConnected = (): void => {
          console.info(`${TAG}: MQTT 已连接`);
          if (this.listener) {
            this.listener.onMqttConnected();
            this.listener.onInitialized();
          }
        };
      mqttListener.onDisconnected = (): void => {
            console.warn(`${TAG}: MQTT 已断开`);
            if (this.listener) {
              this.listener.onMqttDisconnected();
            }
          };
      mqttListener.onReconnecting = (): void => {
            console.info(`${TAG}: MQTT 重连中...`);
            if (this.listener) {
              this.listener.onReconnecting();
            }
          };
      mqttListener.onIncomingCall = (fromSn: string, roomId: string, msg: string, callType: string,
          livekitRoom: string, callId: string, contextJson: string): void => {
            console.info(`${TAG}: 收到来电 fromSn=${fromSn}, roomId=${roomId}`);
            this.currentCallId = callId;
            this.currentRoomId = roomId;
            if (this.listener) {
              this.listener.onIncomingCall(fromSn, roomId, msg, callId, livekitRoom, contextJson);
            }
          };
      mqttListener.onCallAccepted = (fromSn: string): void => {
            console.info(`${TAG}: 对方已接听: ${fromSn}`);
          };
      mqttListener.onRemoteHangup = (fromSn: string): void => {
            console.info(`${TAG}: 对方挂断: ${fromSn}`);
            this.isCallActive = false;
            if (this.listener) {
              this.listener.onMemberLeft(fromSn);
              this.listener.onCallEnded();
            }
          };
      mqttListener.onCallIgnored = (fromSn: string): void => {
            console.info(`${TAG}: 对方忽略呼叫: ${fromSn}`);
            if (this.listener) {
              this.listener.onMemberLeft(fromSn);
            }
          };
      mqttListener.onError = (msg: string): void => {
            console.error(`${TAG}: 错误: ${msg}`);
            if (this.listener) {
              this.listener.onError(msg);
            }
          };

      // 保存 LiveKit 配置供后续 token 生成使用
      this.livekitApiKey = option.livekitApiKey || '';
      this.livekitApiSecret = option.livekitApiSecret || '';
      this.livekitUrl = option.livekitUrl;

      const sigOptions = new MqttSignalingOptions();
      sigOptions.brokerUrl = option.brokerUrl;
      sigOptions.mqttUsername = option.mqttUsername;
      sigOptions.mqttPassword = option.mqttPassword;
      sigOptions.deviceSn = option.deviceSn;
      sigOptions.userId = userId;
      sigOptions.deviceType = 'PHONE';
      sigOptions.listener = mqttListener;

      await this.signaling.initialize(sigOptions);
      console.info(`${TAG}: 初始化完成, livekitUrl=${option.livekitUrl}`);
    } catch (e) {
      console.error(`${TAG}: 初始化异常: ${e}`);
      if (this.listener) {
        this.listener.onError(`初始化失败: ${e}`);
      }
    }
  }

  isInitialized(): boolean {
    return this.signaling.connected;
  }

  /**
   * 获取快反级别列表（带降级兜底）
   */
  async fetchCallRules(): Promise<RrtLevel[]> {
    try {
      const api = ApiClient.getApi();
      const resp = await api.getRrtLevels();
      const codeOk = resp.code === 'SUCCESS' || String(resp.code) === '0' || String(resp.code) === '200' || resp.status === 200;
      if (codeOk && resp.data && resp.data.length > 0) {
        this.callRules = resp.data;
        console.info(`${TAG}: 获取呼叫规则成功: ${this.callRules.map(l => l.levelCode).join(',')}`);
        return this.callRules;
      }
    } catch (e) {
      console.warn(`${TAG}: 获取呼叫规则异常: ${e}`);
    }

    // 降级兜底：默认 3 个级别
    this.callRules = [
      { levelCode: 'BLUE', levelName: '蓝色预警', priority: 1, description: '' } as RrtLevel,
      { levelCode: 'YELLOW', levelName: '黄色预警', priority: 2, description: '' } as RrtLevel,
      { levelCode: 'RED', levelName: '红色预警', priority: 3, description: '' } as RrtLevel,
    ];
    return this.callRules;
  }

  getCachedCallRules(): RrtLevel[] {
    return this.callRules;
  }

  /**
   * 发起快反呼叫
   */
  async initiateCall(levelCode: string, callContext?: CallContextDto): Promise<CallInitiateResponse | null> {
    if (!this.isInitialized()) {
      console.error(`${TAG}: 未初始化，无法发起呼叫`);
      if (this.listener) {
        this.listener.onError('呼叫SDK未初始化');
      }
      return null;
    }

    try {
      const deviceSn = ApiClient.token;  // use token as device identifier placeholder
      const actualDeviceSn = deviceSn; // will be set from MqttSignaling

      const request: CallInitiateRequest = new CallInitiateRequest();
      request.levelCode = levelCode;
      request.callerDeviceSn = ''; // will be set below
      request.teamId = '';
      request.context = callContext || null;

      // Use the device identifier
      request.callerDeviceSn = this.signaling.sn;

      console.info(`${TAG}: 发起快反呼叫: levelCode=${levelCode}`);
      const api = ApiClient.getApi();
      const resp = await api.initiateCall(request);

      const codeOk = resp.code === 'SUCCESS' || String(resp.code) === '0' || String(resp.code) === '200' || resp.status === 200;
      if (!codeOk || !resp.data) {
        const errMsg = resp.msg || `code=${resp.code}`;
        console.error(`${TAG}: 发起呼叫失败: ${errMsg}`);
        if (this.listener) {
          this.listener.onError(`发起呼叫失败: ${errMsg}`);
        }
        return null;
      }

      const result = resp.data;
      this.currentCallId = result.callId;
      this.currentRoomId = result.livekitRoom;
      this.currentTargets = result.targets;
      this.isCallActive = true;

      // 1. 后端已通过 send_video_call_signaling 向每个目标发送 MQTT
      // 2. OHOS 客户端同时直发 MQTT 作为兜底，确保所有目标（包括未被后端 team_member 匹配到的 Web 客户端）都能收到通知
      // 使用 rrt Topic 体系: rrt/{roomId}/to/{targetSn}/video_call
      const msg = callContext?.description || `快反呼叫: ${levelCode}`;
      this.signaling.sendVideoCall(
        this.currentRoomId, result.targets, result.livekitRoom,
        result.callId, msg
      );

      console.info(`${TAG}: 呼叫已创建: callId=${result.callId}, room=${result.livekitRoom}, targets=${result.targets.length}`);
      return result;

    } catch (e) {
      console.error(`${TAG}: 发起呼叫异常: ${e}`);
      if (this.listener) {
        this.listener.onError(`发起呼叫失败: ${e}`);
      }
      return null;
    }
  }

  /**
   * 接听呼叫
   */
  async acceptCall(fromSn: string, roomId: string, callId: string, livekitRoom: string): Promise<void> {
    if (!this.isInitialized()) {
      console.error(`${TAG}: 未初始化`);
      return;
    }

    this.currentCallId = callId;
    this.currentRoomId = roomId;
    this.isCallActive = true;

    // 发送 MQTT 接听信令
    this.signaling.sendAnswer(roomId, fromSn, callId);

    console.info(`${TAG}: 已接听呼叫: roomId=${roomId}, livekitRoom=${livekitRoom}`);
  }

  /**
   * 拒绝呼叫
   */
  rejectCall(fromSn: string, roomId: string): void {
    this.signaling.sendIgnore(fromSn, roomId);
    console.info(`${TAG}: 已拒绝呼叫: fromSn=${fromSn}, roomId=${roomId}`);
  }

  /**
   * 挂断通话（同步发送 MQTT hangup + 异步调用后端 cancel 释放房间锁）
   */
  hangup(): void {
    if (this.currentRoomId.length > 0) {
      this.signaling.sendHangup(this.currentRoomId, this.currentTargets);
    }
    this.isCallActive = false;
    console.info(`${TAG}: 已挂断: roomId=${this.currentRoomId}`);

    // 异步调用后端 cancel 释放 active_room_lock，防止下次发起呼叫时被锁
    if (this.currentCallId.length > 0) {
      const callId = this.currentCallId;
      ApiClient.getApi().cancelCall(callId).then((resp: RespVO<Object>) => {
        console.info(`${TAG}: cancelCall 完成: callId=${callId}, code=${resp.code}`);
      }).catch((e: Error) => {
        console.warn(`${TAG}: cancelCall 失败（非关键）: callId=${callId}, err=${e.message}`);
      });
    }
  }

  getCurrentCallId(): string {
    return this.currentCallId;
  }

  getCurrentRoomId(): string {
    return this.currentRoomId;
  }

  getCurrentTargets(): string[] {
    return this.currentTargets;
  }

  isInCall(): boolean {
    return this.isCallActive;
  }

  getLivekitApiKey(): string {
    return this.livekitApiKey;
  }

  getLivekitApiSecret(): string {
    return this.livekitApiSecret;
  }

  getLivekitUrl(): string {
    return this.livekitUrl;
  }

  /**
   * 获取 LiveKit Token（后端优先，本地 fallback）
   * 对齐 rrt-web tidengApi.ts getLiveKitToken() 逻辑
   */
  async getLiveKitToken(room: string, identity: string, name?: string): Promise<string> {
    try {
      const api = ApiClient.getApi();
      return await api.getLiveKitToken(room, identity, name);
    } catch (e) {
      console.warn(`${TAG}: 后端 token 接口不可用，使用本地签发 token`);
      return await RrtCallManager.generateLiveKitTokenLocal(
        room, identity, this.livekitApiKey, this.livekitApiSecret);
    }
  }

  /**
   * 本地签发 LiveKit JWT token (HS256, fallback)
   * 使用 RrtCallManager 持有的 API Key/Secret
   */
  private static async generateLiveKitTokenLocal(
    room: string, identity: string, apiKey: string, apiSecret: string
  ): Promise<string> {
    const now = Math.floor(Date.now() / 1000);
    const exp = now + 6 * 3600;

    // ArkTS: build JSON via string concat to avoid comma-operator / untyped-object-literal errors
    const headerJson = '{"typ":"JWT","alg":"HS256"}';
    const claimsJson =
      '{"exp":' + exp + ',"iss":"' + apiKey + '","nbf":' + now + ',"sub":"' + identity + '","name":"",' +
      '"video":{"roomJoin":true,"room":"' + room + '","canPublish":true,"canSubscribe":true,"canPublishData":true},' +
      '"sip":{"admin":false,"call":false},"sha256":"","metadata":"","attributes":{},"roomConfig":null}';

    // string → bytes → base64 → URL-safe (no padding, - instead of +, _ instead of /)
    const toBase64Url = (str: string): string => {
      const bytes = new Uint8Array(buffer.from(str).buffer);
      const b64 = buffer.from(bytes.buffer).toString('base64');
      return b64.replace(/=/g, '').replace(/\+/g, '-').replace(/\//g, '_');
    };

    const headerB64 = toBase64Url(headerJson);
    const claimsB64 = toBase64Url(claimsJson);
    const signingInput = headerB64 + '.' + claimsB64;

    // HMAC-SHA256 signing via cryptoFramework
    const mac = cryptoFramework.createMac('SHA256');
    const symKeyGenerator = cryptoFramework.createSymKeyGenerator('HMAC');
    const keyBytes = new Uint8Array(buffer.from(apiSecret).buffer);
    const symKey = await symKeyGenerator.convertKey({ data: keyBytes });
    await mac.init(symKey);
    const signBytes = new Uint8Array(buffer.from(signingInput).buffer);
    await mac.update({ data: signBytes });
    const sigResult = await mac.doFinal();
    // sigResult.data is ArrayBuffer; encode signature bytes to base64-url
    const sigB64 = buffer.from(sigResult.data).toString('base64');
    const signature = sigB64.replace(/=/g, '').replace(/\+/g, '-').replace(/\//g, '_');

    return signingInput + '.' + signature;
  }

  /**
   * 释放资源
   */
  release(): void {
    this.signaling.disconnect();
    this.isCallActive = false;
    this.currentCallId = '';
    this.currentRoomId = '';
    this.currentTargets = [];
    this.listener = null;
    console.info(`${TAG}: 资源已释放`);
  }
}
