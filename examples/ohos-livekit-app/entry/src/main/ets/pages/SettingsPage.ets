import { router } from '@kit.ArkUI';
import { ApiClient } from '../data/api/ApiClient';
import { SessionManager } from '../data/session/SessionManager';

@Entry
@Component
struct SettingsPage {
  @State apiServerUrl: string = '';
  @State message: string = '';

  aboutToAppear(): void {
    ApiClient.getBaseUrl().then((url: string) => {
      this.apiServerUrl = url;
    });
  }

  async save(): Promise<void> {
    const url = this.apiServerUrl.trim();
    if (url.length === 0) {
      this.message = '请输入服务器地址';
      return;
    }

    // 确保 URL 以 / 结尾
    const normalizedUrl = url.endsWith('/') ? url : url + '/';
    await ApiClient.setBaseUrl(normalizedUrl);
    this.message = '设置已保存';
    setTimeout(() => {
      router.back();
    }, 500);
  }

  async logout(): Promise<void> {
    await SessionManager.logout();
    router.replaceUrl({ url: 'pages/LoginPage' });
  }

  build() {
    Column() {
      // 标题栏
      Row() {
        Button('← 返回')
          .fontSize(14)
          .backgroundColor('#00000000')
          .fontColor('#007AFF')
          .onClick(() => {
            router.back();
          })
        Text('设置')
          .fontSize(20)
          .fontWeight(FontWeight.Bold)
          .layoutWeight(1)
          .textAlign(TextAlign.Center)
        Blank()
          .width(60)
      }
      .width('100%')
      .padding(16)

      Divider()

      // API 服务器地址
      Column() {
        Text('API 服务器地址')
          .fontSize(14)
          .fontColor('#666666')
          .margin({ bottom: 8 })

        TextInput({ text: this.apiServerUrl, placeholder: 'https://api.example.com/' })
          .width('100%')
          .type(InputType.URL)
          .onChange((value: string) => {
            this.apiServerUrl = value;
          })
      }
      .width('100%')
      .padding(16)
      .alignItems(HorizontalAlign.Start)

      // 保存按钮
      Button('保存设置')
        .width('90%')
        .margin({ top: 24, bottom: 12 })
        .onClick(() => {
          this.save();
        })

      // 退出登录按钮
      Button('退出登录')
        .width('90%')
        .backgroundColor('#FF4444')
        .margin({ top: 12 })
        .onClick(() => {
          this.logout();
        })

      if (this.message.length > 0) {
        Text(this.message)
          .fontSize(14)
          .fontColor(this.message.includes('失败') ? '#FF4444' : '#4CAF50')
          .margin({ top: 16 })
      }

      Blank()
    }
    .width('100%')
    .height('100%')
    .backgroundColor('#F5F5F5')
  }
}
