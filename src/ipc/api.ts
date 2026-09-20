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
  /** 索引任务启动恢复（库级后台：缩略图三档/EXIF 深提取/未来 AI）；pending=剩余项数 */
  | { type: "indexTaskResumed"; pending: number }
  /** 索引任务进度（kind="ai" 语义索引 / 其余为缩略图等）；节流由后端负责 */
  | { type: "indexTaskProgress"; kind: string; done: number; total: number }
  /** AI 模型下载进度（单模型，节流 1s） */
  | { type: "aiModelDownloadProgress"; id: string; doneBytes: number; totalBytes: number }
  /** AI 模型下载结束（ok=false 时 error 为原因文案） */
  | { type: "aiModelDownloadFinished"; id: string; ok: boolean; error?: string | null }
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
  /** RAW+JPG 配对 id（同一拍摄的两格式同值）；后端契约扩展中，缺省/单条均不成对 */
  pairId?: number | null;
  /** 像素宽（EXIF 深提取回填，~95% 资产有值）；缺失时前端 justify 网格按 4:3 兜底 */
  width?: number | null;
  /** 像素高（同上） */
  height?: number | null;
  /** 入库时间（ISO 8601；recent_assets 用于「最近添加」范围过滤，assets_page 不带） */
  createdAt?: string | null;
  /** 连拍组 id（M6：时间链×pHash 聚类；同组连续资产在画廊折叠为堆叠卡） */
  burstId?: number | null;
  /** 连拍组员数（照片数，含封面；仅 ≥2 时携带） */
  burstCount?: number | null;
}

/** 相机型号计数（cameras_list 返回，搜索页相机勾选数据源；按 count 降序） */
export interface AssetCameraCount {
  camera: string;
  count: number;
}

/** 镜头型号计数（lens_list 返回，搜索页镜头勾选数据源；按 count 降序） */
export interface AssetLensCount {
  lens: string;
  count: number;
}

/** 文件格式计数（format_list 返回，搜索页格式勾选数据源；如 NEF/ARW/JPG） */
export interface AssetFormatCount {
  format: string;
  count: number;
}

/** 搜索/过滤条件（camelCase 平铺进 assets_page 负载；全字段可省略） */
export interface AssetFilters {
  /** 类型集合（两档语义：照片=photo+raw，视频=video）；省略=全部。
   *  M3 二轮起用 kinds 数组（后端 lane 同步加 Vec<AssetKind> 参数），替代单值 kind */
  kinds?: AssetKind[];
  /** RFC3339（后端按绝对时间归一比较）；前端由 "YYYY-MM-DD" 本地日界转 UTC */
  capturedAfter?: string;
  capturedBefore?: string;
  /** 相机型号多选：所选值精确匹配（OR）；无相机信息的资产不会命中 */
  cameras?: string[];
  // --- M4 扩展筛选（后端 lane 契约扩展中；全 Optional，后端未实现时不传） ---
  /** 镜头多选 OR */
  lenses?: string[];
  /** 文件格式多选 OR（如 NEF/ARW/JPG） */
  formats?: string[];
  /** 焦距区间（mm，含端点） */
  focalMin?: number;
  focalMax?: number;
  /** ISO 区间（含端点） */
  isoMin?: number;
  isoMax?: number;
  /** 光圈 f 值区间（如 2.8-5.6，含端点） */
  apertureMin?: number;
  apertureMax?: number;
  /** 快门区间（秒，如 0.004=1/250s，含端点） */
  shutterMin?: number;
  shutterMax?: number;
  /** 闪光灯是否闪光（true=开/false=关；省略=不过滤） */
  /** 闪光灯三态："on"=闪光 | "off"=已知未闪光 | "unknown"=无信息 */
  flash?: "on" | "off" | "unknown";
  /** 拍摄方向（按 EXIF orientation 归类）；省略=不过滤 */
  orientation?: "landscape" | "portrait";
  /** 是否有 GPS 坐标（true=有/false=无；省略=不过滤） */
  hasGps?: boolean;
  /** 文件大小区间（字节，含端点） */
  sizeMin?: number;
  sizeMax?: number;
}

