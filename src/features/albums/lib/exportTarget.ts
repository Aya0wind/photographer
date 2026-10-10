/**
 * 相册导出目标路径工具（M6 导出为文件夹，计划 2026-10-09 §六）：
 * - 库内判定：目标文件夹落在任一照片库 root 内时，导出物会被增量扫描识别为
 *   库内重复（硬链接 file-id 直跳 / 同库同哈希 skip）而「对库完全不可见」——
 *   不禁止导出，但对话框要提示「该文件夹会被扫描忽略，建议选择照片库外」。
 * - 上次导出位置：localStorage 记住，作为下次对话框的默认建议（库外优先）。
 *
 * 简化项：纯 UI 偏好不走 settingsStore（与语义搜索历史同模式）。
 */

import type { PhotoLibrary } from "@/ipc/api";

export const LAST_EXPORT_DIR_KEY = "photographer.albums.exportDir";

type StorageLike = Pick<Storage, "getItem" | "setItem" | "removeItem">;

/** 比较用路径规范化：统一分隔符为 \、去尾分隔符、大小写不敏感（Windows 主战场；
 *  macOS 默认卷同样大小写不敏感；Linux 精确卷误判为「库内」也只是多显示一条
 *  可继续的提示，不阻断任何操作） */
export function normalizePathForCompare(path: string): string {
  return path.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
}

/** child 是否在 root 内（或等于 root）：只做前缀包含判定，不碰文件系统 */
export function isPathInside(child: string, root: string): boolean {
  const c = normalizePathForCompare(child.trim());
  const r = normalizePathForCompare(root.trim());
  if (c === "" || r === "") return false;
  return c === r || c.startsWith(`${r}\\`);
}

/**
 * 找出包含 path 的照片库（root 等于或包含 path）；多库嵌套时返回最先登记的
 * 一个即可（提示语义只关心「在不在某个库内」，不区分哪个库）。
 */
export function findContainingLibrary(
  path: string,
  libraries: Pick<PhotoLibrary, "id" | "name" | "rootPath">[],
): Pick<PhotoLibrary, "id" | "name" | "rootPath"> | null {
  const target = path.trim();
  if (target === "") return null;
  for (const library of libraries) {
    if (isPathInside(target, library.rootPath)) return library;
  }
  return null;
}

/** 读取上次导出位置（默认建议值）；缺失/损坏/空串静默回退 null */
export function loadLastExportDir(storage: StorageLike = localStorage): string | null {
  try {
    const raw = storage.getItem(LAST_EXPORT_DIR_KEY);
    if (raw === null) return null;
    const parsed: unknown = JSON.parse(raw);
    return typeof parsed === "string" && parsed.trim() !== "" ? parsed : null;
  } catch {
    return null;
  }
}

/** 记住本次导出位置（导出成功启动时调用）；隐私模式/配额满静默忽略 */
export function saveLastExportDir(dir: string, storage: StorageLike = localStorage): void {
  const trimmed = dir.trim();
  if (trimmed === "") return;
  try {
    storage.setItem(LAST_EXPORT_DIR_KEY, JSON.stringify(trimmed));
  } catch {
    // 隐私模式/配额满：仅本次会话内有效
  }
}
