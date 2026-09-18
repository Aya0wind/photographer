import { create } from "zustand";

import {
  deviceList,
  deviceScan,
  importCancel,
  importJobsPage,
  importPause,
  importResume,
  importRetryFailed,
  subscribeAppEvents,
  type AppEvent,
  type CleanResultDto,
  type DeviceKind,
  type DeviceSnapshot,
  type FileKind,
  type ImportMode,
  type ImportStats,
  type JobRow,
} from "@/ipc/api";
import { useSettingsStore } from "@/stores/settingsStore";

/**
 * 导入域中心状态：设备列表（deviceScanned 事件驱动）、活跃任务进度
 * （importFileProgress 150ms 节流渲染）、任务状态机
 * （idle→running→paused→done/cancelled/failed，非法转移忽略）、
 * 历史任务游标分页缓存、sessionFinished 的总结快照。
 * 所有命令调用均经 api 层兜底，dev 预览无后端不崩。
 */

/** 源文件（M1 由扫描结果注入；后端文件枚举命令接入前可能为空） */
export interface SourceFile {
  /** 完整路径 */
  path: string;
  /** 所属目录（树分组键） */
  dir: string;
  /** 文件名 */
  name: string;
  size: number;
  kind: FileKind;
}

export interface FailedFileEntry {
  src: string;
  dst: string;
  state: string;
}

export interface ActiveJob {
  jobId: number;
  status: "running" | "paused" | "done" | "cancelled" | "failed";
  totalFiles: number;
  totalBytes: number;
  doneFiles: number;
  doneBytes: number;
  /** 进度条口径：完成+跳过+失败；可选（旧事件/条目缺省回落 doneBytes） */
  settledBytes?: number;
  bytesPerSec: number;
  currentFile: string;
}

export interface JobSummary {
  jobId: number;
  stats: ImportStats;
  failures: FailedFileEntry[];
  /** 导入模式快照（总结弹窗「复制/移动」文案用）；事件不含 mode，由启动方记录，未知回退 copy */
  mode: ImportMode;
}

export interface HistoryState {
  rows: JobRow[];
  /** 下一页游标（已加载最后一行 id；0 表示从头） */
  cursor: number;
  exhausted: boolean;
  loading: boolean;
}

/** 清卡任务态（cleanStarted/cleanFinished 事件驱动；单任务串行，M2 无并发清卡） */
export interface CleanState {
  jobId: number;
  phase: "running" | "finished";
  /** cleanStarted 的候选数/字节数 */
  count: number;
  bytes: number;
  /** cleanFinished 的结果（phase=finished 时非空） */
  stats: CleanResultDto | null;
}

export const PROGRESS_THROTTLE_MS = 150;
export const HISTORY_PAGE_SIZE = 20;

/** 最近使用源（LR 式源面板）：设备或文件夹，localStorage 持久化，最多 5 条 */
export interface RecentSource {
  id: string;
  name: string;
  kind: DeviceKind;
}

export const RECENT_SOURCES_MAX = 5;
const RECENT_SOURCES_KEY = "smartphoto.import.recentSources";

/** importFileCompleted 的 state 字段中表示失败的取值 */
const FAILURE_STATES = new Set(["failed", "error"]);

interface ImportState {
  devices: DeviceSnapshot[];
  /** 已到达、快照尚未回来的设备（deviceArrived） */
  scanning: { id: string; name: string; kind: DeviceKind }[];
  /** 等待 DeviceDialog 逐个弹出的设备 id 队列 */
  promptQueue: string[];
  activeJobs: Record<number, ActiveJob>;
  /** 最近一次交互（启动/进度/暂停…）的任务 id，任务中心顶部卡片用 */
  currentJobId: number | null;
  /** 每任务的失败文件清单（importFileCompleted state=failed 收集） */
  failedFiles: Record<number, FailedFileEntry[]>;
  history: HistoryState;
  /** importSessionFinished 的总结快照；null 表示无弹窗 */
  summary: JobSummary | null;
  lastError: { level: string; message: string; recoverable: boolean } | null;
  sourceFiles: Record<string, SourceFile[]>;
  /** 最近使用的导入源（设备或文件夹），新选择的排最前 */
  recentSources: RecentSource[];
  /** 每任务的导入模式（启动时由向导记录，事件不含 mode） */
  jobModes: Record<number, ImportMode>;
  /**
   * 待归位的导入模式：向导在调用 import_start 前设置。
   * 修 E2E 竞态——sessionStarted 事件可能先于 import_start 的 invoke 返回到达，
   * 此时 jobId 未知；sessionStarted 创建任务时把 pending 归位到 jobModes。
   */
  pendingJobMode: ImportMode | null;
  /** 每任务的源设备类型（清卡入口只对 volume/MTP 源显示；folder 源不可清） */
  jobSources: Record<number, DeviceKind>;
  /** 待归位源类型（与 pendingJobMode 同款竞态防护） */
  pendingJobSource: DeviceKind | null;
  /** 清卡任务态（cleanStarted/cleanFinished 驱动）；null=无清卡 */
  clean: CleanState | null;

