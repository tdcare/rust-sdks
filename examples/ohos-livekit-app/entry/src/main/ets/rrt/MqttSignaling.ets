import { MqttClient, MqttEvent, MqttQos } from 'tdmqtt';

const TAG = 'MqttSignaling';

/**
 * MQTT 事件监听器类（使用 class 而非 interface 以符合 ArkTS 严格模式）
 */
export class MqttEventListener {
  onConnected: () => void = () => {};
  onDisconnected: () => void = () => {};
  onReconnecting: () => void = () => {};
  onIncomingCall: (fromSn: string, roomId: string, msg: string, callType: string,
    livekitRoom: string, callId: string, contextJson: string) => void = () => {};
  onCallAccepted: (fromSn: string) => void = () => {};
  onRemoteHangup: (fromSn: string) => void = () => {};
  onCallIgnored: (fromSn: string) => void = () => {};
  onError: (msg: string) => void = () => {};
}

/**
 * MQTT 信令管理器
 * 负责 MQTT 连接、Presence（在线/离线心跳）、呼叫信令收发。
 * 基于 tdmqtt (rumqttc napi-ohos)，参考 tdnis-ohos MQTTManager 实现。
 */
export class MqttSignaling {
  private client: MqttClient | null = null;
  private listener: MqttEventListener | null = null;
  private isConnected: boolean = false;

  // 重连相关
  private reconnectTimer: number | null = null;
  private readonly MAX_RECONNECT_TIMES: number = 20;
  private retryCount: number = 0;
  private isRebuilding: boolean = false;

  // 事件轮询
  private pollTimer: number | null = null;
  private readonly POLL_INTERVAL: number = 50; // 50ms 轮询
  private clientInstanceId: number = 0;

  // Presence 心跳
  private presenceTimer: number | null = null;
  private readonly PRESENCE_INTERVAL: number = 30000; // 30 秒心跳

  // 连接参数
  private brokerUrl: string = '';
  private mqttUsername: string = '';
  private mqttPassword: string = '';
  private deviceSn: string = '';
  private userId: string = '';
  private deviceType: string = 'PHONE';

  // 已订阅主题
  private subscribedTopics: Set<string> = new Set();

  /** 获取连接状态 */
  get connected(): boolean {
    return this.isConnected;
  }

  /** 获取设备 SN */
  get sn(): string {
    return this.deviceSn;
  }

  /**
   * 初始化并连接
   */
  async initialize(option: MqttSignalingOptions): Promise<boolean> {
    this.brokerUrl = option.brokerUrl;
    this.mqttUsername = option.mqttUsername;
    this.mqttPassword = option.mqttPassword;
    this.deviceSn = option.deviceSn;
    this.userId = option.userId;
    this.deviceType = option.deviceType || 'PHONE';
    this.listener = option.listener;

    return this.connect();
  }

  setListener(listener: MqttEventListener | null): void {
    this.listener = listener;
  }

  // 记录最近一次连接错误，用于判断是否需要切换协议
  private lastConnectError: string = '';

  /**
   * 连接到 MQTT Broker，自动协商协议
   * - wss:// 失败 → 回退 ws://（nginx TLS 终结场景）
   * - ws:// 失败 → 回退 wss://（直接 TLS 场景）
   */
  private async connect(): Promise<boolean> {
    if (this.isRebuilding) {
      console.warn(`${TAG}: 正在重建中，跳过本次连接请求`);
      return false;
    }

    // 首先尝试原始协议，失败后自动切换 ws↔wss
    const urlsToTry: string[] = [this.brokerUrl];
    if (this.brokerUrl.startsWith('wss://')) {
      urlsToTry.push(this.brokerUrl.replace('wss://', 'ws://'));
    } else if (this.brokerUrl.startsWith('ws://')) {
      urlsToTry.push(this.brokerUrl.replace('ws://', 'wss://'));
    }

    for (let i = 0; i < urlsToTry.length; i++) {
      const url = urlsToTry[i];
      const isFallback = i > 0;
      if (isFallback) {
        console.info(`${TAG}: 协议回退尝试: ${url} (上次错误=${this.lastConnectError})`);
      }

      const result = await this.tryConnect(url);
      if (result) {
        if (isFallback) {
          console.info(`${TAG}: 协议回退成功，更新 brokerUrl`);
          this.brokerUrl = url;
        }
        return true;
      }
    }

    console.error(`${TAG}: 所有连接尝试均失败`);
    this.scheduleReconnect();
    return false;
  }

