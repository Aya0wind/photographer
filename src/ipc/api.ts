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
  scanStatus?: "scanning" | "ready" | "failed";
  scanError?: string | null;
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
  /** 缺省 copy（Rust 侧默认）；move 时入库后删除源文件 */
  mode: ImportMode;
  /** 双目的地（可选）：一次读取同时复制到第二位置；目录模板与主目的地相同。
   *  后端约束：move + secondTarget 会被拒绝（前端互斥保证不发出）。 */
  secondTarget?: { targetRoot: string; dirTemplate: string };
  /** 本次导入的文件清单（rel_path 列表）——向导勾选结果，引擎只导入集合内的文件；
   *  省略 = 全部（历史计划兼容）。 */
  include?: string[];
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
  | { type: "deviceFilesProgress"; id: string; files: FileEntryDto[] }
  | { type: "deviceScanFailed"; id: string; message: string }
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
      /** 进度条口径：完成+跳过+失败的已结算字节（skip 不推进 doneBytes） */
      settledBytes: number;
      currentFile: string;
      bytesPerSec: number;
    }
  | { type: "importPaused"; jobId: number }
  | { type: "importResumed"; jobId: number }
  | { type: "importCancelled"; jobId: number }
  | { type: "importSessionFinished"; jobId: number; stats: ImportStats }
  | { type: "importFileCompleted"; jobId: number; src: string; dst: string; state: string }
  | { type: "cleanStarted"; jobId: number; count: number; bytes: number }
  | { type: "cleanFinished"; jobId: number; stats: CleanResultDto }
  | { type: "thumbnailReady"; assetId: number; size: number; path: string }
  | { type: "appError"; level: string; message: string; recoverable: boolean };

// --- M3 画廊/搜索/查看器契约 ------------------------------------------------------

/** 库内资产大类（与导入侧 FileKind 对齐，不含 other——入库文件必属其一） */
export type AssetKind = "photo" | "raw" | "video";

/** 库内资产（assets_page 返回；按 capturedAt DESC 排列，capturedAt 为 NULL 的排最前） */
export interface AssetDto {
  id: number;
  /** 库内绝对路径 */
  path: string;
  name: string;
  kind: AssetKind;
  /** EXIF 拍摄时间（ISO 8601）；EXIF 缺失为 null——前端归「未知日期」组（组序最前） */
  capturedAt: string | null;
  camera: string | null;
  sizeBytes: number;
}

/** 搜索/过滤条件（camelCase 平铺进 assets_page 负载；全字段可省略） */
export interface AssetFilters {
  kind?: AssetKind;
  /** "YYYY-MM-DD"（含当日，由后端解释） */
  capturedAfter?: string;
  capturedBefore?: string;
  /** 相机名子串（不区分大小写，后端解释） */
  camera?: string;
}

/** 日期分组统计（asset_group_dates 返回，chips 条数据源；未知日期组 date=null 排最前） */
export interface AssetGroupDate {
  date: string | null;
  count: number;
  coverAssetId: number;
}

/** 单资产全量元数据（asset_detail 返回，查看器 EXIF 面板）；dupCount=库内内容指纹重复数 */
export interface AssetDetailDto {
  id: number;
  path: string;
  name: string;
  kind: AssetKind;
  capturedAt: string | null;
  camera: string | null;
  lens: string | null;
  sizeBytes: number;
  importedAt: string | null;
  dupCount: number;
  width: number | null;
  height: number | null;
  iso: number | null;
  aperture: number | null;
  shutter: string | null;
  focalLength: number | null;
}

// --- 安全清卡（M2）：候选预览 → 强确认 → 后端逐文件指纹复验后删除 ---------------

/** 可清理源文件（clean_candidates 返回；已入库且指纹匹配的源文件） */
export interface CleanCandidateDto {
  /** 源文件绝对路径 */
  src: string;
  /** 库内相对路径 */
  relPath: string;
  size: number;
  assetId: string | null;
}

/** 清卡结果（clean_apply 返回 / cleanFinished 事件） */
export interface CleanResultDto {
  deleted: number;
  failed: number;
  freedBytes: number;
  errors: string[];
}

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

/** import_start 结果：ok=false 时 error 为后端 Err 文案；error=null 表示 invoke 本身不可用（调用方显示通用文案） */
export type ImportStartResult = { ok: true; jobId: number } | { ok: false; error: string | null };