  /** 事件入口（initImportStore 订阅转发；测试可直接驱动） */
  handleAppEvent: (event: AppEvent) => void;
  /** 忽略设备弹窗（队列继续下一个） */
  ignoreDevice: (id: string) => void;
  /** 手动刷新设备快照（device_scan），成功则更新列表 */
  refreshDevice: (id: string) => Promise<void>;
  /** 注入/更新设备快照（文件夹源 folderScan 结果走这里，同 id 则覆盖） */
  addDevice: (snapshot: DeviceSnapshot) => void;
  /** 记录最近使用源（去重置顶，截断至 5 条，写 localStorage） */
  recordRecentSource: (source: RecentSource) => void;
  /** 记录任务导入模式（总结弹窗/任务中心文案用） */
  recordJobMode: (jobId: number, mode: ImportMode) => void;
  /** 设置待归位导入模式（紧贴 importStart 调用；sessionStarted 消费） */
  setPendingJobMode: (mode: ImportMode | null) => void;
  /** 记录任务源设备类型（清卡入口判定用） */
  recordJobSource: (jobId: number, kind: DeviceKind) => void;
  /** 设置待归位源类型（紧贴 importStart 调用；sessionStarted 消费） */
  setPendingJobSource: (kind: DeviceKind | null) => void;
  dismissSummary: () => void;
  /** 历史任务分页；reset=true 重置游标重新加载 */
  loadHistory: (reset?: boolean) => Promise<void>;
  /** 以下三个动作只发命令，状态由事件驱动推进 */
  pauseJob: (jobId: number) => Promise<void>;
  resumeJob: (jobId: number) => Promise<void>;
  cancelJob: (jobId: number) => Promise<void>;
  /** 重试失败文件，返回新 jobId（失败 null） */
  retryFailed: (jobId: number) => Promise<number | null>;
  /** 注入源文件清单（后端枚举接入点） */
  setSourceFiles: (deviceId: string, files: SourceFile[]) => void;
}

// --- 进度节流缓冲（模块级，避免进入 React 状态） -------------------------------

type ProgressFields = Pick<ActiveJob, "doneFiles" | "doneBytes" | "settledBytes" | "currentFile" | "bytesPerSec">;

const pendingProgress = new Map<number, ProgressFields>();
let flushTimer: ReturnType<typeof setTimeout> | null = null;

function scheduleFlush(): void {
  if (flushTimer !== null) return;
  flushTimer = setTimeout(() => {
    flushTimer = null;
    flushPendingProgress();
  }, PROGRESS_THROTTLE_MS);
}

/** 把缓冲中的最新进度一次性写入 store（渲染层 150ms 只见一次更新） */
function flushPendingProgress(): void {
  if (flushTimer !== null) {
    clearTimeout(flushTimer);
    flushTimer = null;
  }
  if (pendingProgress.size === 0) return;
  const entries = [...pendingProgress.entries()];
  pendingProgress.clear();
  useImportStore.setState((s) => {
    const activeJobs = { ...s.activeJobs };
    for (const [jobId, progress] of entries) {
      const job = activeJobs[jobId];
      if (job) activeJobs[jobId] = { ...job, ...progress };
    }
    return { activeJobs };
  });
}

function dropPendingProgress(jobId: number): void {
  pendingProgress.delete(jobId);
  if (pendingProgress.size === 0 && flushTimer !== null) {
    clearTimeout(flushTimer);
    flushTimer = null;
  }
}

