import { create } from "zustand";

import { cullSessionList } from "@/ipc/api";

/**
 * 选片域轻量状态（V1）：进行中会话数（侧栏「选片」徽标）。
 * 侧栏常驻挂载，本地拉取看不到跨页变化——集中一处由各入口（会话页挂载/
 * 建会话/收尾/丢弃）显式 refresh，避免徽标滞留旧值。会话清单本体由
 * CullingPage 自持（本 store 只养徽标计数，不缓存列表）。
 */

interface CullingStore {
  /** 进行中（finishedAt=null）会话数；0 不显示徽标 */
  activeCount: number;
  /** 重拉进行中会话数（命令失败静默保持原值——后端未就绪不误清零） */
  refreshActiveCount: () => Promise<void>;
}

export const useCullingStore = create<CullingStore>((set) => ({
  activeCount: 0,
  refreshActiveCount: async () => {
    const list = await cullSessionList();
    set({ activeCount: list.filter((s) => s.finishedAt === null).length });
  },
}));
