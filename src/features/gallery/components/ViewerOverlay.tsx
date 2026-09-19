import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion } from "motion/react";
import { convertFileSrc } from "@tauri-apps/api/core";

import { assetDetail, type AssetDetailDto, type AssetDto } from "@/ipc/api";
import type { AssetGroup } from "../lib/assetGroups";
import { prefetchAssetThumb, useAssetThumbUrl } from "../lib/thumbPipeline";
import AssetThumb from "./AssetThumb";
import { formatBytes } from "@/lib/format";

/**
 * 全屏沉浸查看器（M3）：
 * - 大图策略按 kind：photo 优先原图 asset 协议（convertFileSrc(path)），加载失败回退
 *   大档缩略图（名义 1280，后端 snap 512）；RAW 无可载原图（inline-JPEG 提取在 M4），
 *   直接用大档缩略图放大显示；视频恒占位（播放是后续里程碑）。
 * - 交互：wheel 以指针为锚缩放 1x-4x（原生非 passive 监听），scale>1 可拖拽平移，
 *   90° 步进旋转（按钮 / 键盘 . , R，transform 顺序 rotate→scale→translate，
 *   150ms 过渡），双击复位（含旋转与平移）；←/→ 同组切换（首尾禁用）；
 *   Esc 返回画廊（画廊页不卸载，滚动位置保留）。旋转随资产切换重置，不持久化。
 * - 右侧 EXIF 面板可收起（assetDetail 全元数据 + 库内重复数）；底部胶片条为当前组
 *   缩略图（240 档与网格共享缓存），当前项 accent 描边，点击跳转；相邻 1 张预取。
 */

const VIEWER_THUMB_SIZE = 1280; // 名义边长；后端 snap 512 档就近
const FILM_THUMB_SIZE = 240; // 与网格同档，共享会话缓存
const MIN_SCALE = 1;
const MAX_SCALE = 4;

/** 路径 → asset 协议 URL（非 Tauri 环境抛错回退 null） */
function safeConvert(path: string): string | null {
  try {
    return convertFileSrc(path) || null;
  } catch {
    return null;
  }
}

