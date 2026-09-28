import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import type { AssetKind } from "@/ipc/api";
import { useAssetThumbUrl } from "@/features/gallery/lib/thumbPipeline";
import type { CullDecisionMap } from "../lib/cullingCore";

/**
 * 选片胶片条（V1）：底部横向虚拟滚动缩略图带（方案 §3.1——万张会话只渲染
 * 视口窗口，点击跳转；决定角标绿✓/红✗，AI 预标记附 AI 小徽）。
 *
 * 虚拟化不引第三方虚拟器：单元格定宽（CELL_W+GAP 步进），由 scrollLeft 直接
 * 推导可见窗口（±OVERSCAN），绝对定位铺进总宽容器——jsdom 无布局也能确定性
 * 驱动（测试设 scrollLeft + fire scroll 即可断言窗口）。
 */

const CELL_W = 68;
const CELL_H = 60;
const GAP = 6;
const STEP = CELL_W + GAP;
const OVERSCAN = 10;

/** 视口推导（纯函数，测试共用）：[start, end) 渲染窗口 */
export function filmstripWindow(
  count: number,
  scrollLeft: number,
  viewportWidth: number,
): { start: number; end: number } {
  const start = Math.max(0, Math.floor(scrollLeft / STEP) - OVERSCAN);
  const end = Math.min(
    count,
    Math.ceil((scrollLeft + Math.max(viewportWidth, CELL_W)) / STEP) + OVERSCAN,
  );
  return { start: Math.min(start, end), end };
}

/** 单格缩略图（管线优先级 low——主图永远先走） */
function FilmCell({
  assetId,
  kind,
  name,
  decision,
  aiOrigin,
  current,
  index,
  onJump,
}: {
  assetId: number;
  kind: AssetKind;
  name: string;
  decision: "accepted" | "rejected" | null;
  aiOrigin: boolean;
  current: boolean;
  index: number;
  onJump: (index: number) => void;
}) {
  const { url } = useAssetThumbUrl(assetId, 240, true, "low");
  return (
    <button
      type="button"
      onClick={() => onJump(index)}
      aria-label={name}
      aria-current={current}
      title={name}
      className={`relative shrink-0 overflow-hidden rounded-md border-2 bg-panel/50 transition-colors ${
        current ? "border-accent" : "border-transparent hover:border-edge"
      }`}
      style={{ width: CELL_W, height: CELL_H }}
      data-testid="cull-film-cell"
      data-index={index}
      data-asset-id={assetId}
      data-decision={decision ?? "none"}
      data-current={current}
    >
      {url !== null ? (
        <img
          src={url}
          alt=""
          loading="eager"
          decoding="async"
          draggable={false}
          className="h-full w-full object-cover"
        />
      ) : (
        <span className="absolute inset-0 flex items-center justify-center font-mono text-[10px] tabular-nums text-text-muted">
          {index + 1}
        </span>
      )}
      {/* 决定角标：绿✓ / 红✗；AI 预标记附小徽（方案：AI 只建议） */}
      {decision === "accepted" && (
        <span
          className="absolute left-0.5 top-0.5 flex h-4 w-4 items-center justify-center rounded-full bg-emerald-500 text-[10px] font-bold leading-none text-black"
          data-testid="cull-film-accepted"
        >
          ✓
        </span>
      )}
      {decision === "rejected" && (
        <span
          className="absolute left-0.5 top-0.5 flex h-4 w-4 items-center justify-center rounded-full bg-red-500 text-[10px] font-bold leading-none text-white"
          data-testid="cull-film-rejected"
        >
          ✗
        </span>
      )}
      {aiOrigin && decision !== null && (
        <span
          className="absolute bottom-0.5 right-0.5 rounded bg-black/70 px-1 text-[8px] font-semibold leading-3 text-sky-300"
          data-testid="cull-film-ai"
        >
          AI
        </span>
      )}
      {/* RAW 角标（与画廊瓦片同款：过片时一眼区分 RAW/JPG） */}
      {kind === "raw" && (
        <span
          className="absolute right-0.5 top-0.5 rounded bg-black/60 px-1 font-mono text-[8px] font-bold leading-3 text-white"
          data-testid="cull-film-raw"
        >
          RAW
        </span>
      )}
    </button>
  );
}

export interface CullFilmstripProps {
  /** 会话资产 id 快照序（与决定表同源） */
  ids: number[];
  decisions: CullDecisionMap;
  /** AI 预标记来源（origin='ai'） */
  origins: ReadonlyMap<number, "manual" | "ai">;
  /** 已加载的资产元数据（kind 用于占位；未加载格显示序号） */
  assetMeta: ReadonlyMap<number, { kind: AssetKind; name: string }>;
  index: number;
  onJump: (index: number) => void;
}

export default function CullFilmstrip({
  ids,
  decisions,
  origins,
  assetMeta,
  index,
  onJump,
}: CullFilmstripProps) {
  const { t } = useTranslation();
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [scrollLeft, setScrollLeft] = useState(0);
  const [width, setWidth] = useState(0);

  // 尺寸测量（mount + 滚动时顺带刷新；jsdom 无 ResizeObserver 回调也无妨）
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    setWidth(el.clientWidth);
    if (typeof ResizeObserver !== "undefined") {
      const ro = new ResizeObserver(() => setWidth(el.clientWidth));
      ro.observe(el);
      return () => ro.disconnect();
    }
    return undefined;
  }, []);

  // 当前格保持可见（键盘流翻片自动跟随；jsdom scrollTo 缺失时直接写 scrollLeft）
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const left = index * STEP;
    if (left < el.scrollLeft || left + CELL_W > el.scrollLeft + Math.max(el.clientWidth, CELL_W)) {
      const target = Math.max(0, left - Math.max(el.clientWidth, CELL_W) / 2);
      if (typeof el.scrollTo === "function") {
        el.scrollTo({ left: target });
      } else {
        el.scrollLeft = target;
      }
      setScrollLeft(target);
      setWidth(el.clientWidth);
    }
  }, [index]);

  const { start, end } = filmstripWindow(ids.length, scrollLeft, width);
  const cells = [];
  for (let i = start; i < end; i += 1) {
    const id = ids[i];
    const meta = assetMeta.get(id);
    cells.push(
      <FilmCell
        key={id}
        assetId={id}
        kind={meta?.kind ?? "photo"}
        name={meta?.name ?? `#${i + 1}`}
        decision={decisions.get(id) ?? null}
        aiOrigin={origins.get(id) === "ai"}
        current={i === index}
        index={i}
        onJump={onJump}
      />,
    );
  }

  return (
    <div
      className="flex h-[72px] shrink-0 items-center border-t border-edge bg-surface/80"
      data-testid="cull-filmstrip"
      data-count={ids.length}
    >
      <div
        ref={scrollRef}
        className="sp-scroll h-full w-full overflow-x-auto px-3"
        onScroll={(e) => {
          setScrollLeft(e.currentTarget.scrollLeft);
          setWidth(e.currentTarget.clientWidth);
        }}
        aria-label={t("culling.overlay.filmstrip")}
      >
        <div className="relative flex items-center" style={{ height: "100%", width: ids.length * STEP }}>
          {/* 绝对定位虚拟窗口（步进 = CELL_W+GAP） */}
          <div
            className="absolute inset-y-0 flex items-center gap-[6px]"
            style={{ left: start * STEP }}
            data-testid="cull-film-window"
            data-window={`${start}:${end}`}
          >
            {cells}
          </div>
        </div>
      </div>
    </div>
  );
}
