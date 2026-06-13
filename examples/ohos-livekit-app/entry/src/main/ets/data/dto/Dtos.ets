/** 登录请求体 */
export class LoginRequest {
  username: string = '';
  password: string = '';

  constructor(username: string, password: string) {
    this.username = username;
    this.password = password;
  }
}

/** auth-admin 登录响应 */
export class AuthLoginResponse {
  access_token: string = '';
  username: string = '';
  display_name: string = '';
}

/** API 统一返回包装 */
export class ResultDTO<T> {
  status: number = 0;
  message: string = '';
  data: T | null = null;

  get isSuccess(): boolean {
    return this.status === 1;
  }
}

/** 用户信息 */
export class UserInfoDTO {
  id: string = '';
  userName: string = '';
  name: string = '';
  departmentId: string = '';
  departmentName: string = '';
  departmentCode: string = '';
}

/** 自定义配置模型（后端返回） */
export class CustomConfigModel {
  id: string = '';
  typeName: string = '';
  content: string = '';       // JSON字符串，需要二次解析
  departmentId: string = '';
  orderId: string = '';
}

/** MQTT 连接配置 */
export class MqttConfig {
  username: string = '';
  password: string = '';
  url: string = '';
}

/** 呼叫配置（从 CustomConfigModel.content 解析） */
export class CallConfigModel {
  enable: boolean = false;
  address: string = '';
  livekitUrl: string = '';
  broker: string = '';
  mqtt: MqttConfig | null = null;
}

/** API 统一返回包装（兼容 code/data 和 status/data 两种格式） */
export class RespVO<T> {
  code: string = '';
  status: number = 0;
  msg: string = '';
  message: string = '';
  data: T | null = null;

  /**
   * 检查 RespVO 是否成功。
   * 使用静态方法而非 getter，因为 JSON.parse() 返回的普通对象没有原型链，getter 不生效。
   * 兼容 code 字段为字符串（"SUCCESS"/"0"/"200"）或数字（0/200）两种情况，
   * 因为 JSON.parse 保留原始类型，不像 Gson 会自动将数字转为字符串。
   */
  static isOk(obj: RespVO<Object>): boolean {
    const code = obj.code;
    // 使用 String() 转换来兼容 code 可能为数字（0/200）或字符串（"0"/"200"）的情况
    return code === 'SUCCESS' || String(code) === '0' || String(code) === '200' || obj.status === 200;
  }
}

/** 客户端配置 */
export class ClientConfig {
  mqtt: MqttConfig | null = null;
  livekit: LiveKitConfig | null = null;
  call: Record<string, Object> | null = null;
}

export class LiveKitConfig {
  url: string = '';
  apiKey: string = '';
  apiSecret: string = '';
}

/** 快反级别 */
export class RrtLevel {
  levelCode: string = '';
  levelName: string = '';
  priority: number = 0;
  description: string = '';
}

/** 快反呼叫目标 */
export class RrtCallTargets {
  levelCode: string = '';
  levelName: string = '';
  serialNumberList: string[] = [];
  targetCount: number = 0;
}

/** 发起呼叫请求体 */
export class CallInitiateRequest {
  levelCode: string = '';
  callerDeviceSn: string = '';
  teamId: string = '';
  context: CallContextDto | null = null;
}

/** 呼叫上下文信息 */
export class CallContextDto {
  patientName: string = '';
  bedNo: string = '';
  department: string = '';
  location: string = '';
  triggerType: string = '';
  triggerValue: string = '';
  rrtType: string = '';
  description: string = '';
}

/** 发起呼叫响应体 */
export class CallInitiateResponse {
  callId: string = '';
  livekitRoom: string = '';
  livekitUrl: string = '';
  targets: string[] = [];
  timeoutSeconds: number = 30;
}

/** LiveKit Token 请求 */
export class LiveKitTokenRequest {
  room_name: string = '';
  identity: string = '';
  name: string = '';
}

/** LiveKit Token 响应 data */
export class LiveKitTokenData {
  token: string = '';
}