// --- 任务状态机 ----------------------------------------------------------------

type JobLifecycleStatus = ActiveJob["status"] | "idle";

/** 合法转移表：idle 只能经 sessionStarted 进入 running；终态不可逆 */
const VALID_TRANSITIONS: Record<JobLifecycleStatus, ActiveJob["status"][]> = {
  idle: [],
  running: ["paused", "cancelled", "done"],
  paused: ["running", "cancelled", "done"],
  done: [],
  cancelled: [],
  failed: [],
};

function canTransition(from: JobLifecycleStatus, to: ActiveJob["status"]): boolean {
  return VALID_TRANSITIONS[from].includes(to);
}

/** 本会话已弹过窗的设备 id（避免手动重扫重复弹；拔出后清除以便重插再弹） */
const promptedDevices = new Set<string>();

const EMPTY_HISTORY: HistoryState = { rows: [], cursor: 0, exhausted: false, loading: false };

function upsertDevice(list: DeviceSnapshot[], snapshot: DeviceSnapshot): DeviceSnapshot[] {
  const idx = list.findIndex((d) => d.id === snapshot.id);
  if (idx === -1) return [...list, snapshot];
  const next = [...list];
  next[idx] = snapshot;
  return next;
}

// --- 最近使用源持久化（localStorage，坏数据静默丢弃） ---------------------------

function isRecentSource(value: unknown): value is RecentSource {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  return typeof v.id === "string" && typeof v.name === "string" && typeof v.kind === "string";
}

function loadRecentSources(): RecentSource[] {
  try {
    const raw = localStorage.getItem(RECENT_SOURCES_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed)
      ? parsed.filter(isRecentSource).slice(0, RECENT_SOURCES_MAX)
      : [];
  } catch {
    return [];
  }
}

function saveRecentSources(list: RecentSource[]): void {
  try {
    localStorage.setItem(RECENT_SOURCES_KEY, JSON.stringify(list));
  } catch {
    // 存储不可用（隐私模式/超限）时静默，仅内存态生效
  }
}