  /**
   * 尝试用指定 URL 连接
   */
  private async tryConnect(url: string): Promise<boolean> {
    try {
      console.info(`${TAG}: 尝试连接: ${url}`);

      // 清理旧客户端
      this.stopEventPolling();
      if (this.client) {
        try {
          this.client.disconnect();
        } catch (_) { /* ignore */ }
        this.client = null;
      }

      this.clientInstanceId++;

      // 创建 MQTT 客户端（构造时自动连接）
      // 与 Android 保持一致: clientId 使用 sn 后 8 位 + userId hash 后缀
      const snSuffix = this.deviceSn.slice(-8);
      const userIdHash = (this.userId || String(Date.now())).split('')
        .reduce((h, c) => ((h << 5) - h + c.charCodeAt(0)) | 0, 0) & 0xFFFF;
      const userIdHex = userIdHash.toString(16);
      const clientId = `${snSuffix}_${userIdHex}`;
      this.client = new MqttClient({
        brokerUrl: url,
        clientId: clientId,
        username: this.mqttUsername || undefined,
        password: this.mqttPassword || undefined,
        keepAliveSecs: 5,  // 与 Android 保持一致 (HiveMQ keepAlive(5))
        cleanSession: true
      });

      // 启动事件轮询
      this.startEventPolling();

      // 等待连接
      const connected = await this.waitForConnection(10000);

      if (connected) {
        console.info(`${TAG}: MQTT 连接成功 (${url})`);
        this.isConnected = true;
        this.retryCount = 0;

        // 订阅呼叫相关主题
        await this.subscribeCallTopics();

        // 发布在线状态
        this.publishPresence('online');

        // 启动心跳
        this.startPresenceHeartbeat();

        if (this.listener) {
          this.listener.onConnected();
        }
        return true;
      } else {
        console.warn(`${TAG}: 连接超时 (${url})`);
        return false;
      }
    } catch (e) {
      const msg = (e instanceof Error) ? e.message : String(e);
      console.error(`${TAG}: 连接失败 (${url}): ${msg}`);
      this.lastConnectError = msg;
      return false;
    }
  }

  /**
   * 等待连接建立
   */
  private waitForConnection(timeout: number): Promise<boolean> {
    return new Promise<boolean>((resolve) => {
      const startTime = Date.now();
      const check = () => {
        if (this.client?.isConnected()) {
          resolve(true);
          return;
        }
        if (Date.now() - startTime > timeout) {
          resolve(false);
          return;
        }
        setTimeout(check, 100);
      };
      check();
    });
  }

  /**
   * 启动事件轮询
   */
  private startEventPolling(): void {
    if (this.pollTimer !== null) {
      return;
    }
    const currentInstanceId = this.clientInstanceId;

    const poll = () => {
      if (currentInstanceId !== this.clientInstanceId || this.isRebuilding) {
        return;
      }
      if (this.client) {
        try {
          const events: MqttEvent[] = this.client.pollAllEvents();
          for (const event of events) {
            this.handleEvent(event);
          }
        } catch (e) {
          console.error(`${TAG}: 轮询事件异常: ${e}`);
        }
      }
      if (currentInstanceId === this.clientInstanceId && !this.isRebuilding) {
        this.pollTimer = setTimeout(poll, this.POLL_INTERVAL);
      }
    };

    poll();
    console.info(`${TAG}: 事件轮询已启动`);
  }

  /**
   * 停止事件轮询
   */
  private stopEventPolling(): void {
    if (this.pollTimer !== null) {
      clearTimeout(this.pollTimer);
      this.pollTimer = null;
    }
  }

  /**
   * 处理 MQTT 事件
   */
  private handleEvent(event: MqttEvent): void {
    switch (event.type) {
      case 'connected':
        console.info(`${TAG}: 收到连接事件`);
        if (!this.isConnected) {
          this.isConnected = true;
          this.retryCount = 0;
          this.resubscribeTopics();
          this.publishPresence('online');
          this.startPresenceHeartbeat();
          if (this.listener) {
            this.listener.onConnected();
          }
        }
        break;

      case 'disconnected':
        console.warn(`${TAG}: 收到断连事件`);
        if (this.isConnected) {
          this.isConnected = false;
          this.stopPresenceHeartbeat();
          if (this.listener) {
            this.listener.onDisconnected();
          }
          this.scheduleReconnect();
        }
        break;

      case 'message':
        if (event.topic && event.payload) {
          this.handleMessage(event.topic, event.payload);
        }
        break;

      case 'error':
        console.error(`${TAG}: 错误事件: ${event.error}`);
        this.lastConnectError = event.error || 'unknown MQTT error';
        if (this.listener) {
          this.listener.onError(event.error || 'MQTT error');
        }
        break;
    }
  }

