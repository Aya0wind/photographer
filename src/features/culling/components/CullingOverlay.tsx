import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { convertFileSrc } from "@tauri-apps/api/core";

import {
  assetDetail,
  assetsByIds,
  cullDecisionApply,
  cullSessionOpen,
  type AssetDetailDto,
  type AssetDto,
  type CullFinishResult,
  type CullItemState,
  type CullSessionDto,
} from "@/ipc/api";
import {
  fetchAssetThumb,
  useAssetThumbUrl,
  warmImageDecode,
} from "@/features/gallery/lib/thumbPipeline";
import {
  cullKeyAction,
  deriveProgress,
  indexAfterDecision,
  withDecision,
  type CullDecision,
  type CullDecisionMap,
} from "../lib/cullingCore";
import CullFilmstrip from "./CullFilmstrip";
import CullFinishDialog from "./CullFinishDialog";

/**
 * 全屏选片层（V1 单图过片，方案 §3.1）：
 * - 键盘流：←/→ 翻片 · 空格/↑ 选入 · X/↓ 剔除 · U 回未定 · Z 按住放大（位置跟随
 *   鼠标）· Esc 退出回列表。决定即落库（乐观更新+失败回滚）——退出=保存，
 *   无确认弹窗（进度天然持久，方案定案）。
 * - 接管键盘：window 捕获阶段监听 + stopPropagation，压过全局快捷键/
 *   查看器键位（编辑器浮层同款让位思想；收尾弹窗打开时本层让位）。
 * - 大图走既有预览分级管线（photo：原图→2048→512；RAW：内嵌全幅→2048 显影
 *   →512），相邻预取；底部胶片条虚拟滚动（点击跳转，绿✓/红✗ 角标）。
 * - AI 行内提示 chips：闭眼/失焦（asset_detail.aiAnalysis；AI 只建议）。
 * - 顶栏带 data-tauri-drag-region 拖拽层（全屏浮层铁律——按钮区域
 *   pointer-events 正常，空白处落到拖拽层）。
 */

const VIEWER_MID_SIZE = 2048;
const VIEWER_RAW_EMBED_SIZE = 6000;
const VIEWER_THUMB_SIZE = 1280;
/** Z 按住放大倍率（位置=transform-origin 跟随鼠标；跨图保持为 V2） */
const ZOOM_SCALE = 2.5;

/** 资产元数据窗口加载（胶片条/主图用；万张会话不整表取回） */
const WINDOW_BEFORE = 24;
const WINDOW_AFTER = 72;
const ASSET_CHUNK = 60;

function safeConvert(path: string): string | null {
  try {
    return convertFileSrc(path) || null;
  } catch {
    return null;
  }
}

/** 断点续选位：首个未定项；全决定完停在末张（复盘用） */
export function resumeIndexOf(items: CullItemState[]): number {
  const first = items.findIndex((item) => item.decision === null);
  return first === -1 ? Math.max(0, items.length - 1) : first;
}

export interface CullingOverlayProps {
  session: CullSessionDto;
  /** Esc/关闭 → 回列表（决定已落库，无需确认） */
  onClose: () => void;
  /** 收尾完成回传（页面出摘要 toast + 刷新清单） */
  onFinished: (summary: CullFinishResult, sessionName: string) => void;
  /** finish 调用失败回传（页面 toast） */
  onFinishFailed: () => void;
}

