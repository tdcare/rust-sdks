import { router } from '@kit.ArkUI';
import { vibrator } from '@kit.SensorServiceKit';
import { RrtCallManager } from '../rrt/RrtCallManager';

const TAG = 'IncomingCallPage';

/**
 * 来电界面 — 收到快反呼叫时弹出，可接听或拒绝。
 * 支持展示呼叫上下文（快反级别、科室、床号、患者姓名、快反类型、位置）。
 */

// 路由参数接口
interface IncomingCallParams {
  fromSn: string;
  roomId: string;
  msg: string;
  callId: string;
  livekitRoom: string;
  contextJson: string;
}

@Entry
@Component
struct IncomingCallPage {
  @State fromSn: string = '';
  @State roomId: string = '';
  @State msg: string = '';
  @State callId: string = '';
  @State livekitRoom: string = '';
  @State contextJson: string = '';

  // 呼叫上下文展示
  @State levelText: string = '';
  @State levelColor: string = '#FFFFFF';
  @State rrtType: string = '';
  @State locationText: string = '';
  @State patientText: string = '';
  @State showContext: boolean = false;

  private vibrationId: number = -1;

  aboutToAppear(): void {
    // 从路由参数获取来电信息
    const params = router.getParams() as IncomingCallParams;
    if (params) {
      this.fromSn = params.fromSn;
      this.roomId = params.roomId;
      this.msg = params.msg || '快反呼叫';
      this.callId = params.callId;
      this.livekitRoom = params.livekitRoom;
      this.contextJson = params.contextJson;
    }

    console.info(`${TAG}: 来电 fromSn=${this.fromSn}, roomId=${this.roomId}, callId=${this.callId}`);

    this.parseCallContext();
    this.startVibration();
  }

  aboutToDisappear(): void {
    this.stopVibration();
  }

  /**
   * 解析呼叫上下文 JSON 并展示
   */
  parseCallContext(): void {
    if (!this.contextJson || this.contextJson.length === 0) {
      this.showContext = false;
      return;
    }

    try {
      const ctx = JSON.parse(this.contextJson) as Record<string, string>;
      const levelCode = ctx['levelCode'] || '';
      const levelName = ctx['levelName'] || '';
      const department = ctx['department'] || '';
      const bedNo = ctx['bedNo'] || '';
      const patientName = ctx['patientName'] || '';
      const rrtType = ctx['rrtType'] || '';
      const location = ctx['location'] || '';

      // 级别标签
      if (levelName.length > 0 || levelCode.length > 0) {
        this.levelText = levelName.length > 0 ? levelName : levelCode;
        switch (levelCode.toUpperCase()) {
          case 'RED':
            this.levelColor = '#FF1744';
            break;
          case 'YELLOW':
            this.levelColor = '#FFD600';
            break;
          case 'BLUE':
            this.levelColor = '#2979FF';
            break;
          default:
            this.levelColor = '#FFFFFF';
        }
      }

      // 快反类型
      this.rrtType = rrtType;

      // 科室 + 位置
      const locationParts: string[] = [];
      if (department.length > 0) {
        locationParts.push(department);
      }
      if (location.length > 0) {
        locationParts.push(location);
      }
      this.locationText = locationParts.join(' · ');

      // 床号 + 患者
      const patientParts: string[] = [];
      if (bedNo.length > 0) {
        patientParts.push(`${bedNo}床`);
      }
      if (patientName.length > 0) {
        patientParts.push(patientName);
      }
      this.patientText = patientParts.join(' ');

      this.showContext = true;
    } catch (e) {
      console.error(`${TAG}: 解析呼叫上下文异常: ${e}`);
      this.showContext = false;
    }
  }

  /**
   * 启动振动（长震-短停循环）
   */
  startVibration(): void {
    try {
      const pattern: number[] = [1000, 500, 1000, 500]; // 振动时长数组
      vibrator.startVibration({
        type: 'time',
        duration: -1, // 持续振动直到停止
      }, {
        usage: 'alarm'
      });
    } catch (e) {
      console.warn(`${TAG}: 启动振动失败: ${e}`);
    }
  }