export const useImportStore = create<ImportState>((set, get) => ({
  devices: [],
  scanning: [],
  promptQueue: [],
  activeJobs: {},
  currentJobId: null,
  failedFiles: {},
  history: { ...EMPTY_HISTORY },
  summary: null,
  lastError: null,
  sourceFiles: {},
  recentSources: loadRecentSources(),
  jobModes: {},
  pendingJobMode: null,
  jobSources: {},
  pendingJobSource: null,
  clean: null,

  handleAppEvent: (event) => {
    switch (event.type) {
      case "deviceArrived": {
        set((s) => ({
          scanning: [
            ...s.scanning.filter((d) => d.id !== event.id),
            { id: event.id, name: event.name, kind: event.kind },
          ],
        }));
        break;
      }

      case "deviceRemoved": {
        promptedDevices.delete(event.id);
        set((s) => ({
          devices: s.devices.filter((d) => d.id !== event.id),
          scanning: s.scanning.filter((d) => d.id !== event.id),
          promptQueue: s.promptQueue.filter((id) => id !== event.id),
        }));
        break;
      }

      case "deviceScanned": {
        // 遵循“插入设备时提示”设置：关闭则不弹窗（视为已处理）
        const prompt =
          useSettingsStore.getState().settings.import.promptOnDevice &&
          !promptedDevices.has(event.id);
        promptedDevices.add(event.id);
        set((s) => ({
          devices: upsertDevice(s.devices, event.snapshot),
          scanning: s.scanning.filter((d) => d.id !== event.id),
          promptQueue:
            prompt && !s.promptQueue.includes(event.id)
              ? [...s.promptQueue, event.id]
              : s.promptQueue,
        }));
        break;
      }

      case "importSessionStarted": {
        set((s) => {
          // idle→running：重复 start（任务已存在）属非法转移，忽略
          if (s.activeJobs[event.jobId]) return s;
          const activeJobs = { ...s.activeJobs };
          activeJobs[event.jobId] = {
            jobId: event.jobId,
            status: "running",
            totalFiles: event.totalFiles,
            totalBytes: event.totalBytes,
            doneFiles: 0,
            doneBytes: 0,
            settledBytes: 0,
            bytesPerSec: 0,
            currentFile: "",
          };
          const failedFiles = { ...s.failedFiles };
          delete failedFiles[event.jobId];
          // 竞态修复：事件可能先于 import_start 返回到达，把待归位模式挂到本任务
          const jobModes = { ...s.jobModes };
          jobModes[event.jobId] = s.pendingJobMode ?? "copy";
          const jobSources = { ...s.jobSources };
          jobSources[event.jobId] = s.pendingJobSource ?? "volume";
          return {
            activeJobs,
            failedFiles,
            currentJobId: event.jobId,
            jobModes,
            jobSources,
            pendingJobMode: null,
            pendingJobSource: null,
          };
        });
        break;
      }

      case "importFileProgress": {
        // 只写入缓冲，150ms 后统一落到渲染状态
        pendingProgress.set(event.jobId, {
          doneFiles: event.doneFiles,
          doneBytes: event.doneBytes,
          settledBytes: event.settledBytes ?? event.doneBytes,
          currentFile: event.currentFile,
          bytesPerSec: event.bytesPerSec,
        });
        scheduleFlush();
        set({ currentJobId: event.jobId });
        break;
      }

      case "importPaused": {
        set((s) => applyStatusTransition(s, event.jobId, "paused"));
        break;
      }

      case "importResumed": {
        set((s) => applyStatusTransition(s, event.jobId, "running"));
        break;
      }

      case "importCancelled": {
        dropPendingProgress(event.jobId);
        set((s) => applyStatusTransition(s, event.jobId, "cancelled"));
        break;
      }

      case "importSessionFinished": {
        dropPendingProgress(event.jobId);
        set((s) => {
          const job = s.activeJobs[event.jobId];
          // 已取消/已完成的任务收到 finished：保持原终态（非法转移忽略）
          if (!job || !canTransition(job.status, "done")) return s;
          const activeJobs = { ...s.activeJobs };
          activeJobs[event.jobId] = {
            ...job,
            status: "done",
            doneFiles: event.stats.doneFiles,
            doneBytes: event.stats.doneBytes,
            bytesPerSec: event.stats.bytesPerSec,
            currentFile: "",
          };
          return {
            activeJobs,
            summary: {
              jobId: event.jobId,
              stats: event.stats,
              failures: s.failedFiles[event.jobId] ?? [],
              mode: s.jobModes[event.jobId] ?? "copy",
            },
          };
        });
        break;
      }

      case "importFileCompleted": {
        if (FAILURE_STATES.has(event.state)) {
          set((s) => ({
            failedFiles: {
              ...s.failedFiles,
              [event.jobId]: [
                ...(s.failedFiles[event.jobId] ?? []),
                { src: event.src, dst: event.dst, state: event.state },
              ],
            },
          }));
        }
        break;
      }

      case "cleanStarted": {
        set({ clean: { jobId: event.jobId, phase: "running", count: event.count, bytes: event.bytes, stats: null } });
        break;
      }

      case "cleanFinished": {
        set((s) => ({
          // 保留 cleanStarted 的 count/bytes 上下文（无 running 态直接到达时兜底为结果值）
          clean: {
            jobId: event.jobId,
            phase: "finished",
            count: s.clean?.jobId === event.jobId ? s.clean.count : event.stats.deleted,
            bytes: s.clean?.jobId === event.jobId ? s.clean.bytes : event.stats.freedBytes,
            stats: event.stats,
          },
        }));
        break;
      }

      case "appError": {
        set({
          lastError: {
            level: event.level,
            message: event.message,
            recoverable: event.recoverable,
          },
        });
        break;
      }
    }
  },

  ignoreDevice: (id) => {
    set((s) => ({ promptQueue: s.promptQueue.filter((d) => d !== id) }));
  },

  refreshDevice: async (id) => {
    const snapshot = await deviceScan(id);
    if (snapshot) {
      set((s) => ({ devices: upsertDevice(s.devices, snapshot) }));
    }
  },

  addDevice: (snapshot) => {
    set((s) => ({ devices: upsertDevice(s.devices, snapshot) }));
  },

  recordRecentSource: (source) => {
    set((s) => {
      const next = [
        source,
        ...s.recentSources.filter((r) => r.id !== source.id),
      ].slice(0, RECENT_SOURCES_MAX);
      saveRecentSources(next);
      return { recentSources: next };
    });
  },

  recordJobMode: (jobId, mode) => {
    set((s) => ({ jobModes: { ...s.jobModes, [jobId]: mode } }));
  },

  setPendingJobMode: (mode) => {
    set({ pendingJobMode: mode });
  },

  recordJobSource: (jobId, kind) => {
    set((s) => ({ jobSources: { ...s.jobSources, [jobId]: kind } }));
  },

  setPendingJobSource: (kind) => {
    set({ pendingJobSource: kind });
  },

  dismissSummary: () => {
    set({ summary: null });
  },

  loadHistory: async (reset = false) => {
    const current = get().history;
    if (current.loading) return;
    if (!reset && current.exhausted) return;
    const afterId = reset ? 0 : current.cursor;
    set({ history: { ...current, loading: true } });
    const rows = await importJobsPage(afterId, HISTORY_PAGE_SIZE);
    const safeRows = Array.isArray(rows) ? rows : [];
    set((s) => ({
      history: {
        rows: reset ? safeRows : [...s.history.rows, ...safeRows],
        cursor: safeRows.length > 0 ? safeRows[safeRows.length - 1].id : afterId,
        exhausted: safeRows.length < HISTORY_PAGE_SIZE,
        loading: false,
      },
    }));
  },

  pauseJob: async (jobId) => {
    await importPause(jobId);
  },

  resumeJob: async (jobId) => {
    await importResume(jobId);
  },

  cancelJob: async (jobId) => {
    await importCancel(jobId);
  },

  retryFailed: async (jobId) => {
    return importRetryFailed(jobId);
  },

  setSourceFiles: (deviceId, files) => {
    set((s) => ({ sourceFiles: { ...s.sourceFiles, [deviceId]: files } }));
  },
}));

