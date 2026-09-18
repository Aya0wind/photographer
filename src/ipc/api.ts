import { listen } from "@tauri-apps/api/event";

import { ipc } from "./index";

/**
 * 后端命令的类型化封装（IPC 契约见 docs 设计文档 §5.9 / M1 T10-T12）。
 *
 * 所有封装都必须 catch：后端命令由并行任务实现，dev 预览/测试环境下
 * 尚不可用——列表/分页返回 []，标量返回 null，并置全局 ipcAvailable=false
 * （UI 可据此显示“后端未连接”提示）。参数键名按 Tauri 惯例传 camelCase，
 * 由 Tauri 自动映射到 Rust 侧 snake_case 形参。
 */

// --- 契约类型 ----------------------------------------------------------------

export type DeviceKind = "volume" | "mtp" | "folder";
export type FileKind = "photo" | "raw" | "video" | "other";

export interface DeviceSnapshot {
  id: string;
  name: string;
  kind: DeviceKind;
  filesByKind: Record<FileKind, number>;
  bytesTotal: number;
  newFiles: number;
}

export type DuplicatePolicy = "skip" | "rename" | "ask";

/** 导入模式：copy=保留原文件（复制），move=入库后删除源（纳管已有照片） */
export type ImportMode = "copy" | "move";

export interface ImportPlan {
  sourceId: string;
  targetRoot: string;
  dirTemplate: string;
  nameTemplate: string;
  duplicatePolicy: DuplicatePolicy;
  skipImported: boolean;
  streams: number;
  /** 缺省 copy（Rust 侧默认）；move 时后端入库后删除源文件 */
  mode: ImportMode;
}

export type JobStatus = "running" | "paused" | "done" | "cancelled" | "failed";
export type LogLevel = "error" | "warn" | "info";

export interface JobRow {
  id: number;
  kind: string;
  deviceId: string;
  deviceName: string;
  status: JobStatus;
  totalFiles: number;
  totalBytes: number;
  statsJson: string;
  startedAt: number;
  finishedAt: number | null;
}

export interface LogRow {
  id: number;
  ts: number;
  level: LogLevel;
  jobId: number;
  message: string;
}

export interface ImportStats {
  totalFiles: number;
  doneFiles: number;
  skippedDuplicates: number;
  failedFiles: number;
  totalBytes: number;
  doneBytes: number;
  elapsedMs: number;
  bytesPerSec: number;
  /** 移动模式：成功移动（=复制后删除源）的文件数；copy 任务缺失（契约扩展中） */
  moved?: number;
  /** 移动模式：源文件删除失败数；copy 任务缺失（契约扩展中） */
  sourceDeleteFailed?: number;
}

/** 唯一事件通道 `app://event` 的 payload：以 type（camelCase）辨识的联合 */
export type AppEvent =
  | { type: "deviceArrived"; id: string; kind: DeviceKind; name: string }
  | { type: "deviceRemoved"; id: string }
  | {
      type: "deviceScanned";
      id: string;
      name: string;
      kind: DeviceKind;
      snapshot: DeviceSnapshot;
    }
  | { type: "importSessionStarted"; jobId: number; totalFiles: number; totalBytes: number }
  | {
      type: "importFileProgress";
      jobId: number;
      doneFiles: number;
      doneBytes: number;
      currentFile: string;
      bytesPerSec: number;
    }
  | { type: "importPaused"; jobId: number }
  | { type: "importResumed"; jobId: number }
  | { type: "importCancelled"; jobId: number }
  | { type: "importSessionFinished"; jobId: number; stats: ImportStats }
  | { type: "importFileCompleted"; jobId: number; src: string; dst: string; state: string }
  | { type: "appError"; level: string; message: string; recoverable: boolean };

// --- IPC 可用性（自愈式，唯一定义在 ./index） -----------------------------------
export { isIpcAvailable, resetIpcAvailable } from "./index";

// --- 命令封装 ----------------------------------------------------------------