  stopVibration(): void {
    try {
      vibrator.stopVibration();
    } catch (e) {
      // ignore
    }
  }

  acceptCall(): void {
    this.stopVibration();
    const callManager = RrtCallManager.getInstance();
    callManager.acceptCall(this.fromSn, this.roomId, this.callId, this.livekitRoom);

    // 跳转到通话页面
    router.replaceUrl({
      url: 'pages/RoomPage',
      params: {
        mode: 'answer',
        fromSn: this.fromSn,
        roomId: this.roomId,
        callId: this.callId,
        livekitRoom: this.livekitRoom
      }
    });
  }

  rejectCall(): void {
    this.stopVibration();
    const callManager = RrtCallManager.getInstance();
    callManager.rejectCall(this.fromSn, this.roomId);
    router.back();
  }

  build() {
    Column() {
      // 标题
      Text('快反来电')
        .fontSize(28)
        .fontWeight(FontWeight.Bold)
        .fontColor('#FFFFFF')
        .margin({ top: 60 })

      // 来电者信息
      Text(`来自: ${this.fromSn}`)
        .fontSize(16)
        .fontColor('#CCCCCC')
        .margin({ top: 8 })

      Text(this.msg)
        .fontSize(20)
        .fontColor('#FFFFFF')
        .fontWeight(FontWeight.Medium)
        .margin({ top: 16 })

      // 呼叫上下文
      if (this.showContext) {
        Column() {
          // 级别标签
          if (this.levelText.length > 0) {
            Text(this.levelText)
              .fontSize(24)
              .fontWeight(FontWeight.Bold)
              .fontColor(this.levelColor)
              .margin({ bottom: 8 })
          }

          // 快反类型
          if (this.rrtType.length > 0) {
            Text(`快反类型: ${this.rrtType}`)
              .fontSize(14)
              .fontColor('#CCCCCC')
              .margin({ bottom: 4 })
          }

          // 位置
          if (this.locationText.length > 0) {
            Text(this.locationText)
              .fontSize(14)
              .fontColor('#CCCCCC')
              .margin({ bottom: 4 })
          }

          // 患者信息
          if (this.patientText.length > 0) {
            Text(this.patientText)
              .fontSize(16)
              .fontWeight(FontWeight.Medium)
              .fontColor('#FFFFFF')
          }
        }
        .width('80%')
        .padding(16)
        .margin({ top: 24 })
        .borderRadius(12)
        .backgroundColor('rgba(255,255,255,0.1)')
        .alignItems(HorizontalAlign.Center)
      }

      Blank()

      // 操作按钮
      Row({ space: 40 }) {
        // 拒绝按钮
        Button() {
          Column() {
            Text('拒绝')
              .fontSize(18)
              .fontWeight(FontWeight.Bold)
              .fontColor('#FFFFFF')
          }
          .width(80)
          .height(80)
          .justifyContent(FlexAlign.Center)
        }
        .width(80)
        .height(80)
        .borderRadius(40)
        .backgroundColor('#FF4444')
        .onClick(() => {
          this.rejectCall();
        })

        // 接听按钮
        Button() {
          Column() {
            Text('接听')
              .fontSize(18)
              .fontWeight(FontWeight.Bold)
              .fontColor('#FFFFFF')
          }
          .width(80)
          .height(80)
          .justifyContent(FlexAlign.Center)
        }
        .width(80)
        .height(80)
        .borderRadius(40)
        .backgroundColor('#4CAF50')
        .onClick(() => {
          this.acceptCall();
        })
      }
      .justifyContent(FlexAlign.Center)
      .margin({ bottom: 60 })
    }
    .width('100%')
    .height('100%')
    .backgroundColor('#1A1A2E')
    .padding(20)
  }
}
