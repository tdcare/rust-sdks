import { router } from '@kit.ArkUI';
import { ApiClient } from '../data/api/ApiClient';
import { SessionManager } from '../data/session/SessionManager';
import { AuthRepository } from '../data/repository/AuthRepository';
import { DeviceUtil } from '../util/DeviceUtil';
import { RrtCallManager, RrtCallListener } from '../rrt/RrtCallManager';
import { RrtLevel, RespVO } from '../data/dto/Dtos';

const TAG = 'HomePage';

@Entry
@Component
struct HomePage {
  @State userName: string = '加载中...';
  @State department: string = '加载中...';
  @State deviceSn: string = '';
  @State statusText: string = '连接中...';
  @State statusColor: string = '#FF9800'; // 橙色 = 连接中
  @State isOnline: boolean = false;
  @State isRefreshing: boolean = false;
  private authRepo: AuthRepository = new AuthRepository();

  aboutToAppear(): void {
    // 检查是否已登录
    if (!ApiClient.isLoggedIn) {
      router.replaceUrl({ url: 'pages/LoginPage' });
      return;
    }

    // 注册 401 拦截回调
    ApiClient.onUnauthorized = () => {
      this.logout();
    };

    this.updateUserInfo();
    this.initConnection();
  }

  onPageShow(): void {
    // 确保 401 回调始终指向当前页面
    ApiClient.onUnauthorized = () => {
      this.logout();
    };
    this.updateUserInfo();
  }

  updateUserInfo(): void {
    this.deviceSn = DeviceUtil.getDeviceSn();
    SessionManager.getUserName().then((name: string) => {
      this.userName = name.length > 0 ? name : '未知用户';
    });
    SessionManager.getDepartmentName().then((name: string) => {
      this.department = name.length > 0 ? name : '未知科室';
    });
  }

  async initConnection(): Promise<void> {
    // 确保 userId 已设置
    const userId = await SessionManager.getUserId();
    if (userId.length === 0 && ApiClient.token.length > 0) {
      const userName = await SessionManager.getUserName();
      this.authRepo.parseAndSaveUserIdFromToken(ApiClient.token, userName);
      console.info(`${TAG}: 从已保存 token 补充解析 userId`);
    }

    const departmentId = await SessionManager.getDepartmentId();
    if (departmentId.length === 0) {
      console.warn(`${TAG}: 科室信息为空，仍继续初始化连接`);
    }

    this.updateStatus(false, '连接中...');

    try {
      const api = ApiClient.getApi();
      const deviceSn = DeviceUtil.getDeviceSn();
      this.deviceSn = deviceSn;

      // 1. 获取客户端配置
      const resp = await api.getClientConfig();
      if (!RespVO.isOk(resp as RespVO<Object>) || !resp.data) {
        console.warn(`${TAG}: 客户端配置未获取到`);
        this.updateStatus(false, '客户端配置未获取到');
        return;
      }
      const config = resp.data;

      // 配置优先级：call.mqtt > 顶层 mqtt
      let mqttUrl = '';
      let mqttUsername = '';
      let mqttPassword = '';

      const callMap = config.call;
      if (callMap) {
        const callMqtt = callMap['mqtt'];
        if (callMqtt && typeof callMqtt === 'object') {
          const mqttObj = callMqtt as Record<string, Object>;
          mqttUrl = (mqttObj['url'] as string) || '';
          mqttUsername = (mqttObj['username'] as string) || '';
          mqttPassword = (mqttObj['password'] as string) || '';
        }
      }
      if (mqttUrl.length === 0 && config.mqtt) {
        mqttUrl = config.mqtt.url || '';
        mqttUsername = config.mqtt.username || '';
        mqttPassword = config.mqtt.password || '';
      }

      const brokerUrl = mqttUrl || 'wss://tdtest.tdcare.cn:6115/mqtt';
      const finalMqttUsername = mqttUsername || 'cdo';
      const finalMqttPassword = mqttPassword || 'Tdcarefor123';

      // LiveKit 配置
      let livekitUrl = '';
      let livekitApiKey = '';
      let livekitApiSecret = '';

      if (callMap) {
        const callLivekit = callMap['livekit'];
        if (callLivekit && typeof callLivekit === 'object') {
          const lkObj = callLivekit as Record<string, Object>;
          livekitUrl = (lkObj['url'] as string) || '';
          livekitApiKey = (lkObj['apiKey'] as string) || '';
          livekitApiSecret = (lkObj['apiSecret'] as string) || '';
        }
      }
      if (livekitUrl.length === 0 && config.livekit) {
        livekitUrl = config.livekit.url || '';
        livekitApiKey = config.livekit.apiKey || '';
        livekitApiSecret = config.livekit.apiSecret || '';
      }

      if (livekitUrl.length === 0) {
        console.warn(`${TAG}: LiveKit服务器地址为空`);
        this.updateStatus(false, 'LiveKit服务器地址为空');
        return;
      }

      console.info(`${TAG}: MQTT broker=${brokerUrl}, LiveKit=${livekitUrl}`);

      // 初始化 RrtCallManager（MQTT + 信令）
      const callManager = RrtCallManager.getInstance();
      callManager.listener = this.createCallListener();
      await callManager.initialize({
        livekitUrl: livekitUrl,
        deviceSn: deviceSn,
        brokerUrl: brokerUrl,
        mqttUsername: finalMqttUsername,
        mqttPassword: finalMqttPassword,
        livekitApiKey: livekitApiKey,
        livekitApiSecret: livekitApiSecret
      });

      // 预加载快反级别列表
      try {
        const levelsResult = await api.getRrtLevels();
        const levels = levelsResult.data;
        if (levels && levels.length > 0) {
          console.info(`${TAG}: 预加载快反级别: ${levels.map((l) => l.levelCode).join(',')}`);
        }
      } catch (e) {
        console.warn(`${TAG}: 预加载快反级别失败: ${e}`);
      }

    } catch (e) {
      console.error(`${TAG}: 连接初始化失败: ${e}`);
      this.updateStatus(false, `连接失败: ${e}`);
    }
  }

