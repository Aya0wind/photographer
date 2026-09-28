import { useTranslation } from "react-i18next";

import type { AssetKind, CullDecisionValue } from "@/ipc/api";
import { useAssetThumbUrl } from "@/features/gallery/lib/thumbPipeline";
import type { CullDecisionMap } from "../lib/cullingCore";

/**
 * 选片对比视图（V2，方案 §3.2）：同屏 2/3/4 张，每张独立标记。
 * - 屏位成员由 cullingCore.comparisonMembers 选取（同连拍组优先，不足补相邻），
 *   本组件只负责铺格与交互回传（组员/焦点/决定全在浮层状态——进出对比保持单图位置）
 * - 键盘流：1-4 选焦（focusedPane 高亮 + 屏位号）→ 空格/X/U 作用于焦点张；
 *   鼠标流：点张选焦，张上 ✓/✗/↺ 按钮
 * - AI 预标记（origin='ai'）决定角标蓝描边 + AI 小标（与单图/胶片条一致）
 * - 缩略图走既有管线（对比格用 640 档，priority high——主视图）
 */

/** 对比格缩略图档位（2-4 分屏单格尺寸有限；RAW 由管线内部降档处理） */
const COMPARE_THUMB_SIZE = 640;

interface ComparePaneProps {
  pane: number;
  assetId: number;
  name: string;
  kind: AssetKind;
  decision: CullDecisionValue | null;
  aiOrigin: boolean;
  focused: boolean;
  onFocus: (pane: number) => void;
  onDecide: (assetId: number, decision: CullDecisionValue | null) => void;
}

function ComparePane({
  pane,
  assetId,
  name,
  kind,
  decision,
  aiOrigin,
  focused,
  onFocus,
  onDecide,
}: ComparePaneProps) {
  const { t } = useTranslation();
  const { url, status } = useAssetThumbUrl(assetId, COMPARE_THUMB_SIZE, true, "high");
  const stop = (e: React.MouseEvent): void => {
    e.stopPropagation();
  };

  return (
    <div
      role="button"
      tabIndex={0}
      aria-label={name}
      aria-pressed={focused}
      onClick={() => onFocus(pane)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onFocus(pane);
        }
      }}
      className={`relative flex min-h-0 min-w-0 select-none flex-col overflow-hidden rounded-lg border-2 bg-panel/40 transition-colors ${
        focused ? "border-accent" : "border-transparent hover:border-edge"
      }`}
      data-testid="culling-compare-pane"
      data-pane={pane}
      data-asset-id={assetId}
      data-decision={decision ?? "none"}
      data-origin={aiOrigin ? "ai" : "manual"}
      data-focused={focused}
    >
      {/* 图区（object-contain；屏位号 + 决定角标叠加） */}
      <div className="relative min-h-0 flex-1">
        {url !== null ? (
          <img
            src={url}
            alt={name}
            loading="eager"
            decoding="async"
            draggable={false}
            className="absolute inset-0 h-full w-full object-contain"
          />
        ) : (
          <span className="absolute inset-0 flex items-center justify-center font-mono text-xs text-text-muted">
            {name}
          </span>
        )}
        {/* 屏位号 1-4（键盘选焦提示） */}
        <span
          className={`absolute left-1.5 top-1.5 flex h-5 w-5 items-center justify-center rounded-full font-mono text-[11px] font-bold leading-none shadow ${
            focused ? "bg-accent text-black" : "bg-black/60 text-text-primary"
          }`}
          data-testid="culling-compare-pane-num"
        >
          {pane + 1}
        </span>
        {/* 决定角标：绿✓/红✗；AI 预标记蓝描边 + AI 小标 */}
        {decision !== null && (
          <span
            className={`absolute right-1.5 top-1.5 flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-semibold shadow ${
              decision === "accepted"
                ? "bg-emerald-500 text-black"
                : "bg-red-500 text-white"
            } ${aiOrigin ? "ring-2 ring-sky-400" : ""}`}
            data-testid="culling-compare-decision"
            data-decision={decision}
            data-ai={aiOrigin}
          >
            {decision === "accepted"
              ? t("culling.overlay.decisionAccepted")
              : t("culling.overlay.decisionRejected")}
            {aiOrigin && (
              <span className="rounded bg-black/25 px-1 text-[8px] font-semibold leading-3">
                {t("culling.overlay.ai.badge")}
              </span>
            )}
          </span>
        )}
        {/* RAW 角标（与胶片条同款） */}
        {kind === "raw" && (
          <span
            className="absolute bottom-1.5 right-1.5 rounded bg-black/60 px-1 font-mono text-[8px] font-bold leading-3 text-white"
            data-testid="culling-compare-raw"
          >
            RAW
          </span>
        )}
        {/* 源缺失角标（管线结算 missing，终态）：有缓存图仍显示，恒标「源缺失」 */}
        {status === "missing" && (
          <span
            className="absolute bottom-1.5 left-1.5 rounded bg-amber-500/85 px-1.5 py-0.5 text-[10px] font-medium leading-none text-black"
            data-testid="cull-pane-missing"
          >
            {t("thumb.missingBadge")}
          </span>
        )}
      </div>

      {/* 张上独立标记按钮（✓/✗/↺ = 空格/X/U 的鼠标等价物；不翻页） */}
      <div
        className="flex h-9 shrink-0 items-center justify-center gap-1.5 border-t border-edge/60 bg-surface/70"
        onClick={stop}
        data-testid="culling-compare-actions"
      >
        <button
          type="button"
          onClick={() => onDecide(assetId, "accepted")}
          aria-label={t("culling.overlay.accept")}
          className="rounded bg-emerald-500 px-2 py-0.5 text-[11px] font-medium text-black transition-colors hover:brightness-110"
          data-testid="culling-compare-accept"
        >
          ✓ {t("culling.overlay.accept")}
        </button>
        <button
          type="button"
          onClick={() => onDecide(assetId, null)}
          aria-label={t("culling.overlay.undecided")}
          className="rounded border border-edge px-2 py-0.5 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
          data-testid="culling-compare-undo"
        >
          ↺ {t("culling.overlay.undecided")}
        </button>
        <button
          type="button"
          onClick={() => onDecide(assetId, "rejected")}
          aria-label={t("culling.overlay.reject")}
          className="rounded bg-red-500 px-2 py-0.5 text-[11px] font-medium text-white transition-colors hover:brightness-110"
          data-testid="culling-compare-reject"
        >
          ✗ {t("culling.overlay.reject")}
        </button>
      </div>
    </div>
  );
}

