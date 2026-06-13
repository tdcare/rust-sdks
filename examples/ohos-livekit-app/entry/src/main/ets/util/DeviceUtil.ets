import { deviceInfo } from '@kit.BasicServicesKit';
import { cryptoFramework } from '@kit.CryptoArchitectureKit';
import { util } from '@kit.ArkTS';

/**
 * 设备工具类 — OHOS 版 SN 计算逻辑。
 * 通过 deviceInfo 的 serial + ODID 拼接后计算 SHA1 哈希，与后端标识保持一致。
 */
export class DeviceUtil {
  /** 缓存的设备 SN */
  private static cachedDeviceSn: string = '';

  /**
   * 获取设备唯一标识（SHA1 哈希）
   */
  static getDeviceSn(): string {
    if (DeviceUtil.cachedDeviceSn.length > 0) {
      return DeviceUtil.cachedDeviceSn;
    }

    const sb = new util.TextEncoder().encodeInto('');

    // 获取设备序列号
    try {
      const serial = deviceInfo.serial;
      if (serial && serial.length > 0) {
        DeviceUtil.cachedDeviceSn += serial + '|';
      }
    } catch (_) {
      // 某些设备可能无 serial
    }

    // 获取 ODID（Open Device Identifier）
    try {
      const odid = deviceInfo.ODID;
      if (odid && odid.length > 0) {
        DeviceUtil.cachedDeviceSn += odid;
      }
    } catch (_) {
      // 无 ODID 时忽略
    }

    // 如果成功获取到设备信息，计算 SHA1
    if (DeviceUtil.cachedDeviceSn.length > 0) {
      try {
        const hash = DeviceUtil.sha1(DeviceUtil.cachedDeviceSn);
        if (hash.length > 0) {
          DeviceUtil.cachedDeviceSn = hash;
          return DeviceUtil.cachedDeviceSn;
        }
      } catch (_) {
        // SHA1 失败，使用拼接原始值
      }
    }

    // 兜底：基于设备属性生成 UUID
    const fallback = DeviceUtil.buildFallbackId();
    DeviceUtil.cachedDeviceSn = DeviceUtil.sha1(fallback);
    return DeviceUtil.cachedDeviceSn;
  }

  /**
   * SHA1 哈希计算
   */
  private static sha1(data: string): string {
    const md = cryptoFramework.createMd('SHA1');
    const input: cryptoFramework.DataBlob = {
      data: new Uint8Array(new util.TextEncoder().encodeInto(data).buffer)
    };
    md.updateSync(input);
    const output = md.digestSync();
    const hex = DeviceUtil.toHexString(new Uint8Array(output.data));
    return hex.toUpperCase();
  }

  /**
   * 字节数组转十六进制字符串
   */
  private static toHexString(bytes: Uint8Array): string {
    let result = '';
    for (let i = 0; i < bytes.length; i++) {
      const hex = (bytes[i] & 0xFF).toString(16);
      result += hex.length === 1 ? '0' + hex : hex;
    }
    return result;
  }

  /**
   * 兜底：根据设备硬件属性生成伪 UUID
   */
  private static buildFallbackId(): string {
    try {
      const parts: string[] = [
        '3883756',
        String(deviceInfo.manufacture.length % 10),
        String(deviceInfo.brand.length % 10),
        String(deviceInfo.productModel.length % 10),
        String((deviceInfo.serial || '').length % 10)
      ];
      return parts.join('');
    } catch (_) {
      return 'ohos_fallback_device';
    }
  }
}
