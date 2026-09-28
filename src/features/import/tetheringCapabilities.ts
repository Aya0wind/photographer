import type { CameraInfo } from "@/ipc/api";

/**
 * 阶段 E-1 联拍能力映射（纯函数，DeviceDialog 消费）：
 * capabilities → chips 展示位 + 拍摄按钮门控 + 事件 pnpId 与设备 id 匹配。
 * UI 永远按后端探测报告渲染（评估文档 §3.4 原则），不做品牌名承诺。
 */

export type TetheringChipId = "fileTransfer" | "triggerCapture" | "autoIngest";

export interface TetheringChip {
  id: TetheringChipId;
  supported: boolean;
}

/** 可触发拍摄：Nikon 厂商码（0x90C0）与标准 InitiateCapture（0x100E）任一支持即可 */
export function canTriggerCapture(capabilities: CameraInfo["capabilities"]): boolean {
  return capabilities.vendorCaptureNikon || capabilities.standardCapture;
}

/** capabilities → chips（固定顺序：文件传输 / 可触发拍摄 / 自动收片）。
 *  liveView 属于后续阶段（L1 未验证），本轮 UI 不展示。 */
export function tetheringChips(capabilities: CameraInfo["capabilities"]): TetheringChip[] {
  return [
    { id: "fileTransfer", supported: capabilities.fileTransfer },
    { id: "triggerCapture", supported: canTriggerCapture(capabilities) },
    { id: "autoIngest", supported: capabilities.objectAddedEvents },
  ];
}

/** 事件 pnpId 与设备列表 id 是否同一台设备：WPD PnP id（\\?\ 前缀）大小写归一比较
 *  （与 importStore.normalizeDeviceId 同口径）。 */
export function sameCameraDevice(pnpId: string, deviceId: string): boolean {
  const norm = (s: string): string => (s.startsWith("\\\\?\\") ? s.toLowerCase() : s);
  return norm(pnpId) === norm(deviceId);
}