export interface CullCompareGridProps {
  /** 屏位成员（会话内索引 → assetId/name；长度即实际屏位数 ≤ count） */
  panes: ReadonlyArray<{ assetId: number; name: string }>;
  /** 同屏张数（2/3/4；仅决定布局列数） */
  count: 2 | 3 | 4;
  decisions: CullDecisionMap;
  origins: ReadonlyMap<number, "manual" | "ai">;
  assetMeta: ReadonlyMap<number, { kind: AssetKind; name: string }>;
  /** 焦点屏位（0 基；键盘空格/X/U 作用对象） */
  focusedPane: number;
  onFocusPane: (pane: number) => void;
  onDecide: (assetId: number, decision: CullDecisionValue | null) => void;
}

export default function CullCompareGrid({
  panes,
  count,
  decisions,
  origins,
  assetMeta,
  focusedPane,
  onFocusPane,
  onDecide,
}: CullCompareGridProps) {
  const { t } = useTranslation();
  const columns = count === 3 ? "grid-cols-3" : "grid-cols-2"; // 4 → 2×2

  return (
    <div
      className="relative flex min-h-0 flex-1 flex-col"
      data-testid="culling-compare-grid"
      data-count={count}
    >
      <div className={`grid min-h-0 flex-1 gap-1.5 p-1.5 ${columns}`}>
        {panes.map((member, pane) => (
          <ComparePane
            key={member.assetId}
            pane={pane}
            assetId={member.assetId}
            name={member.name}
            kind={assetMeta.get(member.assetId)?.kind ?? "photo"}
            decision={decisions.get(member.assetId) ?? null}
            aiOrigin={origins.get(member.assetId) === "ai"}
            focused={pane === focusedPane}
            onFocus={onFocusPane}
            onDecide={onDecide}
          />
        ))}
      </div>
      <p
        className="pointer-events-none absolute bottom-3 left-1/2 -translate-x-1/2 rounded bg-black/40 px-2 py-0.5 font-mono text-[10px] text-text-muted/80"
        data-testid="culling-compare-keys-hint"
      >
        {t("culling.overlay.compareKeysHint")}
      </p>
    </div>
  );
}