export default function CullingOverlay({
  session: sessionProp,
  onClose,
  onFinished,
  onFinishFailed,
}: CullingOverlayProps) {
  const { t } = useTranslation();

  // --- 会话装载（open：决定表 + 快照序） ----------------------------------------------
  const [session, setSession] = useState<CullSessionDto>(sessionProp);
  const [items, setItems] = useState<CullItemState[] | null>(null);
  const [openFailed, setOpenFailed] = useState(false);
  const [decisions, setDecisions] = useState<CullDecisionMap>(new Map());
  const [origins, setOrigins] = useState<Map<number, "manual" | "ai">>(new Map());
  const [index, setIndex] = useState(0);

  useEffect(() => {
    let cancelled = false;
    void cullSessionOpen(sessionProp.id).then((opened) => {
      if (cancelled) return;
      if (opened === null) {
        setOpenFailed(true);
        return;
      }
      setSession(opened.session);
      setItems(opened.items);
      setDecisions(
        new Map(
          opened.items
            .filter((item) => item.decision !== null)
            .map((item) => [item.assetId, item.decision] as const),
        ),
      );
      setOrigins(new Map(opened.items.map((item) => [item.assetId, item.origin] as const)));
      setIndex(resumeIndexOf(opened.items));
    });
    return () => {
      cancelled = true;
    };
  }, [sessionProp.id]);

  const ids = useMemo(() => items?.map((item) => item.assetId) ?? [], [items]);
  const currentId = items !== null && index < items.length ? items[index].assetId : null;
  const progress = useMemo(
    () => deriveProgress(session.total, decisions),
    [session.total, decisions],
  );

  // --- 资产元数据窗口加载（assetsByIds 分块；已入队去重） ------------------------------
  const [assetById, setAssetById] = useState<Map<number, AssetDto>>(new Map());
  const requestedRef = useRef<Set<number>>(new Set());
  useEffect(() => {
    if (items === null || items.length === 0) return;
    const from = Math.max(0, index - WINDOW_BEFORE);
    const to = Math.min(items.length, index + WINDOW_AFTER);
    const missing: number[] = [];
    for (let i = from; i < to; i += 1) {
      const id = items[i].assetId;
      if (!requestedRef.current.has(id)) {
        requestedRef.current.add(id);
        missing.push(id);
      }
    }
    if (missing.length === 0) return;
    void (async () => {
      for (let offset = 0; offset < missing.length; offset += ASSET_CHUNK) {
        const list = await assetsByIds(missing.slice(offset, offset + ASSET_CHUNK));
        if (list.length === 0) continue;
        setAssetById((prev) => {
          const next = new Map(prev);
          for (const asset of list) next.set(asset.id, asset);
          return next;
        });
      }
    })();
  }, [items, index]);

  const asset = currentId !== null ? assetById.get(currentId) ?? null : null;
  /** 胶片条元数据投影（kind/name；未加载格回退序号占位） */
  const assetMeta = useMemo(() => {
    const map = new Map<number, { kind: AssetDto["kind"]; name: string }>();
    for (const a of assetById.values()) map.set(a.id, { kind: a.kind, name: a.name });
    return map;
  }, [assetById]);

  // --- 大图分级链（与查看器同款：photo 原图→2048→512；RAW 内嵌全幅→2048→512） -----------
  const originalUrl = useMemo(
    () => (asset?.kind === "photo" ? safeConvert(asset.path) : null),
    [asset?.kind, asset?.path],
  );
  const [photoStage, setPhotoStage] = useState<"original" | "mid" | "thumb">("original");
  useEffect(() => {
    setPhotoStage("original");
  }, [currentId]);
  const thumb = useAssetThumbUrl(currentId ?? 0, VIEWER_THUMB_SIZE, currentId !== null, "high");
  const mid = useAssetThumbUrl(
    currentId ?? 0,
    VIEWER_MID_SIZE,
    asset?.kind === "photo" && photoStage !== "original",
    "high",
  );
  const rawEmbed = useAssetThumbUrl(
    currentId ?? 0,
    VIEWER_RAW_EMBED_SIZE,
    asset?.kind === "raw",
    "high",
  );
  const rawFull = useAssetThumbUrl(
    currentId ?? 0,
    VIEWER_MID_SIZE,
    asset?.kind === "raw" && rawEmbed.status === "failed",
    "high",
  );

  let mainSrc: string | null = null;
  let mainFailed = false;
  if (asset?.kind === "photo") {
    if (photoStage === "original" && originalUrl !== null) mainSrc = originalUrl;
    else if (photoStage === "mid") mainSrc = mid.url;
    else mainSrc = thumb.url;
    mainFailed = thumb.status === "failed";
  } else if (asset?.kind === "raw") {
    mainSrc = rawEmbed.url ?? rawFull.url ?? thumb.url;
    mainFailed = rawEmbed.status === "failed" && rawFull.status === "failed" && thumb.status === "failed";
  }
  // photo 原图不可用（非 Tauri 环境/convert 抛错）→ 直达缩略图档
  useEffect(() => {
    if (asset?.kind === "photo" && photoStage === "original" && originalUrl === null) {
      setPhotoStage("thumb");
    }
  }, [asset?.kind, photoStage, originalUrl]);
  // 中间档确定无图（后端未加 2048 档）→ 自动降到 512 档
  useEffect(() => {
    if (asset?.kind === "photo" && photoStage === "mid" && mid.status === "failed") {
      setPhotoStage("thumb");
    }
  }, [asset?.kind, photoStage, mid.status]);

  // --- 无空窗图层（查看器同款精简版：新图解码完成才替换旧图） ---------------------------
  type ImageLayer = { src: string; phase: "active" | "loading" | "retiring" };
  const [imageLayers, setImageLayers] = useState<ImageLayer[]>([]);
  useEffect(() => {
    if (mainSrc === null) return;
    setImageLayers((previous) => {
      if (previous.some((layer) => layer.src === mainSrc)) return previous;
      const active = previous.find((layer) => layer.phase === "active");
      return active
        ? [active, { src: mainSrc, phase: "loading" }]
        : [{ src: mainSrc, phase: "loading" }];
    });
  }, [mainSrc]);
  useEffect(() => {
    if (!imageLayers.some((layer) => layer.phase === "retiring")) return;
    const frame = requestAnimationFrame(() => {
      setImageLayers((previous) => previous.filter((layer) => layer.phase !== "retiring"));
    });
    return () => cancelAnimationFrame(frame);
  }, [imageLayers]);
  useEffect(() => {
    if (mainFailed) setImageLayers([]);
  }, [mainFailed]);

  // --- Z 按住放大（transform scale + transform-origin 跟随鼠标） ------------------------
  const [zoomed, setZoomed] = useState(false);
  const [zoomAt, setZoomAt] = useState({ x: 0.5, y: 0.5 });
  const stageRef = useRef<HTMLDivElement | null>(null);
  function handleStageMouseMove(e: React.MouseEvent<HTMLDivElement>): void {
    if (!zoomed) return;
    const rect = e.currentTarget.getBoundingClientRect();
    setZoomAt({
      x: Math.min(1, Math.max(0, (e.clientX - rect.left) / Math.max(1, rect.width))),
      y: Math.min(1, Math.max(0, (e.clientY - rect.top) / Math.max(1, rect.height))),
    });
  }

  // --- 决定：乐观更新 + 失败回滚（决定即落库） -----------------------------------------
  const [rollbackAt, setRollbackAt] = useState<{ assetId: number; decision: CullDecision } | null>(null);
  // 回滚提示自动消退（2.5s；不阻塞继续过片）
  useEffect(() => {
    if (rollbackAt === null) return undefined;
    const timer = window.setTimeout(() => setRollbackAt(null), 2500);
    return () => clearTimeout(timer);
  }, [rollbackAt]);
  const applyDecision = useCallback(
    (decision: CullDecision, options?: { advance?: boolean }) => {
      if (currentId === null) return;
      const prev = decisions.get(currentId) ?? null;
      setDecisions(withDecision(decisions, currentId, decision));
      void cullDecisionApply(session.id, [{ assetId: currentId, decision }]).then((dto) => {
        if (dto === null || dto.id !== session.id) {
          // 传输失败/脏数据：回滚本条决定（进度以本地态继续派生）
          setDecisions((cur) => withDecision(cur, currentId, prev));
          setRollbackAt({ assetId: currentId, decision: prev });
          return;
        }
        setSession(dto); // 计数对齐后端真值
      });
      if (options?.advance) setIndex((cur) => indexAfterDecision(cur, ids.length));
    },
    [currentId, decisions, ids.length, session.id],
  );

  const navigate = useCallback(
    (next: number) => {
      if (items === null || items.length === 0) return;
      setIndex(Math.min(Math.max(0, next), items.length - 1));
    },
    [items],
  );

  // --- 相邻预取（翻片瞬时显示） --------------------------------------------------------
  useEffect(() => {
    if (ids.length === 0) return;
    const neighbors = [ids[index - 1], ids[index + 1]];
    for (const id of neighbors) {
      if (id === undefined) continue;
      const meta = assetById.get(id);
      if (meta?.kind === "photo") warmImageDecode(safeConvert(meta.path));
      void fetchAssetThumb(id, VIEWER_THUMB_SIZE, "high").then((r) => {
        if (r.kind === "url") warmImageDecode(r.url);
      });
    }
  }, [ids, index, assetById]);

  // --- AI 行内提示（闭眼/失焦 chips；asset_detail 已有字段） ---------------------------
  const [aiDetail, setAiDetail] = useState<AssetDetailDto | null>(null);
  useEffect(() => {
    let cancelled = false;
    setAiDetail(null);
    if (currentId === null) return undefined;
    void assetDetail(currentId).then((detail) => {
      if (!cancelled) setAiDetail(detail);
    });
    return () => {
      cancelled = true;
    };
  }, [currentId]);
  const aiChips = useMemo(() => {
    const ai = aiDetail?.aiAnalysis;
    if (ai === null || ai === undefined) return [];
    const chips: Array<{ key: string; tone: "red" | "amber" }> = [];
    if (ai.eyes?.value === "closed") chips.push({ key: "eyesClosed", tone: "red" });
    else if (ai.eyes?.value === "maybe") chips.push({ key: "eyesMaybe", tone: "amber" });
    if (ai.blur?.value === "soft") chips.push({ key: "blurSoft", tone: "amber" });
    return chips;
  }, [aiDetail]);

  // --- 收尾弹窗 ------------------------------------------------------------------------
  const [finishOpen, setFinishOpen] = useState(false);
  const currentDecision = currentId !== null ? decisions.get(currentId) ?? null : null;

  // --- 键盘接管（window 捕获 + stopPropagation；收尾弹窗打开时让位） -------------------
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (finishOpen) return; // 弹窗自处理 Esc（其自身捕获监听）
      const target = e.target;
      if (
        target instanceof HTMLElement &&
        (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable)
      ) {
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        setZoomed(false);
        onClose();
        return;
      }
      if (e.key === "z" || e.key === "Z") {
        if (e.ctrlKey || e.metaKey || e.altKey) return;
        e.preventDefault();
        e.stopPropagation();
        setZoomed(true);
        return;
      }
      const action = cullKeyAction(e);
      if (action === null) return;
      e.preventDefault();
      e.stopPropagation();
      if (items === null || items.length === 0) return;
      switch (action) {
        case "prev":
          navigate(index - 1);
          break;
        case "next":
          navigate(index + 1);
          break;
        case "accept":
          applyDecision("accepted", { advance: true });
          break;
        case "reject":
          applyDecision("rejected", { advance: true });
          break;
        case "undecided":
          applyDecision(null);
          break;
      }
    };
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.key === "z" || e.key === "Z") setZoomed(false);
    };
    window.addEventListener("keydown", onKey, { capture: true });
    window.addEventListener("keyup", onKeyUp, { capture: true });
    return () => {
      window.removeEventListener("keydown", onKey, { capture: true });
      window.removeEventListener("keyup", onKeyUp, { capture: true });
    };
  }, [applyDecision, finishOpen, index, items, navigate, onClose]);

  const progressPct = progress.total > 0 ? ((progress.accepted + progress.rejected) / progress.total) * 100 : 0;

  return (
    <div
      className="fixed inset-0 z-[60] flex flex-col bg-black/95"
      role="dialog"
      aria-modal="true"
      aria-label={session.name}
      data-testid="culling-overlay"
    >
      {/* 顶栏：拖拽层 + 会话名 + 进度计数 + 完成选片 + 退出 */}
      <div className="relative flex h-12 shrink-0 items-center gap-3 px-4 text-text-primary">
        <div className="absolute inset-0" data-tauri-drag-region />
        <div className="pointer-events-none relative flex min-w-0 items-baseline gap-3">
          <span className="truncate text-sm font-semibold" title={session.name} data-testid="culling-name">
            {session.name}
          </span>
          <span
            className="shrink-0 font-mono text-xs tabular-nums text-text-secondary"
            data-testid="culling-progress"
            data-accepted={progress.accepted}
            data-rejected={progress.rejected}
            data-undecided={progress.undecided}
            data-total={progress.total}
          >
            {t("culling.overlay.progress", {
              accepted: progress.accepted,
              rejected: progress.rejected,
              undecided: progress.undecided,
              total: progress.total,
            })}
          </span>
        </div>
        {rollbackAt !== null && (
          <span
            className="pointer-events-none relative rounded bg-red-500/15 px-1.5 py-0.5 text-[10px] text-red-300"
            role="alert"
            data-testid="culling-rollback"
          >
            {t("culling.overlay.rollback")}
          </span>
        )}
        <div className="pointer-events-none relative ml-auto flex items-center gap-2">
          <button
            type="button"
            onClick={() => setFinishOpen(true)}
            disabled={items === null || items.length === 0}
            className="pointer-events-auto rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:opacity-40"
            data-testid="culling-finish"
          >
            {t("culling.overlay.finish")}
          </button>
          <button
            type="button"
            onClick={onClose}
            aria-label={t("culling.overlay.exit")}
            title={t("culling.overlay.exit")}
            className="pointer-events-auto rounded-md border border-edge px-2.5 py-1 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
            data-testid="culling-exit"
          >
            {t("culling.overlay.exit")}
          </button>
        </div>
      </div>

      {/* 进度条（已决定占比；已选/已剔除分色） */}
      <div className="flex h-1.5 shrink-0 gap-px bg-panel/60" data-testid="culling-progressbar">
        <div
          className="h-full bg-emerald-500 transition-[width] duration-150"
          style={{ width: `${progress.total > 0 ? (progress.accepted / progress.total) * 100 : 0}%` }}
        />
        <div
          className="h-full bg-red-500 transition-[width] duration-150"
          style={{ width: `${progress.total > 0 ? (progress.rejected / progress.total) * 100 : 0}%` }}
        />
        <span className="sr-only">{Math.round(progressPct)}%</span>
      </div>

      {/* 主区：单图大图 */}
      <div className="relative min-h-0 flex-1" data-testid="culling-stage-pane">
        <div
          ref={stageRef}
          className="absolute inset-0 flex touch-none items-center justify-center overflow-hidden"
          onMouseMove={handleStageMouseMove}
          onDoubleClick={() => setZoomed(false)}
          style={{ cursor: zoomed ? "zoom-in" : "default" }}
          data-testid="culling-stage"
          data-zoomed={zoomed}
        >
          {openFailed ? (
            <div className="flex flex-col items-center gap-3 text-text-muted" data-testid="culling-open-failed">
              <p className="text-sm text-text-secondary">{t("culling.overlay.openFailed")}</p>
              <button
                type="button"
                onClick={onClose}
                className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
              >
                {t("culling.overlay.exit")}
              </button>
            </div>
          ) : items !== null && items.length === 0 ? (
            <div className="flex flex-col items-center gap-3 text-text-muted" data-testid="culling-empty">
              <p className="text-sm text-text-secondary">{t("culling.overlay.empty")}</p>
              <button
                type="button"
                onClick={onClose}
                className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
              >
                {t("culling.overlay.exit")}
              </button>
            </div>
          ) : (
            <>
              {imageLayers.map((layer) => (
                <img
                  key={layer.src}
                  src={layer.src}
                  alt={layer.phase === "active" ? asset?.name ?? "" : ""}
                  aria-hidden={layer.phase === "active" ? undefined : "true"}
                  draggable={false}
                  loading="eager"
                  decoding="async"
                  onLoad={
                    layer.phase === "loading"
                      ? (event) => {
                          const image = event.currentTarget;
                          void (async () => {
                            try {
                              if (typeof image.decode === "function") await image.decode();
                            } catch {
                              // 部分编码器可显示但拒绝 decode；继续交下一帧
                            }
                            await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
                            setImageLayers((previous) => {
                              const target = previous.find((item) => item.src === layer.src);
                              if (!target) return previous;
                              return previous.map((item) =>
                                item.src === layer.src
                                  ? { ...item, phase: "active" }
                                  : { ...item, phase: "retiring" },
                              );
                            });
                          })();
                        }
                      : undefined
                  }
                  onError={
                    layer.phase === "loading"
                      ? () => {
                          // 分级降档：原图失败 → 2048 → 512
                          if (asset?.kind === "photo" && photoStage === "original") setPhotoStage("mid");
                          else if (asset?.kind === "photo" && photoStage === "mid") setPhotoStage("thumb");
                        }
                      : undefined
                  }
                  data-testid={layer.phase === "active" ? "culling-img-active" : "culling-img-loading"}
                  className={`${layer.phase === "loading" ? "invisible " : "absolute "}max-h-full max-w-full select-none object-contain`}
                  style={{
                    transform: `scale(${zoomed ? ZOOM_SCALE : 1})`,
                    transformOrigin: `${zoomAt.x * 100}% ${zoomAt.y * 100}%`,
                    transition: "transform 120ms ease-out",
                  }}
                />
              ))}
              {imageLayers.length === 0 && items !== null && (
                <div
                  className="h-10 w-10 animate-spin rounded-full border-2 border-edge border-t-accent"
                  data-testid="culling-loading"
                />
              )}
              {imageLayers.length === 0 && mainFailed && (
                <div className="flex flex-col items-center gap-2 text-text-muted" data-testid="culling-no-preview">
                  <span className="text-xs">{t("culling.overlay.noPreview")}</span>
                </div>
              )}

              {/* AI 行内提示 chips（闭眼/失焦；AI 只建议） */}
              {aiChips.length > 0 && (
                <div
                  className="pointer-events-none absolute bottom-3 left-3 flex flex-wrap gap-1.5"
                  data-testid="culling-ai-chips"
                >
                  {aiChips.map((chip) => (
                    <span
                      key={chip.key}
                      className={`flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-medium shadow ${
                        chip.tone === "red" ? "bg-red-500/85 text-white" : "bg-amber-400/90 text-black"
                      }`}
                      data-testid="culling-ai-chip"
                      data-kind={chip.key}
                    >
                      {t(`culling.overlay.ai.${chip.key}`)}
                      <span className="rounded bg-black/25 px-1 text-[8px] font-semibold leading-3">
                        {t("culling.overlay.ai.badge")}
                      </span>
                    </span>
                  ))}
                </div>
              )}

              {/* 当前决定态角标 */}
              {currentDecision !== null && (
                <div
                  className={`pointer-events-none absolute right-3 top-3 rounded-full px-2.5 py-1 text-xs font-semibold shadow ${
                    currentDecision === "accepted"
                      ? "bg-emerald-500 text-black"
                      : "bg-red-500 text-white"
                  }`}
                  data-testid="culling-decision-badge"
                  data-decision={currentDecision}
                >
                  {currentDecision === "accepted"
                    ? t("culling.overlay.decisionAccepted")
                    : t("culling.overlay.decisionRejected")}
                </div>
              )}

              {/* 左右翻片 */}
              <button
                type="button"
                onClick={() => navigate(index - 1)}
                disabled={index <= 0}
                aria-label={t("culling.overlay.prev")}
                className="pointer-events-auto absolute left-3 top-1/2 z-10 flex h-11 w-11 -translate-y-1/2 items-center justify-center rounded-full border border-edge bg-black/50 text-text-primary transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-30"
                data-testid="culling-prev"
              >
                <svg viewBox="0 0 16 16" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                  <path d="M10 3L5 8l5 5" />
                </svg>
              </button>
              <button
                type="button"
                onClick={() => navigate(index + 1)}
                disabled={items === null || index >= items.length - 1}
                aria-label={t("culling.overlay.next")}
                className="pointer-events-auto absolute right-3 top-1/2 z-10 flex h-11 w-11 -translate-y-1/2 items-center justify-center rounded-full border border-edge bg-black/50 text-text-primary transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-30"
                data-testid="culling-next"
              >
                <svg viewBox="0 0 16 16" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                  <path d="M6 3l5 5-5 5" />
                </svg>
              </button>

              {/* 键位提示（常驻弱样式） */}
              <p
                className="pointer-events-none absolute bottom-3 left-1/2 -translate-x-1/2 rounded bg-black/40 px-2 py-0.5 font-mono text-[10px] text-text-muted/80"
                data-testid="culling-keys-hint"
              >
                {t("culling.overlay.keysHint")}
              </p>
            </>
          )}
        </div>
      </div>

      {/* 底部：决定操作条（键盘等价物） */}
      <div
        className="flex h-12 shrink-0 items-center justify-center gap-2 border-t border-edge bg-surface/80 px-4"
        data-testid="culling-actions"
      >
        <span className="mr-2 font-mono text-[11px] tabular-nums text-text-muted" data-testid="culling-index">
          {items !== null && items.length > 0
            ? t("culling.overlay.index", { index: index + 1, total: items.length })
            : "—"}
        </span>
        <button
          type="button"
          onClick={() => applyDecision("accepted", { advance: true })}
          className="rounded-md bg-emerald-500 px-3 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110"
          data-testid="culling-accept"
        >
          {t("culling.overlay.accept")} <span className="font-mono opacity-60">空格</span>
        </button>
        <button
          type="button"
          onClick={() => applyDecision(null)}
          className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
          data-testid="culling-undecided"
        >
          {t("culling.overlay.undecided")} <span className="font-mono opacity-60">U</span>
        </button>
        <button
          type="button"
          onClick={() => applyDecision("rejected", { advance: true })}
          className="rounded-md bg-red-500 px-3 py-1.5 text-xs font-medium text-white transition-colors hover:brightness-110"
          data-testid="culling-reject"
        >
          {t("culling.overlay.reject")} <span className="font-mono opacity-60">X</span>
        </button>
      </div>

      {/* 底部胶片条（虚拟滚动 + 决定角标 + 点击跳转） */}
      {items !== null && items.length > 0 && (
        <CullFilmstrip
          ids={ids}
          decisions={decisions}
          origins={origins}
          assetMeta={assetMeta}
          index={index}
          onJump={navigate}
        />
      )}

      {/* 收尾映射弹窗（打开时本层键盘让位） */}
      {finishOpen && (
        <CullFinishDialog
          session={session}
          progress={progress}
          onClose={() => setFinishOpen(false)}
          onApplied={(result) => onFinished(result, session.name)}
          onFailed={onFinishFailed}
        />
      )}
    </div>
  );
}