/** 日期分组统计（asset_group_dates 返回，chips 条数据源；未知日期组 date=null 排最前） */
export interface AssetGroupDate {
  date: string | null;
  count: number;
  coverAssetId: number;
}

/**
 * 单资产全量元数据（asset_detail 返回，查看器 EXIF 面板）。
 * 字段名与后端实测对齐（src-tauri assets.rs：{id, flatten(AssetRow), duplicate_count}），
 * 由 assetDetail() 归一为 camelCase：后端 flatten 的 AssetRow 是 filename/size/createdAt，
 * 顶层 duplicate_count 未做 camelCase 重命名；EXIF 扩展字段（宽高/ISO/光圈/快门/焦距）
 * 后端暂未返回（契约扩展中），存在即透出、缺失为 null/undefined。
 */
export interface AssetDetailDto {
  id: number;
  path: string;
  /** 文件名（后端 AssetRow.filename） */
  filename: string;
  /** 文件大小（字节；后端 AssetRow.size） */
  size: number;
  kind: AssetKind;
  capturedAt: string | null;
  camera: string | null;
  /** 镜头 */
  lens?: string | null;
  /** 入库时间（后端 AssetRow.createdAt） */
  createdAt: string | null;
  /** 库内同指纹重复数（不含自身；后端 duplicate_count 归一） */
  dupCount: number;
  // --- EXIF 扩展（M4 契约扩展；未返回时缺省，面板按缺失隐藏行/组） ---
  width?: number | null;
  height?: number | null;
  /** 总像素（百万，如 24.3） */
  megapixels?: number | null;
  /** 长宽比（如 "3:2"） */
  aspect?: string | null;
  /** EXIF 方向 1-8（5-8=竖拍；1-4=横拍，含镜像） */
  orientation?: number | null;
  /** 快门（如 "1/250"；面板追加 s 展示） */
  shutter?: string | null;
  /** 光圈 f 值 */
  /** 光圈/焦距为展示态字符串（"2.8"/"59"），查看器直接拼 f//mm */
  aperture?: string | null;
  focalLength?: string | null;
  iso?: number | null;
  /** 闪光灯（后端翻译后的文案，如「闪光」/「未闪光」） */
  flash?: string | null;
  meteringMode?: string | null;
  whiteBalance?: string | null;
  exposureProgram?: string | null;
  software?: string | null;
  artist?: string | null;
  gpsLat?: number | null;
  gpsLon?: number | null;
  /** 文件格式（扩展名大写，如 "NEF"） */
  format?: string | null;
  /** 评分 0-5（0=未评；查看器星标条） */
  rating?: number | null;
  /** 旗标（待整理标记） */
  flagged?: boolean | null;
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
    const list = await ipc<AssetDto[] | null>(
      "assets_page",
      filters && Object.keys(filters).length > 0 ? { afterId, limit, filters } : { afterId, limit },
    );
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

/** 库内相机型号清单（搜索页相机勾选；按 count 降序）；失败/非数组回退 [] */
export async function cameraList(): Promise<AssetCameraCount[]> {
  try {
    const list = await ipc<AssetCameraCount[] | null>("camera_list");
    return Array.isArray(list) ? list : [];
  } catch {
    return [];
  }
}

/** 库内镜头清单（搜索页镜头勾选；按 count 降序）；失败/非数组回退 [] */
export async function lensList(): Promise<AssetLensCount[]> {
  try {
    const list = await ipc<AssetLensCount[] | null>("lens_list");
    return Array.isArray(list) ? list : [];
  } catch {
    return [];
  }
}

/** 库内文件格式清单（搜索页格式勾选，如 NEF/ARW/JPG）；失败/非数组回退 [] */
export async function formatList(): Promise<AssetFormatCount[]> {
  try {
    const list = await ipc<AssetFormatCount[] | null>("format_list");
    return Array.isArray(list) ? list : [];
  } catch {
    return [];
  }
}

/** 最近添加的资产（recent_assets；created_at DESC keyset：afterId=上一页最后一条 id，
 *  首页传 0）。后端就绪前命令失败/非数组回退 []——UI 自然降级空态。
 *  注：M4.5 改向后「最近浏览」页走 recentViewed；本封装保留（IPC 仍存在）。 */
export async function recentAssets(afterId: number, limit: number): Promise<AssetDto[]> {
  try {
    const list = await ipc<AssetDto[] | null>("recent_assets", { afterId, limit });
    return Array.isArray(list) ? list : [];
  } catch {
    return [];
  }
}

// --- 最近浏览（M4.5：查看器打开/切图打点，最近浏览页数据源） ---------------------------

/** 最近浏览的资产（recent_viewed；按最后浏览时间 DESC，同资产取最新一次，上限 200）。
 *  后端就绪前命令失败/非数组回退 []——UI 自然降级空态。 */
export async function recentViewed(limit: number): Promise<AssetDto[]> {
  try {
    const list = await ipc<AssetDto[] | null>("recent_viewed", { limit });
    return Array.isArray(list) ? list : [];
  } catch {
    return [];
  }
}

// --- 连拍分组（M6） -------------------------------------------------------------------

/** 连拍统计（burst_stats 返回；后端不可用/失败为 null——UI 隐藏展示行） */
export interface BurstStats {
  groups: number;
  photosInBursts: number;
}

/** 连拍分组统计快照；命令失败/负载异常静默 null */
export async function burstStats(): Promise<BurstStats | null> {
  try {
    const stats = await ipc<BurstStats | null>("burst_stats");
    if (stats === null || typeof stats !== "object") return null;
    const s = stats as Partial<BurstStats>;
    return typeof s.groups === "number" && typeof s.photosInBursts === "number"
      ? (stats as BurstStats)
      : null;
  } catch {
    return null;
  }
}

// --- 那年今天 / 器材统计（M7） ---------------------------------------------------------

/** 那年今天：历年同月日资产（on_this_day；今天无历史为 []）。
 *  前端按 capturedAt 年份归块（降序）；失败/非数组回退 []——UI 自然降级空态。 */
export async function onThisDay(): Promise<AssetDto[]> {
  try {
    const list = await ipc<AssetDto[] | null>("on_this_day");
    return Array.isArray(list) ? list : [];
  } catch {
    return [];
  }
}

/** 机身/镜头型号计数（gear_stats TOP 榜；按 count 降序由后端排） */
export interface GearNameCount {
  /** 型号名（EXIF Model/LensModel，后端已做空值清洗，如 "Canon EOS R5"） */
  name: string;
  count: number;
}

/** 焦段分桶（24/50/85/135/200+mm）：max=null 表示 200+ 开放桶 */
export interface GearFocalBucket {
  /** 桶文案（如 "24mm"、"200+mm"） */
  label: string;
  /** 桶下界（含）；200+ 桶 min=200 */
  min: number;
  /** 桶上界（不含）；null=开放桶 */
  max: number | null;
  count: number;
}

/** 标签计数桶（ISO/光圈/快门分布共用；label 由后端生成，如 "ISO 400"、"f/2.8"、"1/250s"） */
export interface GearLabelBucket {
  label: string;
  count: number;
}

/** 器材与拍摄参数分布快照（gear_stats 返回；无 EXIF 数据/后端未就绪为 null） */
export interface GearStats {
  /** 机身 TOP（型号 × 快门数） */
  cameras: GearNameCount[];
  /** 镜头 TOP */
  lenses: GearNameCount[];
  /** 焦段分布（24/50/85/135/200+mm 分桶） */
  focalBuckets: GearFocalBucket[];
  /** ISO 分布（100/200/400/800/1600/3200+） */
  isoBuckets: GearLabelBucket[];
  /** 光圈分布（f/1.x/2.x/4/5.6/8/11+） */
  apertureBuckets: GearLabelBucket[];
  /** 快门分布（>1s/1s/1/2…1/1000+ 归档） */
  shutterBuckets: GearLabelBucket[];
}

/** 负载形状校验：六个字段必须是数组（桶项内容浅校验 count 为数） */
function isGearStats(value: unknown): value is GearStats {
  if (typeof value !== "object" || value === null) return false;
  const s = value as Partial<Record<keyof GearStats, unknown>>;
  const arrays: Array<[unknown, (item: unknown) => boolean]> = [
    [s.cameras, (i) => typeof (i as GearNameCount)?.name === "string" && typeof (i as GearNameCount)?.count === "number"],
    [s.lenses, (i) => typeof (i as GearNameCount)?.name === "string" && typeof (i as GearNameCount)?.count === "number"],
    [s.focalBuckets, (i) => typeof (i as GearFocalBucket)?.label === "string" && typeof (i as GearFocalBucket)?.count === "number"],
    [s.isoBuckets, (i) => typeof (i as GearLabelBucket)?.label === "string" && typeof (i as GearLabelBucket)?.count === "number"],
    [s.apertureBuckets, (i) => typeof (i as GearLabelBucket)?.label === "string" && typeof (i as GearLabelBucket)?.count === "number"],
    [s.shutterBuckets, (i) => typeof (i as GearLabelBucket)?.label === "string" && typeof (i as GearLabelBucket)?.count === "number"],
  ];
  return arrays.every(([field, itemOk]) => Array.isArray(field) && field.every(itemOk));
}

/** 器材统计快照；命令失败/负载形状异常静默 null */
export async function gearStats(): Promise<GearStats | null> {
  try {
    const stats = await ipc<GearStats | null>("gear_stats");
    return isGearStats(stats) ? stats : null;
  } catch {
    return null;
  }
}

// --- 两级去重（M7 F8：完全重复 exact / 近似 similar） -----------------------------------

/** 去重档位：exact = (size, xxhash) 完全相同；similar = pHash 汉明 ≤6 近似
 *  （RAW+JPG 孪生已被后端排除；连拍组内不排除——正是挑片场景） */
export type DuplicateKind = "exact" | "similar";

/** 重复组（duplicates_list 返回）：组内 created_at 升序（首张=最早入库） */
export interface DuplicateGroupDto {
  kind: DuplicateKind;
  assets: AssetDto[];
}

/**
 * 重复组列表（duplicates_list）。游标语义（对齐 src-tauri duplicates.rs）：
 * after = 上一页末组**序号**（0 基组偏移，skip 计数——不是组 id），首页传
 * 0/省略；limit 限组数（后端默认 50、上限 100）。组序：组大小降序。
 * 失败/非数组回退 []，组项形状异常剔除。
 */
export async function duplicatesList(
  kind: DuplicateKind,
  after = 0,
  limit = 20,
): Promise<DuplicateGroupDto[]> {
  try {
    const list = await ipc<DuplicateGroupDto[] | null>("duplicates_list", { kind, after, limit });
    if (!Array.isArray(list)) return [];
    return list.filter(
      (g): g is DuplicateGroupDto =>
        typeof g?.kind === "string" && (g.kind === "exact" || g.kind === "similar") && Array.isArray(g.assets),
    );
  } catch {
    return [];
  }
}

/** 批量删除资产（duplicate_delete：文件 + 库行级联，幂等容忍文件缺失）。
 *  返回实际删除数；业务错误（如未选库）原样抛给调用方展示。 */
export async function duplicateDelete(assetIds: number[]): Promise<number> {
  const deleted = await ipc<number>("duplicate_delete", { assetIds });
  return typeof deleted === "number" ? deleted : 0;
}

/** 标记资产被浏览（asset_view_mark；查看器打开/切图时调用，fire-and-forget）。
 *  命令失败静默——浏览打点不阻塞查看。 */
export async function assetViewMark(assetId: number): Promise<void> {
  try {
    await ipc<void>("asset_view_mark", { assetId });
  } catch {
    // 静默
  }
}

/** 暂停索引任务（库级后台：缩略图三档/EXIF 深提取）；命令失败静默（后端接线前按钮无副作用） */
export async function indexTaskPause(): Promise<void> {
  try {
    await ipc<void>("index_task_pause");
  } catch {
  }
}

// --- M4 AI：模型管理 / 语义搜索 ----------------------------------------------------

export type AiFeature = "semantic" | "face";
export type AiModelState = "idle" | "downloading" | "verifying" | "done" | "failed";

/** AI 模型状态（ai_models_status 返回；清单：siglip2-visual/siglip2-text/scrfd/arcface） */
export interface AiModelStatus {
  id: string;
  /** 旧字段兼容（=state==="done"） */
  installed: boolean;
  bytesTotal: number;
  downloadedBytes: number;
  version: string | null;
  feature: AiFeature;
  state: AiModelState;
}

/** 模型清单（ai_models_status 失败/非数组回退 []——UI 显示后端未连接态） */
export async function aiModelsStatus(): Promise<AiModelStatus[]> {
  try {
    const list = await ipc<AiModelStatus[] | null>("ai_models_status");
    return Array.isArray(list) ? list : [];
  } catch {
    return [];
  }
}

/** 开始下载模型；命令失败静默（UI 状态以 status 轮询/事件为准） */
export async function aiModelDownload(id: string): Promise<void> {
  try {
    await ipc<void>("ai_model_download", { id });
  } catch {
  }
}

/** 取消下载；命令失败静默 */
export async function aiModelCancel(id: string): Promise<void> {
  try {
    await ipc<void>("ai_model_cancel", { id });
  } catch {
  }
}

/** 删除已安装模型释放磁盘；命令失败静默 */
export async function aiModelDelete(id: string): Promise<void> {
  try {
    await ipc<void>("ai_model_delete", { id });
  } catch {
  }
}

/** 一键清除人脸数据（聚类结果+特征向量，红色强确认后调用）；失败返回 false */
export async function aiFaceDataClear(): Promise<boolean> {
  try {
    const ok = await ipc<boolean | null>("ai_face_data_clear");
    return ok === true;
  } catch {
    return false;
  }
}

// --- M4 人物（人脸聚类）契约 ------------------------------------------------------

/** 人物聚类条目（people_list 返回；faceCount 降序由后端保证） */
export interface PersonCluster {
  clusterId: number;
  /** 用户命名；未命名为 null（UI 显示「人物 N」） */
  name: string | null;
  /** 聚类内人脸数（徽标） */
  faceCount: number;
  /** 封面资产 id（走 asset_thumb_get 管线取图） */
  coverAssetId: number;
}

/** 后端 PersonRow 的公开载荷。历史前端曾把 `id` 误写成 `clusterId`，
 * 这里在 IPC 边界统一归一，避免 UI 再出现「人物 NaN」和 NaN 操作参数。 */
function normalizePersonCluster(value: unknown): PersonCluster | null {
  if (value === null || typeof value !== "object") return null;
  const row = value as Record<string, unknown>;
  const clusterId = typeof row.clusterId === "number" ? row.clusterId : row.id;
  const faceCount = row.faceCount;
  const coverAssetId = row.coverAssetId;
  if (
    typeof clusterId !== "number" || !Number.isFinite(clusterId) ||
    typeof faceCount !== "number" || !Number.isFinite(faceCount) ||
    typeof coverAssetId !== "number" || !Number.isFinite(coverAssetId)
  ) return null;
  return {
    clusterId,
    name: typeof row.name === "string" ? row.name : null,
    faceCount,
    coverAssetId,
  };
}

export interface SemanticHit {
  assetId: number;
  /** 相似度 0..1 */
  score: number;
}

/**
 * 语义搜索（SIGLIP2 向量检索）；模型未就绪时后端返回明确错误字符串，
 * 本封装将其抛给调用方（区别于传输失败——用 isIpcAvailable 区分不了，故显式透传）。
 * @param query 自然语言描述（"海边日落"）；limit 默认 100；minScore 可选阈值
 */
export async function searchSemantic(
  query: string,
  limit: number,
  minScore?: number,
): Promise<SemanticHit[]> {
  const payload: Record<string, unknown> = { query, limit };
  if (minScore !== undefined) payload.minScore = minScore;
  const hits = await ipc<SemanticHit[]>("search_semantic", payload);
  return Array.isArray(hits) ? hits : [];
}

/** 按 id 批量取资产（语义/智能相册结果回填 AssetDto 用）；失败/非数组回退 []。
 *  契约补充项（assets_by_ids，需后端 lane 实现；keyset 之外唯一的随机访问口）。 */
export async function assetsByIds(ids: number[]): Promise<AssetDto[]> {
  try {
    const list = await ipc<AssetDto[] | null>("assets_by_ids", { ids });
    return Array.isArray(list) ? list : [];
  } catch {
    return [];
  }
}

/** 单资产全量元数据（查看器 EXIF 面板）；命令失败/不存在/负载异常返回 null。
 *  后端负载 → AssetDetailDto 归一：filename/size/createdAt + duplicate_count（顶层
 *  snake_case，未做 camelCase 重命名）→ dupCount；字段缺失容错（不透传 undefined），
 *  EXIF 扩展字段存在即带出（aperture/shutter 兼容 fNumber/exposure 别名；
 *  M4 新字段读 camelCase、snake_case 兜底）。 */
export async function assetDetail(id: number): Promise<AssetDetailDto | null> {
  try {
    const raw = await ipc<unknown>("asset_detail", { id });
    if (raw === null || typeof raw !== "object") return null;
    const r = raw as Record<string, unknown>;
    /** 字符串或有限数字 → 展示态字符串（光圈/快门/焦距双形态兼容） */
const strNumOf = (v: unknown): string | null =>
  typeof v === "number" && Number.isFinite(v)
    ? String(v)
    : typeof v === "string" && v.length > 0
      ? v
      : null;

const numOf = (v: unknown): number | null =>
      typeof v === "number" && Number.isFinite(v) ? v : null;
    const strOf = (v: unknown): string | null =>
      typeof v === "string" && v.length > 0 ? v : null;
    const kind = r.kind === "raw" || r.kind === "video" ? r.kind : "photo";
    return {
      id: numOf(r.id) ?? 0,
      path: typeof r.path === "string" ? r.path : "",
      filename:
        typeof r.filename === "string" ? r.filename : typeof r.name === "string" ? r.name : "",
      size: numOf(r.size ?? r.sizeBytes) ?? 0,
      kind,
      capturedAt: strOf(r.capturedAt),
      camera: strOf(r.camera),
      lens: strOf(r.lens),
      createdAt: strOf(r.createdAt),
      dupCount: numOf(r.duplicate_count ?? r.duplicateCount) ?? 0,
      width: numOf(r.width),
      height: numOf(r.height),
      megapixels: numOf(r.megapixels),
      aspect: strOf(r.aspect),
      orientation: numOf(r.orientation),
      iso: numOf(r.iso),
      // 光圈/快门/焦距：展示态字符串（"2.8"/"1/250"/"59"），数字形态兼容转字符串
      aperture: strNumOf(r.aperture ?? r.fNumber),
      shutter: strNumOf(r.shutter ?? r.exposureTime ?? r.exposure),
      focalLength: strNumOf(r.focalLength),
      flash: strOf(r.flash),
      meteringMode: strOf(r.meteringMode ?? r.metering_mode),
      whiteBalance: strOf(r.whiteBalance ?? r.white_balance),
      exposureProgram: strOf(r.exposureProgram ?? r.exposure_program),
      software: strOf(r.software),
      artist: strOf(r.artist),
      gpsLat: numOf(r.gpsLat ?? r.gps_lat),
      gpsLon: numOf(r.gpsLon ?? r.gps_lon),
      format: strOf(r.format),
      rating: numOf(r.rating),
      flagged: typeof r.flagged === "boolean" ? r.flagged : null,
    };
  } catch {
    return null;
  }
}

/** asset_thumb_get 三态结果（「排队中≠永久失败」的关键契约） */
export type ThumbGetResult =
  | { status: "ready"; path: string }
  | { status: "pending" }
  | { status: "unavailable" };

/** 库内资产缩略图（按 assetId 取后端缓存文件绝对路径，调用方自行 convertFileSrc）。
 *  与向导的 thumbGet（按源文件路径，导入前预览用）是两个命令：本命令为 asset_thumb_get。
 *  size 为期望边长（画廊网格 240 / 查看器大图 1280；RAW 传 >2048 = 内嵌全幅直出档）。
 *  三态：ready=缓存命中（附路径）；pending=已入队后台生成（thumbnailReady 事件后
 *  重试即 ready）；unavailable=永久不可用（资产不存在 / thumb_state=2 / 不可解码 /
 *  连续失败 ≥3 次）。 */
export async function assetThumbGet(assetId: number, size: number): Promise<ThumbGetResult> {
  try {
    const raw = await ipc<unknown>("asset_thumb_get", { assetId, size });
    if (raw !== null && typeof raw === "object") {
      const r = raw as { status?: unknown; path?: unknown };
      if (r.status === "ready" && typeof r.path === "string") {
        return { status: "ready", path: r.path };
      }
      if (r.status === "pending" || r.status === "unavailable") {
        return { status: r.status };
      }
    }
    return { status: "unavailable" };
  } catch {
    return { status: "unavailable" };
  }
}

// --- M4 人物命令封装 ----------------------------------------------------------------

/** 人物清单（人脸聚类结果；失败/非数组回退 []——后端未就绪即空态兜底） */
export async function peopleList(): Promise<PersonCluster[]> {
  try {
    const list = await ipc<unknown>("people_list");
    if (!Array.isArray(list)) return [];
    return list
      .map(normalizePersonCluster)
      .filter((person): person is PersonCluster => person !== null);
  } catch {
    return [];
  }
}

/** 某人物聚类内的照片（序由后端保证）；失败回退 [] */
export async function peopleAssets(clusterId: number, limit: number): Promise<AssetDto[]> {
  try {
    const list = await ipc<AssetDto[] | null>("people_assets", { clusterId, limit });
    return Array.isArray(list) ? list : [];
  } catch {
    return [];
  }
}

/** 重命名人物（clusterId 聚类；空名由调用方拦下）；命令失败返回 false */
export async function personRename(clusterId: number, name: string): Promise<boolean> {
  try {
    await ipc<unknown>("person_rename", { clusterId, name });
    return true;
  } catch {
    return false;
  }
}

/** 删除人物聚类（仅拆聚类，照片不受影响）；命令失败返回 false */
export async function personDelete(clusterId: number): Promise<boolean> {
  try {
    await ipc<unknown>("person_delete", { clusterId });
    return true;
  } catch {
    return false;
  }
}

// --- 索引任务（缩略图/EXIF/语义）：状态与手动触发 -------------------------------------

export type IndexKind = "thumb" | "exif" | "ai" | "face";

/**
 * 单通道任务计数（index_status，对应后端 IndexKindStatus）。与后端持久化
 * 任务账（index_tasks 表）同构：pending/running 是「进行中」按钮态的派生
 * 真值；total=可索引资产数（N/M 口径）；failed 仅展示用途。
 */
export interface IndexCounters {
  pending: number;
  running: number;
  done: number;
  failed: number;
  total: number;
}

/** 索引状态（index_status 返回；thumb/exif/ai 三通道计数） */
export interface IndexStatus {
  thumb: IndexCounters;
  exif: IndexCounters;
  ai: IndexCounters;
  /** 新后端始终返回；可选仅用于兼容旧版本/测试夹具。 */
  face?: IndexCounters;
}

/** 索引状态快照；失败/负载异常返回 null（调用方隐藏/降级区块） */
export async function indexStatus(): Promise<IndexStatus | null> {
  try {
    const status = await ipc<IndexStatus | null>("index_status");
    if (status === null || typeof status !== "object") return null;
    const s = status as Partial<Record<IndexKind, unknown>>;
    const okThumb = s.thumb !== null && typeof s.thumb === "object";
    const okExif = s.exif !== null && typeof s.exif === "object";
    const okAi = s.ai !== null && typeof s.ai === "object";
    return okThumb && okExif && okAi ? (status as IndexStatus) : null;
  } catch {
    return null;
  }
}

/** 立即触发指定索引（幂等）。不 catch：ai 模型未就绪等业务错误（后端 Err 文案，
 *  如「请先在设置中下载模型」）由调用方提示；传输失败经 ipc() 统一置不可用标志 */
export async function indexKickNow(kind: IndexKind): Promise<void> {
  await ipc<void>("index_kick_now", { kind });
}

// --- 索引重建 / 资产标记（M5） --------------------------------------------------------

/** 重建索引的通道（index_rebuild；语义通道在重建命令里叫 semantic，与 IndexKind 的 ai 区分） */
export type RebuildKind = "thumb" | "exif" | "semantic" | "face";

/** 重建指定索引（index_rebuild：清缓存/任务账重跑）。不 catch：失败文案透传给调用方 */
export async function indexRebuild(kind: RebuildKind): Promise<void> {
  await ipc<void>("index_rebuild", { kind });
}

/** 资产评分（asset_rating_set；0=清除，1-5 星）。命令失败静默（乐观 UI 由调用方回滚） */
export async function assetRatingSet(assetId: number, rating: number): Promise<void> {
  try {
    await ipc<void>("asset_rating_set", { assetId, rating });
  } catch {
    // 静默
  }
}

/** 资产旗标（asset_flag_set；红旗标记/待整理）。命令失败静默 */
export async function assetFlagSet(assetId: number, flagged: boolean): Promise<void> {
  try {
    await ipc<void>("asset_flag_set", { assetId, flagged });
  } catch {
    // 静默
  }
}

/** 删除历史任务记录（import_job_delete；任务抽屉历史区 × 按钮）。不 catch：
 *  失败文案透传给调用方提示 */
export async function importJobDelete(jobId: number): Promise<void> {
  await ipc<void>("import_job_delete", { jobId });
}

/** 用系统默认程序打开文件（open_with_system；视频 HEVC 缺解码器时的回退播放）。
 *  不 catch：失败文案透传给调用方提示 */
export async function openWithSystem(path: string): Promise<void> {
  await ipc<void>("open_with_system", { path });
}

/** 复制文件到系统剪贴板（clipboard_copy_files：Explorer 可直接粘贴的文件对象；
 *  后端在途契约——命令未注册时 ipc() 抛错由调用方兜底提示）。不 catch：
 *  失败文案透传给调用方。 */
export async function clipboardCopyFiles(paths: string[]): Promise<void> {
  await ipc<void>("clipboard_copy_files", { paths });
}

/** 侧栏导航计数（sidebar_counts：一次性纯 COUNT；后端在途契约——
 *  命令未注册/失败/形状异常静默 null，侧栏不显示徽标） */
export interface SidebarCounts {
  /** 图库资产总数 */
  assets: number;
  /** 最近浏览条数 */
  recentViewed: number;
  /** 那年今天条数 */
  onThisDay: number;
  /** 标签数 */
  tags: number;
  /** 相册数 */
  albums: number;
}

export async function sidebarCounts(): Promise<SidebarCounts | null> {
  try {
    const counts = await ipc<SidebarCounts | null>("sidebar_counts");
    if (counts === null || typeof counts !== "object") return null;
    const c = counts as Partial<Record<keyof SidebarCounts, unknown>>;
    const numOf = (v: unknown): number =>
      typeof v === "number" && Number.isFinite(v) ? v : 0;
    return {
      assets: numOf(c.assets),
      recentViewed: numOf(c.recentViewed),
      onThisDay: numOf(c.onThisDay),
      tags: numOf(c.tags),
      albums: numOf(c.albums),
    };
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
