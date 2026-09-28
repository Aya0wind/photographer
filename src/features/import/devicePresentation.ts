import type { DeviceSnapshot } from "@/ipc/api";

export type DevicePresentationKind = "reader" | "camera" | "folder";

const STORAGE_NAME =
  /(存储卡|储存卡|记忆卡|读卡器|\bsd(?:hc|xc)?\b|cfexpress|compact\s*flash|memory\s*card|card\s*reader|mass\s*storage|removable\s*(?:disk|storage)|usb\s*(?:drive|storage))/i;

/** 连接协议和用户看到的设备类型不是一回事：部分存储卡也通过 MTP 暴露。 */
export function devicePresentationKind(
  device: Pick<DeviceSnapshot, "kind" | "name">,
): DevicePresentationKind {
  if (device.kind === "folder") return "folder";
  if (device.kind === "volume" || STORAGE_NAME.test(device.name) || /^[a-z]:[\\/]?$/i.test(device.name.trim())) return "reader";
  return "camera";
}

export function deviceKindLabelKey(
  device: Pick<DeviceSnapshot, "kind" | "name">,
): string {
  return `deviceDialog.kind.${devicePresentationKind(device)}`;
}