/** invoke 不可用类错误（非 Tauri 环境/命令未注册）：error=null，调用方显示通用文案 */
const INVOKE_UNAVAILABLE_PATTERN = /__TAURI_INTERNALS__|invoke is not available|command [^\s]+ not found/i;

/** 按方案启动导入会话；后端逻辑错误（如目标目录嵌套守卫）透出原始 Err 文案。
 *  可用性标志由 ipc() 统一维护（自愈式），此处不再手动置位。 */
export async function importStart(plan: ImportPlan): Promise<ImportStartResult> {
  try {
    const jobId = await ipc<number>("import_start", { plan });
    if (typeof jobId !== "number" || !Number.isFinite(jobId)) {
      return { ok: false, error: null };
    }
    return { ok: true, jobId };
  } catch (err) {
    const message = err instanceof Error ? err.message : typeof err === "string" ? err : "";
    if (!message || INVOKE_UNAVAILABLE_PATTERN.test(message)) {
      return { ok: false, error: null };
    }
    return { ok: false, error: message };
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

/** 清卡候选预览（该任务已入库且指纹匹配的源文件）；失败/无候选返回 [] */
export async function cleanCandidates(jobId: number): Promise<CleanCandidateDto[]> {
  try {
    const list = await ipc<CleanCandidateDto[]>("clean_candidates", { jobId });
    return Array.isArray(list) ? list : [];
  } catch {
    return [];
  }
}

/** 执行清卡（后端删除前逐文件复验指纹）；进行中/完成态由 cleanStarted/cleanFinished 事件驱动。
 *  返回值仅作兜底（命令失败返回 null），UI 状态以事件为准。 */
export async function cleanApply(jobId: number): Promise<CleanResultDto | null> {
  try {
    return await ipc<CleanResultDto>("clean_apply", { jobId });
  } catch {
    return null;
  }
}

/** 取后端缓存缩略图文件路径（JPG/PNG 等可生成；RAW/视频返回 null）；命令失败静默 null。
 *  @param size 期望边长（px），如 256；实际以缓存档位就近为准 */
export async function thumbGet(path: string, size: number): Promise<string | null> {
  try {
    return await ipc<string | null>("thumb_get", { path, size });
  } catch {
    return null;
  }
}

// --- M3 画廊命令封装 --------------------------------------------------------------

/** 画廊/搜索分页（keyset：afterId=上一页最后一条资产 id，首页传 0；capturedAt DESC，
 *  NULL capturedAt 排最前——排序由后端负责，前端分组渲染按未知组最前处理）。
 *  filters 平铺为 camelCase 负载字段；失败/非数组回退 []。 */
export async function assetsPage(
  afterId: number,
  limit: number,
  filters?: AssetFilters,
): Promise<AssetDto[]> {
  try {
    const list = await ipc<AssetDto[] | null>("assets_page", { afterId, limit, ...(filters ?? {}) });
    return Array.isArray(list) ? list : [];
  } catch {
    return [];
  }
}

/** 日期分组统计（画廊顶部日期 chips 条）；未知日期组 date=null，后端排最前 */
export async function assetGroupDates(): Promise<AssetGroupDate[]> {
  try {
    const list = await ipc<AssetGroupDate[] | null>("asset_group_dates");
    return Array.isArray(list) ? list : [];
  } catch {
    return [];
  }
}

/** 单资产全量元数据（查看器 EXIF 面板）；命令失败/不存在返回 null */
export async function assetDetail(id: number): Promise<AssetDetailDto | null> {
  try {
    return await ipc<AssetDetailDto | null>("asset_detail", { id });
  } catch {
    return null;
  }
}

/** 库内资产缩略图（按 assetId 取后端缓存文件绝对路径，调用方自行 convertFileSrc）。
 *  与向导的 thumbGet（按源文件路径，导入前预览用）是两个命令：本命令为 asset_thumb_get。
 *  size 为期望边长（画廊网格 240 / 查看器大图 1280），后端 snap 到 256/512 档就近返回；
 *  RAW（NEF/ARW 等）与视频恒返回 null——调用方按 kind 短路为永久占位，不进管线。 */
export async function assetThumbGet(assetId: number, size: number): Promise<string | null> {
  try {
    return await ipc<string | null>("asset_thumb_get", { assetId, size });
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
