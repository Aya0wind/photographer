import { useCallback, useEffect, useState } from "react";

import type { AssetKind } from "@/ipc/api";
import { useAssetThumbUrl } from "../lib/thumbPipeline";

/**
 * 库内资产缩略图单元（网格块/查看器胶片条共用）：
 * - photo/raw：进缩略图管线（assetThumbGet→convertFileSrc；未命中占位，
 *   thumbnailReady 重试；RAW 走后端内嵌预览提取，可能较慢——占位期间有水印角标）
 * - video：后端恒无缩略图——永久 kind 占位（不请求不订阅）
 * - RAW 恒叠右上角 RAW 水印角标（半透明深底白字；有真实缩略图后仍可一眼区分）
 * - img onLoad 150ms 淡入；解码失败（缓存文件丢失等）回退占位
 */

/** kind 占位图形：photo=图片框(accent) / raw=扩展名徽标(sky) / video=胶片(violet)。
 *  视觉降噪：图形缩小沉到左下角 1/3 区域（大字降一档），其余留白给骨架动画。
 *  容器底色由外层提供（加载中=sp-skeleton 骨架 / 永久无图=静态 panel），此层保持透明。 */
function KindPlaceholder({ kind, name }: { kind: AssetKind; name: string }) {
  const dot = name.lastIndexOf(".");
  const ext = dot >= 0 ? name.slice(dot + 1).toUpperCase() : "";
  if (kind === "raw") {
    return (
      <div
        className="flex h-full w-full flex-col items-start justify-end gap-0.5 p-1.5"
        data-testid="thumb-raw"
      >
        <span className="font-mono text-sm font-bold tracking-wide text-sky-400">
          {ext || "RAW"}
        </span>
        <span className="rounded bg-bg px-1 py-0.5 text-[9px] font-medium leading-none text-text-secondary">
          RAW
        </span>
      </div>
    );
  }
  if (kind === "video") {
    return (
      <div
        className="flex h-full w-full flex-col items-start justify-end gap-0.5 p-1.5"
        data-testid="thumb-video"
      >
        <svg
          viewBox="0 0 24 24"
          width="18"
          height="18"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.4"
          strokeLinecap="round"
          strokeLinejoin="round"
          className="text-violet-400"
          aria-hidden="true"
        >
          <rect x="3" y="5" width="18" height="14" rx="2" />
          <path d="M10 9.5l5 2.5-5 2.5v-5z" />
        </svg>
        {ext && <span className="font-mono text-[9px] text-text-muted">{ext}</span>}
      </div>
    );
  }
  return (
    <div
      className="flex h-full w-full flex-col items-start justify-end gap-0.5 p-1.5"
      data-testid="thumb-photo"
    >
      <svg
        viewBox="0 0 24 24"
        width="18"
        height="18"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
        strokeLinejoin="round"
        className="text-accent"
        aria-hidden="true"
      >
        <rect x="3.5" y="4.5" width="17" height="15" rx="2" />
        <circle cx="9" cy="10" r="1.8" />
        <path d="M4.5 17l4.5-4.5 3.5 3.5 3-3 4 4" />
      </svg>
      {ext && <span className="font-mono text-[9px] text-text-muted">{ext}</span>}
    </div>
  );
}

export interface AssetThumbProps {
  asset: { id: number; kind: AssetKind; name: string };
  /** 名义请求边长（网格 240 → 后端 snap 256 档；胶片条同 240 复用缓存） */
  size: number;
  alt?: string;
  /** 容器额外类（尺寸/圆角由调用方给） */
  className?: string;
  /** 加载中是否用骨架动画（默认开）；胶片条等密集小格传 false——几十个一起闪很难看 */
  skeleton?: boolean;
  /** 无图占位文案（如胶片条序号）；给定时替代 kind 图形（静态迷你占位） */
  miniLabel?: string;
  /** 请求优先级：查看器主图用 high（插队），网格/胶片条默认 low */
  priority?: "high" | "low";
  testId?: string;
}

export default function AssetThumb({
  asset,
  size,
  alt,
  className = "",
  skeleton = true,
  miniLabel,
  priority = "low",
  testId,
}: AssetThumbProps) {
  // RAW 走后端内嵌预览提取（最大段直出），与 photo 同管线；video 恒占位（不请求）
  const { url, status } = useAssetThumbUrl(asset.id, size, asset.kind !== "video", priority);
  const [loaded, setLoaded] = useState(false);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    setLoaded(false);
    setFailed(false);
  }, [url]);

  // WebView2 对内存缓存中的 asset:// 图片偶尔不会再次派发 load；同时原生
  // lazy-loading 与 transform 虚拟列表组合后可能不启动解码。网格本身只
  // 挂载视口+overscan 项，因此直接 eager，并补查缓存图片的 complete 状态。
  const handleImageRef = useCallback((img: HTMLImageElement | null) => {
    if (!img || !img.complete) return;
    if (img.naturalWidth > 0) setLoaded(true);
    else setFailed(true);
  }, []);

  const showImg = url !== null && !failed;
  // 加载中（请求在途/排队生成/缩略图在解码）= 骨架动画；永久无图或已展示 = 静态底。
  // video 不进管线（status 恒 loading）：静态占位，不吃骨架。
  const loading =
    skeleton && asset.kind !== "video" && (status === "loading" || (showImg && !loaded));
  return (
    <div
      className={`relative overflow-hidden ${loading ? "sp-skeleton" : "bg-panel/40"} ${className}`}
      data-testid={testId}
      data-kind={asset.kind}
      data-loading={loading}
    >
      {showImg ? (
        <img
          ref={handleImageRef}
          src={url ?? undefined}
          alt={alt ?? asset.name}
          loading="eager"
          decoding="async"
          onLoad={() => setLoaded(true)}
          onError={() => setFailed(true)}
          data-testid={testId ? `${testId}-img` : undefined}
          className={`h-full w-full object-cover transition-opacity duration-150 ${
            loaded ? "opacity-100" : "opacity-0"
          }`}
        />
      ) : miniLabel !== undefined ? (
        <div className="flex h-full w-full items-center justify-center" data-testid="thumb-mini">
          <span className="font-mono text-[10px] tabular-nums text-text-muted">{miniLabel}</span>
        </div>
      ) : (
        <KindPlaceholder kind={asset.kind} name={asset.name} />
      )}
      {/* RAW 水印角标：真实缩略图就位后仍可一眼区分 RAW/JPG */}
      {asset.kind === "raw" && (
        <span
          className="absolute right-1 top-1 rounded bg-black/60 px-1 py-0.5 font-mono text-[10px] font-bold leading-none text-white"
          data-testid="thumb-raw-badge"
        >
          RAW
        </span>
      )}
    </div>
  );
}