  updateStatus(online: boolean, text: string): void {
    this.statusText = text;
    this.isOnline = online;
    if (online) {
      this.statusColor = '#4CAF50';
    } else {
      this.statusColor = '#FF4444';
    }
  }

  /**
   * 下拉刷新：重新初始化连接（MQTT + LiveKit 配置）
   */
  async refreshConnection(): Promise<void> {
    this.isRefreshing = true;
    this.updateStatus(false, '刷新中...');
    try {
      await this.initConnection();
    } finally {
      this.isRefreshing = false;
    }
  }

  /**
   * 创建呼叫事件监听器
   */
  createCallListener(): RrtCallListener {
    const listener = new RrtCallListener();
    listener.onInitialized = (): void => {
        const dept = '';
        SessionManager.getDepartmentId().then((id: string) => {
          this.updateStatus(true, id.length > 0 ? '在线待命中' : '在线（未分配科室）');
        });
      };
    listener.onMqttConnected = (): void => {
        SessionManager.getDepartmentId().then((id: string) => {
          this.updateStatus(true, id.length > 0 ? '在线待命中' : '在线（未分配科室）');
        });
      };
    listener.onMqttDisconnected = (): void => {
        this.updateStatus(false, '连接已断开');
      };
    listener.onReconnecting = (): void => {
        this.updateStatus(false, '重连中...');
      };
    listener.onReconnected = (): void => {
        SessionManager.getDepartmentId().then((id: string) => {
          this.updateStatus(true, id.length > 0 ? '在线待命中' : '在线（未分配科室）');
        });
      };
    listener.onReconnectFailed = (): void => {
        this.updateStatus(false, '重连失败');
      };
    listener.onMemberJoined = (memberSn: string, name?: string): void => {
        console.info(`${TAG}: 成员加入: ${memberSn}`);
      };
    listener.onMemberLeft = (memberSn: string): void => {
        console.info(`${TAG}: 成员离开: ${memberSn}`);
      };
    listener.onCallConnected = (): void => {
        console.info(`${TAG}: 通话已连接`);
      };
    listener.onCallEnded = (): void => {
        SessionManager.getDepartmentId().then((id: string) => {
          this.updateStatus(true, id.length > 0 ? '在线待命中' : '在线（未分配科室）');
        });
      };
    listener.onIncomingCall = (fromSn: string, roomId: string, msg: string, callId: string,
        livekitRoom: string, contextJson: string): void => {
        console.info(`${TAG}: 收到快反来电: fromSn=${fromSn}`);
        // 跳转到来电界面
        router.pushUrl({
          url: 'pages/IncomingCallPage',
          params: {
            fromSn: fromSn,
            roomId: roomId,
            msg: msg,
            callId: callId,
            livekitRoom: livekitRoom,
            contextJson: contextJson
          }
        });
      };
    listener.onError = (msg: string): void => {
        this.updateStatus(false, `连接失败: ${msg}`);
      };
    return listener;
  }

