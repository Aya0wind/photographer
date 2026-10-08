import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";

import { ipc } from "@/ipc";
import type { AiQualityTier } from "@/ipc/api";

/** 库（达芬奇式独立数据单元，spec §5.11）：dbDir 自包含数据库/缓存，photoRoot 照片存储。
 *  目录布局已固定为时间/相册+平铺（importSubdir/dirTemplate 退役 2026-09-28）
 *  （dirTemplate 配置退役，2026-09-28 定案——旧 settings.json 里的 dirTemplate 字段
 *  容错保留：后端 serde 有默认值，前端类型已删不再写入）；
 *  并发流数同为库属性（streams，Rust 侧 serde 缺省 4；MTP 源受协议限制恒 1，由向导在组 plan 时钳制）；
 *  configured=配置链（位置/整理规则/AI）是否走完——旧库由后端迁移自动置 true，前端读取兜底 ?? true。
 *  旧配置由后端自动补默认值，前端读取时再以全局 ImportSettings 兜底。 */
export interface Library {
  id: string;
  name: string;
  dbDir: string;
  photoRoot: string;
  /** 库级导入并发流数（后端 serde 缺省 4；旧数据无字段时读取方 ?? 4 兜底） */
  streams: number;
  /** 配置链是否已完成（未完成的库打开时引导回向导补完） */
  configured: boolean;
  /** 独立照片库的 AI 档位；缺省仅用于旧配置迁移。 */
  aiQualityTier?: AiQualityTier;
}

export interface Settings {
  schemaVersion: number;
  onboardingCompleted: boolean;
  libraries: Library[];
  activeLibraryId: string | null;
  import: {
    promptOnDevice: boolean;
    skipImported: boolean;
    duplicatePolicy: "skip" | "rename" | "ask";
  };
  /** 画廊展示（M3+）：RAW+JPG 同 pairId 合并为一张卡（优先 JPG 缩略图 + RAW+JPG 角标） */
  gallery: {
    mergeRawJpg: boolean;
  };
  ai: {
    enableClip: boolean;
    enableFace: boolean;
    enableSceneTags: boolean;
    indexSchedule: "idleOnly" | "afterImport" | "manual";
    cpuLimitPercent: number;
    useGpu: boolean;
    /** 语义检索相似度阈值（cos 0..1)；低于该分的结果过滤，0 = 不过滤。
     *  null=跟随模型自动（普通档 0.09 / 精准档 fp16 标定值，由后端按档取值）。
     *  与 Rust 侧 AiSettings.semantic_min_score 同名映射（camelCase） */
    semanticMinScore: number | null;
    /** AI 三档画质（快速/普通/精准）：决定语义/人脸用哪组模型。切档只写设置，
     *  受影响通道的索引重建由后端指纹机制自动触发（无新命令）。 */
    qualityTier: AiQualityTier;
    /** 嵌入输入边长（px；Rust 侧 serde camelCase 对齐 embed_input_size） */
    embedInputSize: number;
    /** 人脸检测阈值（0..1，scrfd 置信度门限） */
    faceDetectThreshold: number;
    /** 人脸聚类阈值（0..1，arcface 余弦距离门限） */
    faceClusterThreshold: number;
    /** 连拍分组：相邻照片时间间隔上限（毫秒，时间链因子） */
    burstGapMs: number;
    /** 连拍分组：pHash 汉明距离上限（0-63，指纹因子） */
    burstHammingMax: number;
    /** 连拍分组：最小组员数（低于此不成组） */
    burstMinSize: number;
  };
  /** 全局外观，旧配置沿用原深色界面。 */
  appearance: {
    animations: boolean;
    theme?: "dark" | "light";
  };
  system: {
    launchAtLogin: boolean;
    closeToTray: boolean;
    language: string;
  };
}

/** 递归可选，用于 update(partial) 局部 patch */
export type DeepPartial<T> = T extends object ? { [K in keyof T]?: DeepPartial<T[K]> } : T;

/** 与 Rust 侧默认值保持一致（docs 设计文档 §10 默认值清单） */
export const DEFAULT_SETTINGS: Settings = {
  schemaVersion: 1,
  onboardingCompleted: false,
  libraries: [],
  activeLibraryId: null,
  import: {
    promptOnDevice: true,
    skipImported: true,
    duplicatePolicy: "skip",
  },
  gallery: {
    mergeRawJpg: true,
  },
  ai: {
    enableClip: false,
    enableFace: false,
    enableSceneTags: false,
    indexSchedule: "idleOnly",
    cpuLimitPercent: 50,
    useGpu: true,
    semanticMinScore: null,
    qualityTier: "normal",
    embedInputSize: 256,
    faceDetectThreshold: 0.5,
    faceClusterThreshold: 0.4,
    burstGapMs: 2000,
    burstHammingMax: 10,
    burstMinSize: 2,
  },
  appearance: {
    animations: true,
    theme: "dark",
  },
  system: {
    launchAtLogin: false,
    closeToTray: true,
    language: "zh",
  },
};

