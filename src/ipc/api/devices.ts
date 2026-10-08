import { ipc } from "../index";
import { type DeviceSnapshot, type FileEntryDto, type FileKind, type FsDirEntry, type PlatformCapabilities } from "./types";

/** 扩展名 → 文件大类（与 Rust 侧 PHOTO_EXTS/RAW_EXTS 镜像） */
const EXT_KIND_TABLE: Record<string, FileKind> = {
  ...Object.fromEntries(
    ["jpg", "jpeg", "png", "heic", "heif", "avif", "tif", "tiff", "bmp", "gif", "webp", "jxl"].map(
      (e) => [e, "photo" as const],
    ),
  ),
  ...Object.fromEntries(
    ["cr2", "cr3", "nef", "arw", "raf", "dng", "orf", "rw2", "r3d", "iiq", "pef", "srw", "x3f", "nev"].map(
      (e) => [e, "raw" as const],
    ),
  ),
};

/** 按文件名判定大类（未知扩展 → other；与后端 classify 口径一致以扩展名为准） */
export function kindFromName(name: string): FileKind {
  const dot = name.lastIndexOf(".");
  if (dot < 0 || dot === name.length - 1) return "other";
  return EXT_KIND_TABLE[name.slice(dot + 1).toLowerCase()] ?? "other";
}

/** 读取原生能力快照；查询失败向调用方返回错误，避免把未知状态当作不支持。 */
export async function platformCapabilities(): Promise<PlatformCapabilities> {
  return ipc<PlatformCapabilities>("platform_capabilities");
}

/** 已连接设备列表（含各类型文件统计）；非数组回退空（防御后端异常返回） */
export async function deviceList(strict = false): Promise<DeviceSnapshot[]> {
  try {
    const devices = await ipc<DeviceSnapshot[] | null>("device_list");
    if (strict && !Array.isArray(devices)) throw new Error("设备列表响应无效");
    return Array.isArray(devices) ? devices : [];
  } catch (error) {
    if (strict) throw error;
    return [];
  }
}

/** 触发/刷新单设备扫描，返回最新快照 */
export async function deviceScan(id: string): Promise<DeviceSnapshot | null> {
  try {
    return await ipc<DeviceSnapshot>("device_scan", { id });
  } catch {
    return null;
  }
}

/** 仅可靠识别出的相机和存储卡禁止移动/引用；未知来源默认复制但可手动切换。 */
export async function deviceCopyOnly(id: string): Promise<boolean> {
  return ipc<boolean>("device_copy_only", { id });
}

/** 列出指定源的全部媒体文件（向导中央清单区；失败返回 null，调用方保持空态） */
export async function deviceFiles(id: string): Promise<FileEntryDto[] | null> {
  try {
    const files = await ipc<FileEntryDto[] | null>("device_files", { id });
    return Array.isArray(files) ? files : null;
  } catch {
    return null;
  }
}

/** 扫描本地文件夹作为导入源（kind="folder"，id="FOLDER:<绝对路径>"）；失败返回 null */
export async function folderScan(path: string, strict = false): Promise<DeviceSnapshot | null> {
  try {
    const snapshot = await ipc<DeviceSnapshot | null>("folder_scan", { path });
    return snapshot ?? null;
  } catch (error) {
    if (strict) throw error;
    return null;
  }
}

/** 懒加载目录列表：parent 省略 = 盘符根；失败/不可用/非数组默认返回 []；strict 时透传平台错误 */
export async function fsListDirs(parent?: string, strict = false): Promise<FsDirEntry[]> {
  try {
    const dirs = await ipc<FsDirEntry[] | null>("fs_list_dirs", parent ? { parent } : undefined);
    return Array.isArray(dirs) ? dirs : [];
  } catch (error) {
    if (strict) throw error;
    return [];
  }
}

/** 导入前读取相机提供的小预览，不复制整张原图。 */
export async function deviceThumbGet(deviceId: string, objectId: string, version: string, size: number): Promise<string | null> {
  return ipc<string | null>("device_thumb_get", { deviceId, objectId, version, size });
}
