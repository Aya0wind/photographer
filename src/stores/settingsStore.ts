import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";

import { ipc } from "@/ipc";

export interface Settings {
  schemaVersion: number;
  libraryRoot: string | null;
  onboardingCompleted: boolean;
  import: {
    promptOnDevice: boolean;
    skipImported: boolean;
    dirTemplate: string;
    duplicatePolicy: "skip" | "rename" | "ask";
    notifyMilestones: boolean;
  };
  ai: {
    enableClip: boolean;
    enableFace: boolean;
    enableSceneTags: boolean;
    indexSchedule: "idleOnly" | "afterImport" | "manual";
    cpuLimitPercent: number;
    useGpu: boolean;
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
  libraryRoot: null,
  onboardingCompleted: false,
  import: {
    promptOnDevice: true,
    skipImported: true,
    dirTemplate: "{YYYY}/{MM-DD}/{原文件名}",
    duplicatePolicy: "skip",
    notifyMilestones: true,
  },
  ai: {
    enableClip: false,
    enableFace: false,
    enableSceneTags: false,
    indexSchedule: "idleOnly",
    cpuLimitPercent: 50,
    useGpu: true,
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
  /** 从 Rust 侧读取设置；命令尚不存在或失败时静默落回默认值 */
  load: () => Promise<void>;
  /** 持久化设置；IPC 失败时本地状态仍保持更新 */
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

export const useSettingsStore = create<SettingsState>((set, get) => ({
  settings: clone(DEFAULT_SETTINGS),
  loaded: false,

  load: async () => {
    try {
      const remote = await ipc<Settings>("settings_get");
      // 远端可能是旧 schema，用默认值兜底合并，避免字段缺失
      set({ settings: mergeDeep(clone(DEFAULT_SETTINGS), remote), loaded: true });
    } catch {
      // Rust 命令由并行任务实现，可能尚不存在；绝不能抛错
      set({ settings: clone(DEFAULT_SETTINGS), loaded: true });
    }
  },

  save: async (next: Settings) => {
    // 先更新本地，再尝试持久化；失败时本地仍保持新值
    set({ settings: clone(next) });
    try {
      await ipc("settings_set", { settings: next });
    } catch {
      // 忽略持久化失败（如 Rust 命令尚未实现）
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