interface SettingsState {
  settings: Settings;
  loaded: boolean;
  /**
   * 会话内是否已选定库（达芬奇式启动流：每次启动先 /library-picker，选完才进主壳）。
   * 非持久化字段——重启应用后回到选择器。onboardingCompleted 保留兼容但不再作门禁。
   */
  libraryChosen: boolean;
  /** 从 Rust 侧读取设置；命令尚不存在或失败时静默落回默认值 */
  load: () => Promise<void>;
  /** 持久化设置；失败时恢复原状态并把原因交给操作界面 */
  save: (next: Settings) => Promise<void>;
  /** 本地局部更新（不落盘），由调用方决定何时 save */
  update: (partial: DeepPartial<Settings>) => void;
  /** 标记本会话已选定库（选择器打开/向导完成时调用） */
  setLibraryChosen: (chosen: boolean) => void;
}

type AnyRecord = Record<string, unknown>;

function isRecord(value: unknown): value is AnyRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

/** 深合并：patch 中的 undefined 字段跳过，嵌套对象递归合并 */
function mergeDeep<T>(base: T, patch: unknown): T {
  if (isRecord(base) && isRecord(patch)) {
    const out: AnyRecord = { ...base };
    for (const key of Object.keys(patch)) {
      const value = patch[key];
      if (value === undefined) continue;
      out[key] = mergeDeep(out[key], value);
    }
    return out as T;
  }
  return (patch !== undefined ? patch : base) as T;
}

// 供引导向导等处复用（草稿编辑 → 局部 patch 合并）
export { clone, mergeDeep };

/** ai.qualityTier 仅是当前库的投影；切库时从目标库恢复，修改时只写当前库。 */
export function normalizeLibraryQuality(next: Settings, previous?: Settings): Settings {
  const result = clone(next);
  for (const lib of result.libraries) {
    const old = previous?.libraries.find((item) => item.id === lib.id);
    const legacyTier = previous
      ? old ? previous.ai.qualityTier : "normal"
      : result.ai.qualityTier;
    lib.aiQualityTier ??= old?.aiQualityTier ?? legacyTier;
  }
  const active = result.libraries.find((lib) => lib.id === result.activeLibraryId);
  if (active) {
    if (previous && result.activeLibraryId === previous.activeLibraryId && result.ai.qualityTier !== previous.ai.qualityTier) {
      active.aiQualityTier = result.ai.qualityTier;
    }
    result.ai.qualityTier = active.aiQualityTier ?? "normal";
  }
  return result;
}

export const useSettingsStore = create<SettingsState>((set, get) => ({
  settings: clone(DEFAULT_SETTINGS),
  loaded: false,
  libraryChosen: false,

  load: async () => {
    try {
      const remote = await ipc<Settings>("settings_get");
      // 远端可能是旧 schema，用默认值兜底合并，避免字段缺失
      set({ settings: normalizeLibraryQuality(mergeDeep(clone(DEFAULT_SETTINGS), remote)), loaded: true });
    } catch {
      // Rust 命令由并行任务实现，可能尚不存在；绝不能抛错
      set({ settings: clone(DEFAULT_SETTINGS), loaded: true });
    }
  },

  save: async (next: Settings) => {
    const previous = get().settings;
    const normalized = normalizeLibraryQuality(next, previous);
    set({ settings: normalized });
    try {
      await ipc("settings_set", { settings: normalized });
    } catch (e) {
      // Only roll back this write if no subsequent operation has replaced it.
      if (get().settings === normalized) set({ settings: previous });
      console.error("settings_set failed:", e);
      throw e;
    }
  },

  update: (partial: DeepPartial<Settings>) => {
    set({ settings: normalizeLibraryQuality(mergeDeep(clone(get().settings), partial), get().settings) });
  },

  setLibraryChosen: (chosen) => {
    set({ libraryChosen: chosen });
  },
}));

let eventsBound = false;

/**
 * 应用启动时调用一次：加载设置并订阅远端变更事件。
 * `settings://changed` 由 Rust 侧（配置文件热更新/托盘修改）发出。
 */
export async function initSettings(): Promise<void> {
  await useSettingsStore.getState().load();
  if (eventsBound) return;
  eventsBound = true;
  try {
    await listen<Settings>("settings://changed", (event) => {
      useSettingsStore.setState({
        settings: normalizeLibraryQuality(mergeDeep(clone(DEFAULT_SETTINGS), event.payload)),
      });
    });
  } catch {
    // 事件监听失败静默（如非 Tauri 环境下运行 vite dev 预览）
  }
}