/** 设备文件条目 DTO（device_files 返回；relPath 为设备内相对路径，"/" 分隔） */
export interface FileEntryDto {
  id: string;
  relPath: string;
  size: number;
  mtime: string;
}

/** 扩展名 → 文件大类（与 Rust 侧 PHOTO_EXTS/RAW_EXTS/VIDEO_EXTS 镜像） */
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
  ...Object.fromEntries(
    ["mp4", "mov", "avi", "mkv", "mts", "m2ts", "wmv", "3gp", "avchd"].map((e) => [
      e,
      "video" as const,
    ]),
  ),
};

/** 按文件名判定大类（未知扩展 → other；与后端 classify 口径一致以扩展名为准） */
export function kindFromName(name: string): FileKind {
  const dot = name.lastIndexOf(".");
  if (dot < 0 || dot === name.length - 1) return "other";
  return EXT_KIND_TABLE[name.slice(dot + 1).toLowerCase()] ?? "other";
}

/** 已连接设备列表（含各类型文件统计） */
export async function deviceList(): Promise<DeviceSnapshot[]> {
  try {
    return await ipc<DeviceSnapshot[]>("device_list");
  } catch {
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
export async function folderScan(path: string): Promise<DeviceSnapshot | null> {
  try {
    const snapshot = await ipc<DeviceSnapshot | null>("folder_scan", { path });
    return snapshot ?? null;
  } catch {
    return null;
  }
}

/** 文件系统目录树的单个节点（hasSubdirs=false 时无子目录、不显示展开箭头） */
export interface FsDirEntry {
  name: string;
  path: string;
  hasSubdirs: boolean;
}

/** 懒加载目录列表：parent 省略 = 盘符根；失败/不可用/非数组均返回 []（静默降级） */
export async function fsListDirs(parent?: string): Promise<FsDirEntry[]> {
  try {
    const dirs = await ipc<FsDirEntry[] | null>("fs_list_dirs", parent ? { parent } : undefined);
    return Array.isArray(dirs) ? dirs : [];
  } catch {
    return [];
  }
}

/** 按方案启动导入会话，返回 jobId；失败返回 null */
export async function importStart(plan: ImportPlan): Promise<number | null> {
  try {
    return await ipc<number>("import_start", { plan });
  } catch {
    return null;
  }
}

/** 暂停任务（无返回值；失败静默并标记 IPC 不可用） */
export async function importPause(jobId: number): Promise<void> {
  try {
    await ipc<void>("import_pause", { jobId });
  } catch {
  }
}

export async function importResume(jobId: number): Promise<void> {
  try {
    await ipc<void>("import_resume", { jobId });
  } catch {
  }
}

export async function importCancel(jobId: number): Promise<void> {
  try {
    await ipc<void>("import_cancel", { jobId });
  } catch {
  }
}

/** 历史任务游标分页（afterId 升序取下一页） */
export async function importJobsPage(afterId: number, limit: number): Promise<JobRow[]> {
  try {
    return await ipc<JobRow[]>("import_jobs_page", { afterId, limit });
  } catch {
    return [];
  }
}

/** 单任务日志游标分页 */
export async function importLogsPage(
  jobId: number,
  afterId: number,
  limit: number,
): Promise<LogRow[]> {
  try {
    return await ipc<LogRow[]>("import_logs_page", { jobId, afterId, limit });
  } catch {
    return [];
  }
}

/** 重试任务的全部失败文件，返回新 jobId；失败返回 null */
export async function importRetryFailed(jobId: number): Promise<number | null> {
  try {
    return await ipc<number>("import_retry_failed", { jobId });
  } catch {
    return null;
  }
}

// --- 事件订阅 ----------------------------------------------------------------

/**
 * 订阅唯一事件通道 `app://event`。
 * @returns 取消订阅函数；非 Tauri 环境下（vite dev 预览）返回 noop。
 */
export async function subscribeAppEvents(
  handler: (event: AppEvent) => void,
): Promise<() => void> {
  try {
    return await listen<AppEvent>("app://event", (e) => handler(e.payload));
  } catch {
    return () => {};
  }
}
