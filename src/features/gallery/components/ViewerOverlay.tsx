import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion } from "motion/react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { useVirtualizer } from "@tanstack/react-virtual";

import { assetDetail, type AssetDetailDto, type AssetDto } from "@/ipc/api";
import type { AssetGroup } from "../lib/assetGroups";
import {
  fetchAssetThumb,
  useAssetThumbUrl,
  warmImageDecode,
} from "../lib/thumbPipeline";
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

const VIEWER_MID_SIZE = 2048; // 中间档（后端加 2048 档后生效）：原图渲染失败时的清晰回退
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

  // 操作提示（#6）：首次 3s 后淡出；? 键或 hover 底部提示区重新唤出（再计时 3s）
  const [hintVisible, setHintVisible] = useState(true);
  const hintTimerRef = useRef<number | null>(null);
  const showHint = useCallback(() => {
    setHintVisible(true);
    if (hintTimerRef.current !== null) clearTimeout(hintTimerRef.current);
    hintTimerRef.current = window.setTimeout(() => setHintVisible(false), 3000);
  }, []);
  useEffect(() => {
    showHint();
    return () => {
      if (hintTimerRef.current !== null) clearTimeout(hintTimerRef.current);
    };
  }, [showHint]);

  // 键盘：Esc 返回画廊；←/→ 同组切换；Home/End 跳组首/尾；. , R 旋转；? 唤出操作提示
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      } else if (e.key === "ArrowLeft" && index > 0) {
        onNavigate(index - 1);
      } else if (e.key === "ArrowRight" && index < group.assets.length - 1) {
        onNavigate(index + 1);
      } else if (e.key === "Home") {
        onNavigate(0);
      } else if (e.key === "End") {
        onNavigate(group.assets.length - 1);
      } else if (e.key === "." || e.key === "r" || e.key === "R") {
        rotate(90);
      } else if (e.key === ",") {
        rotate(-90);
      } else if (e.key === "?") {
        showHint();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [index, group.assets.length, onNavigate, onClose, showHint]);

  // --- 大图来源（按 kind，分级回退链） -------------------------------------------------
  // photo：原图 → 中间档缩略图（名义 2048，后端加 2048 档）→ 512 档。WebView2 对
  // 大尺寸/无压缩 TIF 等原生渲染失败，onError 逐级降档，而不是一步到 512 模糊图。
  // raw：直接大档缩略图（内嵌预览的缩放版）；video：恒占位。
  const originalUrl = useMemo(
    () => (asset.kind === "photo" ? safeConvert(asset.path) : null),
    [asset.kind, asset.path],
  );
  const [stage, setStage] = useState<"original" | "mid" | "thumb">("original");
  useEffect(() => {
    setStage("original");
  }, [asset.id, originalUrl]);
  const wantsThumb = asset.kind === "photo" || asset.kind === "raw";
  const thumb = useAssetThumbUrl(asset.id, VIEWER_THUMB_SIZE, wantsThumb, "high");
  // 中间档仅 photo 且原图已失败时才请求（2048 档后端就位后生效；未就位时 settled null → 继续降 512）
  const mid = useAssetThumbUrl(
    asset.id,
    VIEWER_MID_SIZE,
    asset.kind === "photo" && stage !== "original",
    "high",
  );

  let mainSrc: string | null;
  let mainFailed = false;
  if (asset.kind === "video") {
    mainSrc = null;
    mainFailed = true; // 恒占位（播放是后续里程碑）
  } else if (asset.kind === "photo" && stage === "original" && originalUrl !== null) {
    mainSrc = originalUrl; // img onError → 降档；渲染失败前不作无图判定
  } else if (asset.kind === "photo" && stage === "mid") {
    mainSrc = mid.url; // 在途 null → 走加载提示；确定无图由下方自动降档
  } else {
    // photo 降到 512 档 / raw 直达大档缩略图
    mainSrc = thumb.url;
    mainFailed = thumb.settled && thumb.url === null;
  }
  // 中间档确定无图（后端未加 2048 档 / 提取失败）→ 自动降到 512 档
  useEffect(() => {
    if (asset.kind === "photo" && stage === "mid" && mid.settled && mid.url === null) {
      setStage("thumb");
    }
  }, [asset.kind, stage, mid.settled, mid.url]);
  // photo 无原图可用（非 Tauri 环境 convertFileSrc 抛错）→ 直达 512 档
  useEffect(() => {
    if (asset.kind === "photo" && stage === "original" && originalUrl === null) {
      setStage("thumb");
    }
  }, [asset.kind, stage, originalUrl]);

  // --- 双图层交叉淡入（#7 切换闪烁） -------------------------------------------------
  // 切换时保留上一张为底层，新图 onLoad 后 150ms 淡入盖上去再移除旧层——永远有内容无空窗。
  const [committed, setCommitted] = useState<string | null>(null);
  const [incoming, setIncoming] = useState<{ src: string } | null>(null);
  const [incomingReady, setIncomingReady] = useState(false);
  useEffect(() => {
    if (mainSrc === null) return; // 源在途（等 2048/512 URL）——旧图层继续显示
    if (mainSrc === committed || mainSrc === incoming?.src) return;
    setIncoming({ src: mainSrc });
    setIncomingReady(false);
  }, [mainSrc, committed, incoming]);
  // 新图 onLoad → 150ms 淡入完成后提交为新底层
  useEffect(() => {
    if (!incomingReady || incoming === null) return;
    const timer = setTimeout(() => {
      setCommitted(incoming.src);
      setIncoming(null);
      setIncomingReady(false);
    }, 150);
    return () => clearTimeout(timer);
  }, [incomingReady, incoming]);
  // 确定无图：清掉残留图层，显示占位
  useEffect(() => {
    if (mainFailed) {
      setCommitted(null);
      setIncoming(null);
      setIncomingReady(false);
    }
  }, [mainFailed]);

  // 大图加载提示：源在途超过 300ms 才转圈（几十 MB 原图加载慢，避免黑屏误判失败）；
  // 切换期间旧图层兜底显示，仅新图 300ms 仍未 onLoad 才叠加 spinner（快速连按不闪）。
  const [slowLoading, setSlowLoading] = useState(false);
  const awaitingImage =
    (incoming !== null && !incomingReady) || (mainSrc === null && !mainFailed);
  useEffect(() => {
    setSlowLoading(false);
    if (!awaitingImage) return;
    const timer = setTimeout(() => setSlowLoading(true), 300);
    return () => clearTimeout(timer);
  }, [awaitingImage]);

  // 相邻预取（#7 预加载强化）：前后各 1 张——
  // a) 512 回退档 URL 高优先预取；b) new Image() 解码预热（photo 的原图 asset URL 也预热），
  // 让箭头切换时下一张大概率已在解码器缓存里。
  useEffect(() => {
    const neighbors = [
      index > 0 ? group.assets[index - 1] : null,
      index < group.assets.length - 1 ? group.assets[index + 1] : null,
    ];
    for (const neighbor of neighbors) {
      if (!neighbor) continue;
      if (neighbor.kind === "photo") warmImageDecode(safeConvert(neighbor.path));
      void fetchAssetThumb(neighbor.id, VIEWER_THUMB_SIZE, "high").then((url) => {
        warmImageDecode(url);
      });
    }
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

  // 胶片条横向虚拟化：格宽 64 + 间距 6；可视区外不渲染不请求
  const stripRef = useRef<HTMLDivElement | null>(null);
  const stripVirtualizer = useVirtualizer({
    count: group.assets.length,
    getScrollElement: () => stripRef.current,
    estimateSize: () => 70,
    horizontal: true,
    overscan: 8,
  });
  // 当前项滚动入可视区（jsdom 无 scrollIntoView，静默跳过）
  useEffect(() => {
    const el = stripRef.current?.querySelector(`[data-asset-id="${asset.id}"]`);
    if (el && typeof el.scrollIntoView === "function") {
      el.scrollIntoView({ block: "nearest", inline: "nearest" });
    }
  }, [asset.id, stripVirtualizer]);

  return (
    <div
      className="fixed inset-0 z-50 flex flex-col bg-black/95"
      role="dialog"
      aria-modal="true"
      aria-label={asset.name}
      data-testid="viewer"
    >
      {/* 顶栏：文件名（分组一） + 计数（分组二，间隔 16px） + 旋转/EXIF/关闭。
          查看器全屏覆盖了主壳标题栏，顶栏背景层带拖拽区让窗口仍可拖动
          （按钮/文件名 pointer-events 正常，仅空白处落到拖拽层）。 */}
      <div className="relative flex h-12 shrink-0 items-center gap-3 px-4 text-text-primary">
        <div className="absolute inset-0" data-tauri-drag-region />
        <div className="flex min-w-0 items-baseline gap-4">
          <span className="truncate text-sm font-semibold" title={asset.name} data-testid="viewer-name">
            {asset.name}
          </span>
          <span
            className="shrink-0 font-mono text-xs text-text-muted"
            data-testid="viewer-index"
          >
            {t("viewer.index", { index: index + 1, total: group.assets.length })}
          </span>
        </div>
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
            {committed !== null && committed !== incoming?.src && (
              /* 底层：上一张已就绪图——新图 onLoad 前一直保留，切换无空窗 */
              <img
                src={committed}
                alt=""
                aria-hidden="true"
                draggable={false}
                loading="eager"
                decoding="async"
                data-testid="viewer-img-prev"
                className="absolute max-h-full max-w-full select-none object-contain"
                style={{
                  transform: `translate(${view.x}px, ${view.y}px) rotate(${view.rotation}deg) scale(${view.scale})`,
                  transition: dragRef.current ? "none" : "transform 150ms ease-out",
                }}
              />
            )}
            {incoming !== null && (
              /* 上层：切换中的新图，onLoad 后 150ms 淡入盖过底层 */
              <img
                src={incoming.src}
                alt={asset.name}
                draggable={false}
                loading="eager"
                decoding="async"
                onLoad={() => setIncomingReady(true)}
                onError={() => {
                  // 分级降档：原图失败 → 中间档（2048）→ 512 档。
                  // TIF 等大尺寸/特殊编码原图 WebView2 渲染不动，逐级降而不是一步到 512。
                  if (asset.kind === "photo" && stage === "original") setStage("mid");
                  else if (asset.kind === "photo" && stage === "mid") setStage("thumb");
                }}
                data-testid="viewer-img"
                data-fallback={
                  incoming.src === originalUrl ? "original" : incoming.src === mid.url ? "mid" : "thumb"
                }
                className={`max-h-full max-w-full select-none object-contain transition-opacity duration-150 ${
                  incomingReady ? "opacity-100" : "opacity-0"
                }`}
                style={{
                  // 变换顺序 translate→rotate→scale（origin=center）：图片自身中心先随平移
                  // 移动，旋转恒绕图片当前视觉中心（Windows 照片同款，平移后旋转不绕错轴）
                  transform: `translate(${view.x}px, ${view.y}px) rotate(${view.rotation}deg) scale(${view.scale})`,
                  transition: dragRef.current
                    ? "none"
                    : "transform 150ms ease-out, opacity 150ms ease-out",
                }}
              />
            )}
            {incoming === null && committed === null && mainFailed && (
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
            )}
            {slowLoading && (
              // 大图/新图在途超过 300ms：中央小 spinner（避免黑屏被误判为失败）
              <div
                className="absolute h-10 w-10 animate-spin rounded-full border-2 border-edge border-t-accent"
                data-testid="viewer-loading"
              />
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
          {/* 操作提示：CSS 过渡淡入淡出（确定性，不走动画帧） */}
          <p
            onMouseEnter={showHint}
            className={`absolute bottom-1.5 left-1/2 -translate-x-1/2 rounded bg-black/40 px-2 py-0.5 font-mono text-[10px] text-text-muted/80 transition-opacity duration-300 ${
              hintVisible ? "opacity-100" : "pointer-events-none opacity-0"
            }`}
            data-testid="viewer-hint"
          >
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

      {/* 底部胶片条：当前组缩略图。横向虚拟化（只渲染可视区 ±overscan）——
          整组几十格同时入队会挤占缩略图信号量、拖慢主图回退档；滚动按需请求。
          格子未加载期间为静态迷你序号占位（skeleton=false，几十个一起闪很难看）。 */}
      <div
        ref={stripRef}
        className="sp-scroll flex h-[76px] shrink-0 items-center overflow-x-auto border-t border-edge px-3"
        data-testid="viewer-filmstrip"
      >
        <div className="relative h-full" style={{ width: stripVirtualizer.getTotalSize() }}>
          {stripVirtualizer.getVirtualItems().map((vi) => {
            const item = group.assets[vi.index];
            const current = vi.index === index;
            return (
              <button
                key={item.id}
                type="button"
                onClick={() => onNavigate(vi.index)}
                aria-label={item.name}
                aria-current={current}
                className={`absolute top-0 flex h-16 w-16 overflow-hidden rounded-md border-2 transition-colors ${
                  current ? "border-accent" : "border-transparent hover:border-edge"
                }`}
                style={{ transform: `translateX(${vi.start}px)` }}
                data-testid="viewer-filmthumb"
                data-asset-id={item.id}
                data-current={current}
              >
                <AssetThumb
                  asset={item}
                  size={FILM_THUMB_SIZE}
                  className="h-full w-full"
                  skeleton={false}
                  miniLabel={String(vi.index + 1)}
                  testId="viewer-filmthumb-cell"
                />
              </button>
            );
          })}
        </div>
      </div>
    </div>
  );
}
