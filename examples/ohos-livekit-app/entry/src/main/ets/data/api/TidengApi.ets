import { http } from '@kit.NetworkKit';
import { ApiClient } from './ApiClient';
import {
  RespVO, AuthLoginResponse, UserInfoDTO, ClientConfig,
  RrtLevel, RrtCallTargets, CallInitiateRequest, CallInitiateResponse,
  CustomConfigModel, LoginRequest, LiveKitTokenRequest, LiveKitTokenData
} from '../dto/Dtos';

/**
 * 提灯快反手机端 API 接口 — OHOS 版，使用 @ohos.net.http 替代 Retrofit。
 */
export class TidengApi {
  // ─── 认证 ───

  /** 登录 */
  async login(body: LoginRequest): Promise<RespVO<AuthLoginResponse>> {
    return ApiClient.request<RespVO<AuthLoginResponse>>(
      http.RequestMethod.POST, 'auth-admin/login', body);
  }

  /** 获取当前登录用户信息（含科室） */
  async getAuthenticationUser(): Promise<RespVO<UserInfoDTO>> {
    return ApiClient.request<RespVO<UserInfoDTO>>(
      http.RequestMethod.GET, 'tdssr/me');
  }

  /** 刷新 Token */
  async refreshToken(): Promise<RespVO<AuthLoginResponse>> {
    return ApiClient.request<RespVO<AuthLoginResponse>>(
      http.RequestMethod.POST, 'auth-admin/refresh');
  }

  // ─── 快反 (Rapid Response) ───

  /** 获取快反级别列表 */
  async getRrtLevels(teamId?: string): Promise<RespVO<RrtLevel[]>> {
    let path = 'tdssr/api/call-rules';
    if (teamId) {
      path += `?teamId=${encodeURIComponent(teamId)}`;
    }
    return ApiClient.request<RespVO<RrtLevel[]>>(
      http.RequestMethod.GET, path);
  }

  /** 获取快反呼叫目标 */
  async getRrtCallTargets(levelCode: string, teamId?: string): Promise<RespVO<RrtCallTargets>> {
    let path = `tdssr/api/call-rules/targets?levelCode=${encodeURIComponent(levelCode)}`;
    if (teamId) {
      path += `&teamId=${encodeURIComponent(teamId)}`;
    }
    return ApiClient.request<RespVO<RrtCallTargets>>(
      http.RequestMethod.GET, path);
  }

  /** 发起快反呼叫 */
  async initiateCall(body: CallInitiateRequest): Promise<RespVO<CallInitiateResponse>> {
    return ApiClient.request<RespVO<CallInitiateResponse>>(
      http.RequestMethod.POST, 'tdssr/api/calls/initiate', body);
  }

  /** 取消/挂断快反呼叫（释放房间锁） */
  async cancelCall(callId: string): Promise<RespVO<Object>> {
    return ApiClient.request<RespVO<Object>>(
      http.RequestMethod.POST, `tdssr/api/calls/${encodeURIComponent(callId)}/cancel`);
  }

  /** 获取 LiveKit Token（后端 JWT 签发，对齐 rrt-web getLiveKitToken） */
  async getLiveKitToken(roomName: string, identity: string, name?: string): Promise<string> {
    const body = new LiveKitTokenRequest();
    body.room_name = roomName;
    body.identity = identity;
    if (name) {
      body.name = name;
    }
    const resp = await ApiClient.request<RespVO<LiveKitTokenData>>(
      http.RequestMethod.POST, 'tdssr/api/livekit/token', body);
    const codeOk = resp.code === 'SUCCESS' || String(resp.code) === '0' || String(resp.code) === '200' || resp.status === 200;
    if (codeOk && resp.data && resp.data.token.length > 0) {
      return resp.data.token;
    }
    throw new Error(resp.msg || '获取 LiveKit token 失败');
  }

  // ─── 客户端配置 ───

  /** 获取客户端配置（MQTT / LiveKit / Call） */
  async getClientConfig(): Promise<RespVO<ClientConfig>> {
    return ApiClient.request<RespVO<ClientConfig>>(
      http.RequestMethod.GET, 'tdssr/api/client-config');
  }

  // ─── 自定义配置 ───

  /** 根据科室ID和配置名称查询自定义配置 */
  async getDeptTypeNames(departmentId: string, configNames: string[]): Promise<CustomConfigModel[]> {
    const names = configNames.map(n => encodeURIComponent(n)).join(',');
    const path =
      `nursingdoc/customconfig/depttypenames?departmentId=${encodeURIComponent(departmentId)}&configNames=${names}`;
    return ApiClient.request<CustomConfigModel[]>(
      http.RequestMethod.GET, path);
  }
}
