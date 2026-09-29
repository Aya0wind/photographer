import { usePhotoCards } from "@/features/gallery/lib/usePhotoCards";
import { useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router";
import { useTranslation } from "react-i18next";

import { indexKickNow, type AssetDto } from "@/ipc/api";
import { useAiStore } from "@/stores/aiStore";
import { groupAssetsByDate } from "@/features/gallery/lib/assetGroups";
import AssetGrid from "@/features/gallery/components/AssetGrid";
import { useAiIndexingProgress } from "./useSemanticSearch";

/**
 * 语义搜索结果视图（搜索页语义模式 / 智能相册共用）：
 * 状态化呈现——索引进度（首次需索引）/ loading / 模型未就绪引导卡（去设置）/
 * 后端未连接 / 空结果（附「语义索引建立中」提示 + 立即索引）/ AssetGrid 结果
 * （相似度百分比角标右下，分档样式见 scoreBadge）。
 */

export interface SemanticResultsViewProps {
  status: "idle" | "loading" | "ready" | "empty" | "modelNotReady" | "unavailable";
  assets: AssetDto[];
  scores: Map<number, number>;
  /** 结果容器高度撑满（调用方布局内） */
  scrollTestId?: string;
  /** 点击资产打开查看器（真机修复 2026-09-20：语义结果此前纯展示点不开） */
  onOpenAsset?: (asset: AssetDto, group: { key: string; date: string | null; assets: AssetDto[] }) => void;
  /** 触发重试（未就绪引导的「重试」入口；可选） */
  onRetry?: () => void;
}

/** 空结果附加提示：库内语义索引未建完（ai.done<total）→「建立中（N/M）」+ 立即索引 */
function SemanticIndexBuildingHint() {
  const { t } = useTranslation();
  const status = useAiStore((s) => s.indexStatus);
  const refreshIndexStatus = useAiStore((s) => s.refreshIndexStatus);
  const [kicked, setKicked] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void refreshIndexStatus();
  }, [refreshIndexStatus]);

  if (status === null || status.ai.total <= 0 || status.ai.done >= status.ai.total) return null;

  async function kick(): Promise<void> {
    setError(null);
    try {
      await indexKickNow("ai");
    } catch (err) {
      setError(err instanceof Error ? err.message : typeof err === "string" ? err : null);
      return;
    }
    setKicked(true);
    void refreshIndexStatus();
  }

  return (
    <div className="mt-2 flex flex-col items-center gap-2" data-testid="semantic-empty-indexing">
      <span className="text-[11px] text-text-muted">
        {t("search.semantic.indexBuilding", { done: status.ai.done, total: status.ai.total })}
      </span>
      {error !== null && (
        <span className="text-[11px] text-red-400" data-testid="semantic-empty-kick-error" role="alert">
          {error}
        </span>
      )}
      {kicked ? (
        <span className="text-[11px] text-emerald-400" data-testid="semantic-empty-kicked">
          {t("settings.ai.index.kicked")}
        </span>
      ) : (
        <button
          type="button"
          onClick={() => void kick()}
          className="rounded-md bg-accent px-3 py-1 text-[11px] font-medium text-black transition-colors hover:brightness-110"
          data-testid="semantic-empty-kick"
        >
          {t("settings.ai.index.kick")}
        </button>
      )}
    </div>
  );
}

export default function SemanticResultsView({
  status,
  assets,
  scores,
  scrollTestId = "semantic-grid-scroll",
  onOpenAsset,
  onRetry,
}: SemanticResultsViewProps) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const indexing = useAiIndexingProgress();
  const { cards, badges } = usePhotoCards(assets);
  const groups = useMemo(() => groupAssetsByDate(cards), [cards]);

  if (status === "loading") {
    return (
      <div className="flex h-full animate-pulse items-center justify-center text-xs text-text-muted" data-testid="semantic-loading">
        {t("search.semantic.loading")}
      </div>
    );
  }
  if (status === "modelNotReady") {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 px-8 text-center" data-testid="semantic-model-notready">
        <svg
          viewBox="0 0 24 24"
          width="44"
          height="44"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.2"
          strokeLinecap="round"
          strokeLinejoin="round"
          className="text-text-muted"
          aria-hidden="true"
        >
          <rect x="4" y="4" width="16" height="16" rx="3" />
          <path d="M9 2.5v3M15 2.5v3M4 9h16M9.5 13.5l1.6 1.6 3.4-3.4" />
        </svg>
        <h2 className="text-sm font-semibold text-text-primary">{t("search.semantic.notReady.title")}</h2>
        <p className="max-w-sm text-xs leading-relaxed text-text-muted">{t("search.semantic.notReady.desc")}</p>
        <div className="mt-1 flex gap-2">
          <button
            type="button"
            onClick={() => navigate("/settings?tab=ai")}
            className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110"
            data-testid="semantic-gosettings"
          >
            {t("search.semantic.notReady.goSettings")}
          </button>
          {onRetry && (
            <button
              type="button"
              onClick={onRetry}
              className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
              data-testid="semantic-retry"
            >
              {t("gallery.retry")}
            </button>
          )}
        </div>
      </div>
    );
  }
  if (status === "unavailable") {
    return (
      <div className="flex h-full items-center justify-center text-xs text-text-muted" data-testid="semantic-unavailable">
        {t("gallery.ipcUnavailable")}
      </div>
    );
  }
  if (status === "empty") {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 px-8 text-center" data-testid="semantic-empty">
        <p className="text-sm text-text-secondary">{t("search.semantic.empty")}</p>
        <p className="text-xs text-text-muted">{t("search.semantic.emptyHint")}</p>
        <SemanticIndexBuildingHint />
      </div>
    );
  }
  if (status !== "ready") return null;

  return (
    <div className="relative h-full min-h-0">
      <SemanticIndexingBanner progress={indexing} />
      <AssetGrid groups={groups} badges={badges} tile={200} scores={scores} onOpenAsset={onOpenAsset} scrollTestId={scrollTestId} />
    </div>
  );
}