  async logout(): Promise<void> {
    // 先断开 MQTT 连接
    RrtCallManager.getInstance().release();
    // 等待 session 清理完成（清除 token + preferences）
    await SessionManager.logout();
    // 清理完成后跳转登录页，携带 logout 参数防止自动恢复
    router.replaceUrl({ url: 'pages/LoginPage', params: { logout: true } });
  }

  build() {
    Column() {
      // 顶部栏（固定，不受下拉刷新影响）
      Row() {
        Text('RRT 快反')
          .fontSize(20)
          .fontWeight(FontWeight.Bold)
          .layoutWeight(1)
        Button('设置')
          .fontSize(14)
          .onClick(() => {
            router.pushUrl({ url: 'pages/SettingsPage' });
          })
          .margin({ right: 8 })
        Button('退出')
          .fontSize(14)
          .backgroundColor('#FF4444')
          .onClick(() => {
            this.logout();
          })
      }
      .width('100%')
      .padding(16)

      Divider()

      // 下拉刷新区域（仅下方内容可下拉）
      Refresh({ refreshing: $$this.isRefreshing }) {
        Scroll() {
          Column() {
            // 用户信息区
            Column() {
              Text(this.userName)
                .fontSize(24)
                .fontWeight(FontWeight.Medium)
              Text(this.department)
                .fontSize(16)
                .fontColor('#666666')
                .margin({ top: 4 })
              Text(`设备: ${this.deviceSn}`)
                .fontSize(12)
                .fontColor('#999999')
                .margin({ top: 4 })
            }
            .width('100%')
            .padding(16)
            .alignItems(HorizontalAlign.Center)

            Divider()

            // 连接状态区
            Row() {
              Circle({ width: 12, height: 12 })
                .fill(this.statusColor)
              Text(this.statusText)
                .fontSize(16)
                .margin({ left: 8 })
            }
            .width('100%')
            .padding(16)
            .justifyContent(FlexAlign.Center)

            Divider()

            // 快反呼叫按钮
            Column() {
              Button('发起快反呼叫')
                .fontSize(20)
                .width('80%')
                .height(56)
                .enabled(this.isOnline)
                .onClick(() => {
                  const callManager = RrtCallManager.getInstance();
                  if (!callManager.isInitialized()) {
                    this.updateStatus(false, '请等待连接就绪');
                    return;
                  }
                  SessionManager.getDepartmentId().then((deptId: string) => {
                    if (deptId.length === 0) {
                      this.updateStatus(true, '当前账号未分配科室，无法发起快反呼叫');
                      return;
                    }
                    // 跳转到通话页面（发起模式）
                    router.pushUrl({
                      url: 'pages/RoomPage',
                      params: {
                        mode: 'initiate'
                      }
                    });
                  });
                })

              Text(this.isOnline ? '点击发起快反呼叫' : '请等待连接就绪')
                .fontSize(12)
                .fontColor('#999999')
                .margin({ top: 8 })
            }
            .width('100%')
            .padding(20)
            .alignItems(HorizontalAlign.Center)
          }
          .width('100%')
        }
        .width('100%')
        .scrollBar(BarState.Off)
      }
      .onRefreshing(() => {
        this.refreshConnection();
      })
      .layoutWeight(1)
    }
    .width('100%')
    .height('100%')
    .backgroundColor('#F5F5F5')
    .onAppear(() => {
      if (!ApiClient.isLoggedIn) {
        router.replaceUrl({ url: 'pages/LoginPage' });
      }
    })
  }
}