  /**
   * 处理收到的 MQTT 消息，根据 topic 路由到不同的呼叫事件
   * 解析 rrt/{roomId}/to/{mySn}/{action} 格式，与 Web 端处理逻辑对齐
   */
  private handleMessage(topic: string, payload: string): void {
    console.info(`${TAG}: 收到消息 topic=${topic}`);

    const parts = topic.split('/');
    // 格式要求: rrt/{roomId}/to/{mySn}/{action}
    if (parts.length < 5 || parts[0] !== 'rrt' || parts[2] !== 'to') {
      return; // 非 P2P 信令消息，忽略
    }

    try {
      const json = JSON.parse(payload) as Record<string, string>;
      const fromSn = json['from_mac'] || json['from'] || '';

      // 跳过自己发送的消息（避免发起方收到自己的来电通知）
      if (fromSn === this.deviceSn) {
        return;
      }

      const roomId = parts[1];     // rrt/{roomId}/to/{mySn}/{action}
      const action = parts[4];

      if (action === 'video_call') {
        // 来电通知
        const msg = json['msg'] || '';
        const callType = json['call_type'] || 'VIDEO';
        const livekitRoom = json['livekitRoom'] || json['livekit_room'] || '';
        const callId = json['callId'] || json['call_id'] || roomId;
        const contextJson = json['context'] || '';

        if (this.listener) {
          this.listener.onIncomingCall(fromSn, roomId, msg, callType,
            livekitRoom, callId, contextJson);
        }
      } else if (action === 'doing') {
        if (this.listener) {
          this.listener.onCallAccepted(fromSn);
        }
      } else if (action === 'video_hangup' || action === 'hangup') {
        if (this.listener) {
          this.listener.onRemoteHangup(fromSn);
        }
      } else if (action === 'ignore') {
        if (this.listener) {
          this.listener.onCallIgnored(fromSn);
        }
      }
    } catch (e) {
      console.warn(`${TAG}: 消息解析失败: ${e}`);
    }
  }

  /**
   * 订阅呼叫相关主题
   */
  private async subscribeCallTopics(): Promise<void> {
    // 使用 rrt Topic 体系，与 Web/后端保持一致
    // rrt/{roomId}/to/{myDeviceSn}/{action} 格式：
    //   video_call — 来电通知
    //   doing      — 接听确认
    //   video_hangup / hangup — 挂断
    //   ignore     — 忽略/拒绝
    await this.subscribe(`rrt/+/to/${this.deviceSn}/#`, MqttQos.AtLeastOnce);
    // Presence 状态（与其他端保持一致）
    await this.subscribe('rrt/presence/+/+', MqttQos.AtLeastOnce);
  }

  /**
   * 订阅主题
   */
  async subscribe(topic: string, qos: MqttQos = MqttQos.AtLeastOnce): Promise<boolean> {
    if (!this.client || !this.isConnected) {
      return false;
    }
    try {
      const result = this.client.subscribe(topic, qos);
      if (result) {
        this.subscribedTopics.add(topic);
        console.info(`${TAG}: 订阅成功: ${topic}`);
      }
      return result;
    } catch (e) {
      console.error(`${TAG}: 订阅失败 ${topic}: ${e}`);
      return false;
    }
  }

  /**
   * 重新订阅所有主题（重连后）
   */
  private resubscribeTopics(): void {
    for (const topic of this.subscribedTopics) {
      if (this.client) {
        try {
          this.client.subscribe(topic, MqttQos.AtLeastOnce);
        } catch (_) { /* ignore */ }
      }
    }
  }

  /**
   * 发布消息
   */
  async publish(topic: string, payload: string, qos: MqttQos = MqttQos.AtLeastOnce): Promise<boolean> {
    if (!this.client || !this.isConnected) {
      return false;
    }
    try {
      return this.client.publish(topic, payload, qos, false);
    } catch (e) {
      console.error(`${TAG}: 发布失败 ${topic}: ${e}`);
      return false;
    }
  }

  // ========== Presence 在线/离线 ==========

  /**
   * 发布在线/离线状态
   */
  publishPresence(status: 'online' | 'offline'): void {
    const topic = `rrt/presence/${this.userId}/${this.deviceSn}`;
    const payload = JSON.stringify({
      deviceSn: this.deviceSn,
      userId: this.userId,
      deviceType: this.deviceType,
      status: status,
      timestamp: Date.now()
    });
    this.publish(topic, payload, MqttQos.AtLeastOnce);
  }

  /**
   * 启动 Presence 心跳
   */
  private startPresenceHeartbeat(): void {
    this.stopPresenceHeartbeat();
    this.presenceTimer = setInterval(() => {
      if (this.isConnected) {
        this.publishPresence('online');
      }
    }, this.PRESENCE_INTERVAL);
  }