/** 状态机转移辅助：非法转移返回原 state（被 zustand 忽略） */
function applyStatusTransition(
  s: Pick<ImportState, "activeJobs">,
  jobId: number,
  to: ActiveJob["status"],
): Pick<ImportState, "activeJobs"> {
  const job = s.activeJobs[jobId];
  if (!job || !canTransition(job.status, to)) return s;
  return { activeJobs: { ...s.activeJobs, [jobId]: { ...job, status: to } } };
}

let eventsBound = false;

/**
 * 从后端拉一次设备初值并合并（upsert 去重）。
 * 必要性：后端启动枚举在 app 一启动就扫描并发出 deviceScanned，
 * 而此刻 webview 尚未加载、前端还没订阅 app://event——事件必然错过；
 * 订阅完成后 / 向导打开时主动拉 device_list 补齐。
 */
export async function seedDevicesFromBackend(): Promise<void> {
  const devices = await deviceList();
  for (const snapshot of devices) {
    useImportStore.getState().addDevice(snapshot);
  }
}

/** 应用启动时调用一次：订阅唯一事件通道 app://event（失败静默）+ 拉设备初值 */
export async function initImportStore(): Promise<void> {
  if (eventsBound) return;
  eventsBound = true;
  await subscribeAppEvents((event) => useImportStore.getState().handleAppEvent(event));
  await seedDevicesFromBackend();
}

/** 仅测试用：重置模块级缓冲/队列并清空 store */
export function resetImportStoreForTests(): void {
  pendingProgress.clear();
  if (flushTimer !== null) {
    clearTimeout(flushTimer);
    flushTimer = null;
  }
  promptedDevices.clear();
  try {
    localStorage.removeItem(RECENT_SOURCES_KEY);
  } catch {
    // 存储不可用时静默
  }
  useImportStore.setState({
    devices: [],
    scanning: [],
    promptQueue: [],
    activeJobs: {},
    currentJobId: null,
    failedFiles: {},
    history: { ...EMPTY_HISTORY },
    summary: null,
    lastError: null,
    sourceFiles: {},
    recentSources: [],
    jobModes: {},
    pendingJobMode: null,
    jobSources: {},
    pendingJobSource: null,
    clean: null,
  });
}