/** 索引建立中细提示条（语义结果视图顶部；索引完成自动消失——progress 派生自
 *  持久化任务账，非本地瞬时态）。仅语义态渲染，默认画廊不弹。 */
export function SemanticIndexingBanner({
  progress,
}: {
  progress: { done: number; total: number } | null;
}) {
  const { t } = useTranslation();
  if (!progress) return null;
  return (
    <div
      className="absolute inset-x-0 top-0 z-10 flex h-7 items-center justify-center gap-2 border-b border-edge/60 bg-bg/85 text-[11px] text-text-secondary backdrop-blur-sm"
      data-testid="semantic-indexing"
    >
      <span
        className="h-3 w-3 animate-spin rounded-full border border-edge border-t-accent"
        aria-hidden="true"
      />
      {t("search.semantic.indexing", { done: progress.done, total: progress.total })}
    </div>
  );
}

/** 门禁拦截提示（行内）：模型未下载/索引未建立 → 原因文案 + 一键跳设置 AI tab。
 *  各语义入口（全局搜索框/画廊语义输入/智能相册）被拦时渲染；输入文字由
 *  调用方保留、不发查询。 */
export function SemanticGateNotice({
  reason,
  testId = "semantic-gate-notice",
}: {
  reason: "models" | "index";
  testId?: string;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  return (
    <div
      className="flex min-h-[40px] flex-wrap items-center justify-between gap-3 rounded-md border border-edge bg-surface px-3 py-2"
      role="alert"
      data-testid={testId}
    >
      <span className="min-w-0 text-xs text-text-secondary">
        {t(`search.semantic.gate.${reason}`)}
      </span>
      <button
        type="button"
        onClick={() => navigate("/settings?tab=ai")}
        className="shrink-0 rounded-md bg-accent px-2.5 py-1 text-[11px] font-medium text-black transition-colors hover:brightness-110"
        data-testid={`${testId}-gosettings`}
      >
        {t("search.semantic.gate.goSettings")}
      </button>
    </div>
  );
}

/** 语义查询输入（大文本框 + 搜索按钮）；搜索页语义模式专用布局，回车触发。
 *  initialQuery：URL 协议预填（/search?mode=semantic&q=…，全局搜索框跳入） */
export function SemanticQueryInput({
  onRun,
  busy,
  initialQuery = "",
}: {
  onRun: (query: string) => void;
  busy: boolean;
  initialQuery?: string;
}) {
  const { t } = useTranslation();
  const [value, setValue] = useState(initialQuery);
  const inputRef = useRef<HTMLTextAreaElement | null>(null);

  // 卸载/重挂不保留（每次进入语义模式为空起点）
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  return (
    <div className="flex min-w-0 flex-1 items-center gap-2" data-testid="semantic-query">
      <textarea
        ref={inputRef}
        rows={1}
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            if (!busy) onRun(value);
          }
        }}
        placeholder={t("search.semantic.placeholder")}
        aria-label={t("search.semantic.input")}
        className="h-8 min-w-0 flex-1 resize-none overflow-hidden rounded-md border border-edge bg-bg px-2.5 py-1.5 text-xs leading-4 text-text-primary outline-none transition-colors focus:border-accent"
        data-testid="semantic-input"
      />
      <button
        type="button"
        onClick={() => onRun(value)}
        disabled={busy || value.trim() === ""}
        className="shrink-0 rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
        data-testid="semantic-run"
      >
        {busy ? t("search.semantic.running") : t("search.semantic.run")}
      </button>
    </div>
  );
}
