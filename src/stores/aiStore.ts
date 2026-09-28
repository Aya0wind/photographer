import { create } from "zustand";
import { subscribeAppEvents, type AppEvent } from "@/ipc/api";

import {
  aiModelsStatus,
  indexStatus,
  type AiModelStatus,
  type IndexStatus,
} from "@/ipc/api";

let indexStatusRequest = 0;

/**
 * AI 模型/索引状态（M4）：设置页 AI tab 与语义搜索共用。
 * - models：ai_models_status 快照（refresh 拉取；事件 aiModelDownloadFinished 后重拉）
 * - downloadProgress：aiModelDownloadProgress 节流事件（单模型下载进度条）
 * - indexProgress：indexTaskProgress（kind="ai" 语义索引进度，"正在建立语义索引（N/M）"）
 * - indexStatus：index_status 三类索引计数快照（缩略图/EXIF/语义）——进相关页面拉一次，
 *   任意 indexTaskProgress/indexTaskResumed 事件后重拉（事件本身节流 1s，由后端负责）
 * 事件经唯一通道 app://event（initAi 幂等订阅；测试可直接 handleAppEvent 驱动）。
 */

export interface AiDownloadProgress {
  doneBytes: number;
  totalBytes: number;
}

export interface AiIndexProgress {
  kind: string;
  done: number;
  total: number;
}

interface AiState {
  models: AiModelStatus[];
  /** null=尚未拉取过（首屏 loading） */
  modelsLoaded: boolean;
  downloadProgress: Record<string, AiDownloadProgress>;
  indexProgress: AiIndexProgress | null;
  /** 三类索引计数（index_status）；null=未拉取/后端不可用 */
  indexStatus: IndexStatus | null;

  /** 拉取模型状态（设置页挂载/下载结束后调用） */
  refresh: () => Promise<void>;
  /** 拉取索引状态快照（相关页面挂载/索引进度事件后调用） */
  refreshIndexStatus: () => Promise<void>;
  /** 事件入口（initAi 订阅转发；测试可直接驱动） */
  handleAppEvent: (event: AppEvent) => void;
  /** 切库时清索引态（索引任务/进度按库私有；模型清单是应用级不动） */
  resetLibrarySession: () => void;
  /** 仅测试用：清空状态 */
  resetForTests: () => void;
}

export const useAiStore = create<AiState>((set, get) => ({
  models: [],
  modelsLoaded: false,
  downloadProgress: {},
  indexProgress: null,
  indexStatus: null,

  refresh: async () => {
    const models = await aiModelsStatus();
    set({ models, modelsLoaded: true });
  },

  refreshIndexStatus: async () => {
    const request = ++indexStatusRequest;
    const status = await indexStatus();
    // 高频进度事件会并发重拉；较早的慢响应不能覆盖较新的完成快照。
    if (request === indexStatusRequest) set({ indexStatus: status });
  },

  handleAppEvent: (event) => {
    switch (event.type) {
      case "aiModelDownloadProgress": {
        set((s) => ({
          downloadProgress: {
            ...s.downloadProgress,
            [event.id]: { doneBytes: event.doneBytes, totalBytes: event.totalBytes },
          },
        }));
        break;
      }
      case "aiModelDownloadFinished": {
        // 进度条退场 + 状态快照重拉（done/failed/idle 由后端落定）
        set((s) => {
          const next = { ...s.downloadProgress };
          delete next[event.id];
          return { downloadProgress: next };
        });
        void get().refresh();
        break;
      }
      case "importFileProgress":
      case "importSessionFinished":
      case "indexTaskResumed": {
        void get().refreshIndexStatus();
        break;
      }
      case "indexTaskProgress": {
        if (event.kind === "ai") {
          set({ indexProgress: { kind: event.kind, done: event.done, total: event.total } });
        }
        // 任一索引推进 → 三类计数快照重拉（事件已由后端节流）
        void get().refreshIndexStatus();
        break;
      }
      default:
        break;
    }
  },

  resetLibrarySession: () => {
    set({ indexProgress: null, indexStatus: null });
  },

  resetForTests: () => {
    indexStatusRequest = 0;
    set({
      models: [],
      modelsLoaded: false,
      downloadProgress: {},
      indexProgress: null,
      indexStatus: null,
    });
  },
}));

let eventsBound = false;

/** 应用启动时调用一次（幂等）：订阅模型下载/索引进度事件 */
export async function initAi(): Promise<void> {
  if (eventsBound) return;
  eventsBound = true;
  try {
    await subscribeAppEvents((event) => useAiStore.getState().handleAppEvent(event));
  } catch {
    // 非 Tauri 环境（vite dev 预览）静默
  }
}