/** ISO 时间 → "YYYY-MM-DD HH:mm:ss"（本地时区）；空值 "—" */
function isoLabel(iso: string | null): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const pad = (n: number) => String(n).padStart(2, "0");
  return (
    `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ` +
    `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
  );
}

/** 文本元数据容错：null/undefined/空串 → "—"（绝不渲染 "undefined"） */
function formatValue(value: string | null | undefined): string {
  return value === null || value === undefined || value === "" ? "—" : value;
}

interface ViewerOverlayProps {
  asset: AssetDto;
  group: AssetGroup;
  index: number;
  /** 同组内切换（胶片条/箭头/键盘） */
  onNavigate: (index: number) => void;
  onClose: () => void;
}

export default function ViewerOverlay({ asset, group, index, onNavigate, onClose }: ViewerOverlayProps) {
  const { t } = useTranslation();
  const stageRef = useRef<HTMLDivElement | null>(null);

  // --- 缩放/平移/旋转状态（资产切换时复位；旋转不持久化） ----------------------------
  const [view, setView] = useState({ scale: 1, x: 0, y: 0, rotation: 0 });
  useEffect(() => {
    setView({ scale: 1, x: 0, y: 0, rotation: 0 });
  }, [asset.id]);

  const clampScale = (s: number) => Math.min(MAX_SCALE, Math.max(MIN_SCALE, s));
  /** 90° 步进旋转（负=逆时针）；触发拖拽/缩放之外的独立维度 */
  const rotate = (delta: number) =>
    setView((v) => ({ ...v, rotation: (((v.rotation + delta) % 360) + 360) % 360 }));

  // wheel 缩放：原生非 passive 监听（React 合成 wheel 为 passive，无法 preventDefault）
  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const rect = el.getBoundingClientRect();
      const cx = e.clientX - rect.left - rect.width / 2;
      const cy = e.clientY - rect.top - rect.height / 2;
      setView((v) => {
        const next = clampScale(v.scale * (e.deltaY < 0 ? 1.2 : 1 / 1.2));
        if (next === v.scale) return v;
        if (next === MIN_SCALE) return { scale: MIN_SCALE, x: 0, y: 0, rotation: v.rotation };
        const ratio = next / v.scale;
        // 指针为锚：保持光标下的图像点不动
        return { scale: next, x: cx - (cx - v.x) * ratio, y: cy - (cy - v.y) * ratio, rotation: v.rotation };
      });
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  // 拖拽平移（scale>1 时）：pointer capture，jsdom/老 WebView 缺失时静默退化
  const dragRef = useRef<{ x: number; y: number } | null>(null);
  function handlePointerDown(e: React.PointerEvent<HTMLDivElement>): void {
    if (view.scale <= MIN_SCALE) return;
    dragRef.current = { x: e.clientX, y: e.clientY };
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      // 无指针捕获时 move/up 仍在本元素内生效
    }
  }
  function handlePointerMove(e: React.PointerEvent<HTMLDivElement>): void {
    const drag = dragRef.current;
    if (!drag) return;
    setView((v) => ({ ...v, x: v.x + (e.clientX - drag.x), y: v.y + (e.clientY - drag.y) }));
    dragRef.current = { x: e.clientX, y: e.clientY };
  }
  function handlePointerUp(e: React.PointerEvent<HTMLDivElement>): void {
    dragRef.current = null;
    try {
      e.currentTarget.releasePointerCapture(e.pointerId);
    } catch {
      // 静默
    }
  }

  // 键盘：Esc 返回画廊；←/→ 同组切换；. , R 旋转（. / R=顺时针，,=逆时针）
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      } else if (e.key === "ArrowLeft" && index > 0) {
        onNavigate(index - 1);
      } else if (e.key === "ArrowRight" && index < group.assets.length - 1) {
        onNavigate(index + 1);
      } else if (e.key === "." || e.key === "r" || e.key === "R") {
        rotate(90);
      } else if (e.key === ",") {
        rotate(-90);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [index, group.assets.length, onNavigate, onClose]);

  // --- 大图来源（按 kind） -----------------------------------------------------------
  // photo：原图优先，onError 回退缩略图；raw：直接大档缩略图；video：恒占位
  const originalUrl = useMemo(
    () => (asset.kind === "photo" ? safeConvert(asset.path) : null),
    [asset.kind, asset.path],
  );
  const [originalFailed, setOriginalFailed] = useState(false);
  useEffect(() => {
    setOriginalFailed(false);
  }, [asset.id, originalUrl]);
  const wantsThumb = asset.kind === "photo" || asset.kind === "raw";
  const thumb = useAssetThumbUrl(asset.id, VIEWER_THUMB_SIZE, wantsThumb);
  const mainSrc = originalUrl !== null && !originalFailed ? originalUrl : thumb.url;
  // 两种「确定无图」：video 恒占位；photo 原图失败后缩略图也结算为 null
  const mainFailed =
    asset.kind === "video" || (mainSrc === null && (originalFailed || (asset.kind === "raw" && thumb.settled)));

  // 相邻预取（胶片条+大图回退）：切到任一张时预热前后各 1
  useEffect(() => {
    if (index > 0) prefetchAssetThumb(group.assets[index - 1].id, FILM_THUMB_SIZE);
    if (index < group.assets.length - 1) prefetchAssetThumb(group.assets[index + 1].id, FILM_THUMB_SIZE);
  }, [index, group.assets]);

  // --- EXIF 面板 ---------------------------------------------------------------------
  const [exifOpen, setExifOpen] = useState(true);
  const [detail, setDetail] = useState<AssetDetailDto | null>(null);
  const [detailLoading, setDetailLoading] = useState(true);
  useEffect(() => {
    let cancelled = false;
    setDetailLoading(true);
    setDetail(null);
    void assetDetail(asset.id).then((d) => {
      if (cancelled) return;
      setDetail(d);
      setDetailLoading(false);
    });
    return () => {
      cancelled = true;
    };
  }, [asset.id]);

  const exifRows: Array<[string, React.ReactNode]> = useMemo(() => {
    if (!detail) return [];
    // 核心行：缺失显示「—」（后端字段可能为 null/undefined，均按无值处理）
    const rows: Array<[string, React.ReactNode]> = [
      [t("viewer.camera"), formatValue(detail.camera)],
      [t("viewer.lens"), formatValue(detail.lens)],
      [t("viewer.capturedAt"), isoLabel(detail.capturedAt)],
      [t("viewer.size"), formatBytes(detail.size)],
    ];
    // EXIF 扩展行（后端契约扩展中）：无值整行隐藏（比一排「—」干净）
    if (detail.width != null && detail.height != null) {
      rows.push([t("viewer.dimensions"), `${detail.width} × ${detail.height}`]);
    }
    if (detail.iso != null) rows.push([t("viewer.iso"), String(detail.iso)]);
    if (detail.aperture != null) rows.push([t("viewer.aperture"), `f/${detail.aperture}`]);
    if (detail.shutter != null) rows.push([t("viewer.shutter"), detail.shutter]);
    if (detail.focalLength != null) {
      rows.push([t("viewer.focalLength"), `${detail.focalLength} mm`]);
    }
    rows.push([t("viewer.path"), <span key="path" className="break-all font-mono text-[11px]">{detail.path}</span>]);
    rows.push([t("viewer.importedAt"), isoLabel(detail.createdAt)]);
    rows.push([
      t("viewer.dupCount"),
      <span key="dup" className={detail.dupCount > 0 ? "font-medium text-accent" : undefined}>
        {t("viewer.dupItems", { count: detail.dupCount })}
      </span>,
    ]);
    return rows;
  }, [detail, t]);

  const hasPrev = index > 0;
  const hasNext = index < group.assets.length - 1;

  return (
    <div
      className="fixed inset-0 z-50 flex flex-col bg-black/95"
      role="dialog"
      aria-modal="true"
      aria-label={asset.name}
      data-testid="viewer"
    >
      {/* 顶栏：文件名 + 计数 + EXIF 开关 + 关闭 */}
      <div className="flex h-12 shrink-0 items-center gap-3 px-4 text-text-primary">
        <span className="truncate text-sm font-medium" title={asset.name} data-testid="viewer-name">
          {asset.name}
        </span>
        <span className="shrink-0 font-mono text-xs text-text-muted" data-testid="viewer-index">
          {t("viewer.index", { index: index + 1, total: group.assets.length })}
        </span>
        <div className="ml-auto flex items-center gap-2">
          {/* 旋转：90° 步进（逆/顺时针），150ms 过渡；随资产切换重置 */}
          <button
            type="button"
            onClick={() => rotate(-90)}
            aria-label={t("viewer.rotateCcw")}
            title={t("viewer.rotateCcw")}
            className="rounded-md border border-edge px-2.5 py-1 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
            data-testid="viewer-rotate-ccw"
          >
            <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M3.5 6.5a5 5 0 1 1 1.2 5.4" />
              <path d="M3.2 3.2v3.3h3.3" />
            </svg>
          </button>
          <button
            type="button"
            onClick={() => rotate(90)}
            aria-label={t("viewer.rotateCw")}
            title={t("viewer.rotateCw")}
            className="rounded-md border border-edge px-2.5 py-1 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
            data-testid="viewer-rotate-cw"
          >
            <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M12.5 6.5a5 5 0 1 0-1.2 5.4" />
              <path d="M12.8 3.2v3.3H9.5" />
            </svg>
          </button>
          <button
            type="button"
            onClick={() => setExifOpen((v) => !v)}
            aria-pressed={exifOpen}
            className="rounded-md border border-edge px-2.5 py-1 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
            data-testid="viewer-exif-toggle"
          >
            {t("viewer.exif")}
          </button>
          <button
            type="button"
            onClick={onClose}
            aria-label={t("viewer.close")}
            className="rounded-md border border-edge px-2.5 py-1 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
            data-testid="viewer-close"
          >
            {t("viewer.close")}
          </button>
        </div>
      </div>

      <div className="flex min-h-0 flex-1">
        {/* 主区：大图 + 左右切换 */}
        <div className="relative min-w-0 flex-1">
          <div
            ref={stageRef}
            className="absolute inset-0 flex touch-none items-center justify-center overflow-hidden"
            onDoubleClick={() => setView({ scale: 1, x: 0, y: 0, rotation: 0 })}
            onPointerDown={handlePointerDown}
            onPointerMove={handlePointerMove}
            onPointerUp={handlePointerUp}
            onPointerCancel={handlePointerUp}
            data-testid="viewer-stage"
            data-scale={view.scale.toFixed(2)}
            data-rotation={view.rotation}
            style={{ cursor: view.scale > 1 ? "grab" : "default" }}
          >
            {mainSrc !== null ? (
              <img
                src={mainSrc}
                alt={asset.name}
                draggable={false}
                onError={() => {
                  // photo 原图失败（RAW 伪装/文件被移动等）→ 回退缩略图
                  if (originalUrl !== null && !originalFailed) setOriginalFailed(true);
                }}
                data-testid="viewer-img"
                data-fallback={mainSrc === originalUrl ? "original" : "thumb"}
                className="max-h-full max-w-full select-none object-contain"
                style={{
                  // 变换顺序 rotate→scale→translate（先旋转后缩放）
                  transform: `rotate(${view.rotation}deg) scale(${view.scale}) translate(${view.x}px, ${view.y}px)`,
                  transition: dragRef.current ? "none" : "transform 150ms ease-out",
                }}
              />
            ) : mainFailed ? (
              <div className="flex flex-col items-center gap-2 text-text-muted" data-testid="viewer-placeholder">
                <svg
                  viewBox="0 0 24 24"
                  width="56"
                  height="56"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.2"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  className={asset.kind === "raw" ? "text-sky-400" : "text-violet-400"}
                  aria-hidden="true"
                >
                  {asset.kind === "video" ? (
                    <>
                      <rect x="3" y="5" width="18" height="14" rx="2" />
                      <path d="M10 9.5l5 2.5-5 2.5v-5z" />
                    </>
                  ) : (
                    <>
                      <rect x="3.5" y="4.5" width="17" height="15" rx="2" />
                      <circle cx="9" cy="10" r="1.8" />
                      <path d="M4.5 17l4.5-4.5 3.5 3.5 3-3 4 4" />
                    </>
                  )}
                </svg>
                <span className="text-xs">{t("viewer.noPreview")}</span>
              </div>
            ) : (
              // 缩略图请求在途：轻占位（无闪跳）
              <div className="h-48 w-48 animate-pulse rounded bg-panel/50" data-testid="viewer-loading" />
            )}

            {/* 左右切换 */}
            <button
              type="button"
              onClick={() => onNavigate(index - 1)}
              disabled={!hasPrev}
              aria-label={t("viewer.prev")}
              className="absolute left-3 top-1/2 flex h-10 w-10 -translate-y-1/2 items-center justify-center rounded-full border border-edge bg-black/50 text-text-primary transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-30"
              data-testid="viewer-prev"
            >
              <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <path d="M10 3L5 8l5 5" />
              </svg>
            </button>
            <button
              type="button"
              onClick={() => onNavigate(index + 1)}
              disabled={!hasNext}
              aria-label={t("viewer.next")}
              className="absolute right-3 top-1/2 flex h-10 w-10 -translate-y-1/2 items-center justify-center rounded-full border border-edge bg-black/50 text-text-primary transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-30"
              data-testid="viewer-next"
            >
              <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <path d="M6 3l5 5-5 5" />
              </svg>
            </button>
          </div>
          <p className="pointer-events-none absolute bottom-1.5 left-1/2 -translate-x-1/2 font-mono text-[10px] text-text-muted/70">
            {t("viewer.zoomHint")}
          </p>
        </div>

        {/* 右侧 EXIF 面板（可收起） */}
        <AnimatePresence initial={false}>
          {exifOpen && (
            <motion.aside
              key="exif"
              initial={{ width: 0, opacity: 0 }}
              animate={{ width: 288, opacity: 1 }}
              exit={{ width: 0, opacity: 0 }}
              transition={{ duration: 0.15, ease: "easeOut" }}
              className="h-full shrink-0 overflow-hidden border-l border-edge bg-surface/95"
              data-testid="viewer-exif"
            >
              <div className="h-full w-72 overflow-y-auto p-3">
                <h2 className="mb-2 text-xs font-semibold text-text-primary">{t("viewer.exif")}</h2>
                {detailLoading ? (
                  <div className="space-y-2" data-testid="viewer-exif-loading">
                    {[0, 1, 2, 3, 4].map((i) => (
                      <div key={i} className="h-4 animate-pulse rounded bg-panel/70" />
                    ))}
                  </div>
                ) : detail === null ? (
                  <p className="text-xs leading-relaxed text-text-muted" data-testid="viewer-exif-unavailable">
                    {t("viewer.exifUnavailable")}
                  </p>
                ) : (
                  <dl className="space-y-1.5" data-testid="viewer-exif-rows">
                    {exifRows.map(([label, value]) => (
                      <div key={label} className="flex items-baseline justify-between gap-2 text-xs">
                        <dt className="shrink-0 text-text-muted">{label}</dt>
                        <dd className="min-w-0 text-right text-text-secondary">{value}</dd>
                      </div>
                    ))}
                  </dl>
                )}
              </div>
            </motion.aside>
          )}
        </AnimatePresence>
      </div>

      {/* 底部胶片条：当前组缩略图 */}
      <div
        className="sp-scroll flex h-[76px] shrink-0 items-center gap-1.5 overflow-x-auto border-t border-edge px-3"
        data-testid="viewer-filmstrip"
      >
        {group.assets.map((item, i) => (
          <button
            key={item.id}
            type="button"
            onClick={() => onNavigate(i)}
            aria-label={item.name}
            aria-current={i === index}
            className={`h-16 w-16 shrink-0 overflow-hidden rounded-md border-2 transition-colors ${
              i === index ? "border-accent" : "border-transparent hover:border-edge"
            }`}
            data-testid="viewer-filmthumb"
            data-asset-id={item.id}
            data-current={i === index}
          >
            <AssetThumb asset={item} size={FILM_THUMB_SIZE} className="h-full w-full" testId="viewer-filmthumb-cell" />
          </button>
        ))}
      </div>
    </div>
  );
}
