import { useCallback, useEffect, useRef, useState } from "react";

import {
  assetsByIds,
  isIpcAvailable,
  searchSemantic,
  type AssetDto,
  type SemanticHit,
} from "@/ipc/api";
import { useAiStore } from "@/stores/aiStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { tierSemanticReady } from "@/features/settings/lib/qualityTier";

/**
 * 语义搜索（M4）：searchSemantic → 命中 id → assets_by_ids 回填 AssetDto。
 * 状态机：
 * - idle：未搜索
 * - loading：请求在途
 * - ready：有结果（assets 与 hits 同序，scores 供角标）
 * - modelNotReady：后端明确报告模型未下载——引导去设置
 * - error：查询或结果加载失败，展示实际原因
 * - unavailable：IPC 传输失败（后端未连接）
 * - empty：模型就绪但无命中
 * 模型未就绪时命令返回明确错误字符串（Rust Err）——ipc() 保持 ipcAvailable=true，
 * 以此与传输失败（ipcAvailable=false）区分。
 */

export type SemanticStatus =
  | "idle"
  | "loading"
  | "ready"
  | "empty"
  | "modelNotReady"
  | "unavailable"
  | "error";

export interface SemanticSearchState {
  status: SemanticStatus;
  errorMessage?: string;
  assets: AssetDto[];
  /** assetId → 相似度（0..1） */
  scores: Map<number, number>;
}

export function useSemanticSearch() {
  const [state, setState] = useState<SemanticSearchState>({
    status: "idle",
    assets: [],
    scores: new Map(),
  });
  const seqRef = useRef(0);

  const run = useCallback(async (query: string, limit = 100, minScore?: number) => {
    const trimmed = query.trim();
    if (!trimmed) return;
    const seq = ++seqRef.current;
    setState({ status: "loading", assets: [], scores: new Map() });
    try {
      const hits: SemanticHit[] = await searchSemantic(trimmed, limit, minScore);
      if (seq !== seqRef.current) return;
      if (hits.length === 0) {
        setState({ status: "empty", assets: [], scores: new Map() });
        return;
      }
      const assets = await assetsByIds(hits.map((h) => h.assetId), true);
      if (seq !== seqRef.current) return;
      const byId = new Map(assets.map((a) => [a.id, a]));
      const ordered = hits.map((h) => byId.get(h.assetId)).filter((a): a is AssetDto => a !== undefined);
      setState({ status: ordered.length ? "ready" : "empty", assets: ordered, scores: new Map(hits.map((h) => [h.assetId, h.score])) });
    } catch (error) {
      if (seq !== seqRef.current) return;
      const message = error instanceof Error ? error.message : String(error);
      const missingModel = /(?:模型|分词器).*未下载|model.*not.*downloaded/i.test(message);
      setState({ status: !isIpcAvailable() ? "unavailable" : missingModel ? "modelNotReady" : "error", errorMessage: message, assets: [], scores: new Map() });
      void useAiStore.getState().refresh();
    }
  }, []);

  const reset = useCallback(() => {
    seqRef.current += 1;
    setState({ status: "idle", assets: [], scores: new Map() });
  }, []);

  return { ...state, run, reset };
}

/** 语义索引进行中（indexTaskProgress kind="ai"，done<total；null=未在进行） */
export function useAiIndexingProgress(): { done: number; total: number } | null {
  const indexStatus = useAiStore((s) => s.indexStatus);
  const indexProgress = useAiStore((s) => s.indexProgress);
  const refreshIndexStatus = useAiStore((s) => s.refreshIndexStatus);

  // 搜索页/智能相册挂载时都读取同一份持久化任务账；即使索引在别的页面
  // 或上次应用运行中完成，也不会被旧的瞬时事件永久留在“正在建立”。
  useEffect(() => {
    void refreshIndexStatus();
  }, [refreshIndexStatus]);

  if (indexStatus !== null) {
    const ai = indexStatus.ai;
    if (ai.pending === 0 && ai.running === 0) return null;
    if (ai.done >= ai.total) return null;
    return { done: ai.done, total: ai.total };
  }
  if (indexProgress === null || indexProgress.done >= indexProgress.total) return null;
  return { done: indexProgress.done, total: indexProgress.total };
}

// --- 语义搜索前置门禁 ----------------------------------------------------------------

export type SemanticGateReason = "models" | "index";

export interface SemanticGate {
  /** true=禁止发起语义查询（模型未齐或语义索引从未建立） */
  blocked: boolean;
  /** 拦截原因（blocked=false 时为 null） */
  reason: SemanticGateReason | null;
}

/**
 * 语义搜索前置门禁（判据从已有 store/IPC 派生，不新增后端）：
 * - models：当前库档位需要的语义三件（visual/text/tokenizer）未全部 done → 禁
 * - index：语义索引从未跑过（index_status 的 ai.total==0）且库内有资产
 *   （thumb/exif 通道 total>0 佐证非空库——空库不算未建立）→ 禁
 * 状态未加载（modelsLoaded=false）时放行——加载窗口内由后端拒绝兜底
 * （searchSemantic Err → modelNotReady 引导卡）；挂载即拉一次两份快照。
 */
export function useSemanticGate(): SemanticGate {
  const tier = useSettingsStore((s) => s.settings.ai.qualityTier);
  const models = useAiStore((s) => s.models);
  const modelsLoaded = useAiStore((s) => s.modelsLoaded);
  const indexStatus = useAiStore((s) => s.indexStatus);
  const refresh = useAiStore((s) => s.refresh);
  const refreshIndexStatus = useAiStore((s) => s.refreshIndexStatus);

  useEffect(() => {
    void refresh();
    void refreshIndexStatus();
  }, [refresh, refreshIndexStatus]);

  if (!modelsLoaded) return { blocked: false, reason: null };
  if (!tierSemanticReady(tier, models)) {
    return { blocked: true, reason: "models" };
  }
  if (indexStatus !== null && indexStatus.ai.total === 0) {
    const libraryHasAssets = indexStatus.thumb.total > 0 || indexStatus.exif.total > 0;
    if (libraryHasAssets) return { blocked: true, reason: "index" };
  }
  return { blocked: false, reason: null };
}