  /**
   * 停止 Presence 心跳
   */
  private stopPresenceHeartbeat(): void {
    if (this.presenceTimer !== null) {
      clearInterval(this.presenceTimer);
      this.presenceTimer = null;
    }
  }

  // ========== 呼叫信令发送 ==========

  /**
   * 发送呼叫邀请 — 使用 rrt Topic 体系，与 Web/后端对齐
   * 对每个目标单独发送到 rrt/{roomId}/to/{targetSn}/video_call
   */
  sendVideoCall(roomId: string, targets: string[], livekitRoom: string,
    callId: string, msg: string): void {
    for (const target of targets) {
      // 跳过自己
      if (target === this.deviceSn) {
        continue;
      }
      const topic = `rrt/${roomId}/to/${target}/video_call`;
      const payload = JSON.stringify({
        action: 'video_call',
        call_id: callId,
        callId: callId,
        livekit_room: livekitRoom,
        livekitRoom: livekitRoom,
        from_mac: this.deviceSn,
        from: this.deviceSn,
        msg: msg,
        call_type: 'VIDEO',
        timestamp: Date.now()
      });
      this.publish(topic, payload, MqttQos.AtLeastOnce);
    }
  }

  /**
   * 发送接听确认 — rrt/{roomId}/to/{targetSn}/doing
   */
  sendAnswer(roomId: string, targetSn: string, callId: string): void {
    const topic = `rrt/${roomId}/to/${targetSn}/doing`;
    const payload = JSON.stringify({
      action: 'doing',
      from_mac: this.deviceSn,
      from: this.deviceSn,
      call_id: callId,
      callId: callId,
      deviceType: this.deviceType,
      timestamp: Date.now()
    });
    this.publish(topic, payload, MqttQos.AtLeastOnce);
  }

  /**
   * 发送忽略/拒绝 — rrt/{roomId}/to/{targetSn}/ignore
   */
  sendIgnore(targetSn: string, roomId: string): void {
    const topic = `rrt/${roomId}/to/${targetSn}/ignore`;
    const payload = JSON.stringify({
      action: 'ignore',
      from_mac: this.deviceSn,
      from: this.deviceSn,
      timestamp: Date.now()
    });
    this.publish(topic, payload, MqttQos.AtLeastOnce);
  }

  /**
   * 发送挂断 — 对每个目标发送 rrt/{roomId}/to/{targetSn}/video_hangup
   */
  sendHangup(roomId: string, targets: string[]): void {
    for (const target of targets) {
      if (target === this.deviceSn) {
        continue;
      }
      const topic = `rrt/${roomId}/to/${target}/video_hangup`;
      const payload = JSON.stringify({
        action: 'video_hangup',
        from_mac: this.deviceSn,
        from: this.deviceSn,
        timestamp: Date.now()
      });
      this.publish(topic, payload, MqttQos.AtLeastOnce);
    }
  }

  // ========== 重连 ==========

  private scheduleReconnect(): void {
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
    }
    if (this.retryCount >= this.MAX_RECONNECT_TIMES) {
      console.error(`${TAG}: 已达最大重连次数，停止重连`);
      if (this.listener) {
        this.listener.onError('重连失败，请检查网络');
      }
      return;
    }
    if (this.listener) {
      this.listener.onReconnecting();
    }
    const delay = Math.min(5000 + (this.retryCount * 2000), 30000);
    console.info(`${TAG}: ${delay / 1000}s 后尝试重连 (第 ${this.retryCount + 1} 次)`);
    this.reconnectTimer = setTimeout(() => {
      this.retryCount++;
      this.connect();
    }, delay);
  }

  /**
   * 断开连接并释放资源
   */
  disconnect(): void {
    this.stopPresenceHeartbeat();
    this.stopEventPolling();

    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }

    // 发送离线状态（最佳努力）
    this.publishPresence('offline');

    this.subscribedTopics.clear();

    if (this.client && this.isConnected) {
      try {
        this.client.disconnect();
      } catch (_) { /* ignore */ }
    }
    this.isConnected = false;
    this.client = null;
    this.listener = null;
    console.info(`${TAG}: MQTT 已断开`);
  }
}

/**
 * MQTT 信令初始化参数
 */
export class MqttSignalingOptions {
  brokerUrl: string = '';
  mqttUsername: string = '';
  mqttPassword: string = '';
  deviceSn: string = '';
  userId: string = '';
  deviceType: string = 'PHONE';
  listener: MqttEventListener | null = null;
}
