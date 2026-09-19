import { useCallback, useRef, useState } from "react";

import {
  assetsByIds,
  isIpcAvailable,
  searchSemantic,
  type AssetDto,
  type SemanticHit,
} from "@/ipc/api";
import { useAiStore } from "@/stores/aiStore";

/**
 * 语义搜索（M4）：searchSemantic → 命中 id → assets_by_ids 回填 AssetDto。
 * 状态机：
 * - idle：未搜索
 * - loading：请求在途
 * - ready：有结果（assets 与 hits 同序，scores 供角标）
 * - modelNotReady：后端可达但拒绝（模型未下载/索引未建）——引导去设置
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
  | "unavailable";

export interface SemanticSearchState {
  status: SemanticStatus;
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
    let hits: SemanticHit[];
    try {
      hits = await searchSemantic(trimmed, limit, minScore);
    } catch {
      // 后端可达但拒绝（模型未就绪/索引未建） vs 传输失败
      setState({
        status: isIpcAvailable() ? "modelNotReady" : "unavailable",
        assets: [],
        scores: new Map(),
      });
      return;
    }
    if (seq !== seqRef.current) return; // 已有更新的搜索
    if (hits.length === 0) {
      setState({ status: "empty", assets: [], scores: new Map() });
      return;
    }
    const assets = await assetsByIds(hits.map((h) => h.assetId));
    if (seq !== seqRef.current) return;
    const byId = new Map(assets.map((a) => [a.id, a]));
    // 保持命中序（分数降序语义由后端保证）
    const ordered = hits.map((h) => byId.get(h.assetId)).filter((a): a is AssetDto => a !== undefined);
    setState({
      status: "ready",
      assets: ordered,
      scores: new Map(hits.map((h) => [h.assetId, h.score])),
    });
  }, []);

  const reset = useCallback(() => {
    seqRef.current += 1;
    setState({ status: "idle", assets: [], scores: new Map() });
  }, []);

  return { ...state, run, reset };
}

/** 语义索引进行中（indexTaskProgress kind="ai"，done<total；null=未在进行） */
export function useAiIndexingProgress(): { done: number; total: number } | null {
  const indexProgress = useAiStore((s) => s.indexProgress);
  if (indexProgress === null || indexProgress.done >= indexProgress.total) return null;
  return { done: indexProgress.done, total: indexProgress.total };
}
