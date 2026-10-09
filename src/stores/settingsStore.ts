import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";

import { ipc } from "@/ipc";
import type { AiQualityTier } from "@/ipc/api";

/**
 * 应用级设置（settings.json，2026-10-09 多数据库修正）：
 * 应用级配置 + **数据库注册表**（databases/activeDatabaseId——应用可登记
 * 多个数据库并切换激活库，为多用户协作预埋；由 database_* 命令族独占维护，
 * settings_set 对这两个字段原样保留后端真值）。照片库登记表由激活数据库
 * photos_libraries 承载（photo_library_list 等命令）。旧键（libraries/
 * activeLibraryId/单库 databaseDir）由后端 serde 忽略（不迁移）。
 */
export interface Settings {
  schemaVersion: number;
  onboardingCompleted: boolean;
  /** 数据库注册表（只读回显；变更走 database_* 命令族，见 ipc/api/databases） */
  databases: { id: string; name: string; dbDir: string }[];
  /** 激活数据库 id（null=尚未创建数据库；切换走 database_switch） */
  activeDatabaseId: string | null;
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
    eyesIncludeSingle?: boolean;
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
  databases: [],
  activeDatabaseId: null,
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
  /** 会话级「已选数据库」标志（达芬奇式启动流，非持久——每次启动都先过
   *  /database-picker 选库）：选择页/引导打开数据库后置 true；GatedShell
   *  据此放行主壳（老模型 libraryChosen 的新名） */
  databaseChosen: boolean;
  setDatabaseChosen: (chosen: boolean) => void;
  /** 从 Rust 侧读取设置；命令尚不存在或失败时静默落回默认值 */
  load: () => Promise<void>;
  /** 持久化设置；失败时恢复原状态并把原因交给操作界面 */
  save: (next: Settings) => Promise<void>;
  /** 本地局部更新（不落盘），由调用方决定何时 save */
  update: (partial: DeepPartial<Settings>) => void;
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

export const useSettingsStore = create<SettingsState>((set, get) => ({
  settings: clone(DEFAULT_SETTINGS),
  loaded: false,
  databaseChosen: false,
  setDatabaseChosen: (chosen) => set({ databaseChosen: chosen }),

  load: async () => {
    try {
      const remote = await ipc<Settings>("settings_get");
      // 远端可能是旧 schema（残留 libraries 等退役键被后端 serde 忽略后不回传），
      // 用默认值兜底合并，避免字段缺失
      set({ settings: mergeDeep(clone(DEFAULT_SETTINGS), remote), loaded: true });
    } catch {
      // Rust 命令由并行任务实现，可能尚不存在；绝不能抛错
      set({ settings: clone(DEFAULT_SETTINGS), loaded: true });
    }
  },

  save: async (next: Settings) => {
    const previous = get().settings;
    set({ settings: next });
    try {
      await ipc("settings_set", { settings: next });
    } catch (e) {
      // Only roll back this write if no subsequent operation has replaced it.
      if (get().settings === next) set({ settings: previous });
      console.error("settings_set failed:", e);
      throw e;
    }
  },

  update: (partial: DeepPartial<Settings>) => {
    set({ settings: mergeDeep(clone(get().settings), partial) });
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
        settings: mergeDeep(clone(DEFAULT_SETTINGS), event.payload),
      });
    });
  } catch {
    // 事件监听失败静默（如非 Tauri 环境下运行 vite dev 预览）
  }
}
