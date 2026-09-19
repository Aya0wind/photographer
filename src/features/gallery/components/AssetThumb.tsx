import { useEffect, useState } from "react";

import type { AssetKind } from "@/ipc/api";
import { useAssetThumbUrl } from "../lib/thumbPipeline";

/**
 * 库内资产缩略图单元（网格块/查看器胶片条共用）：
 * - photo：进缩略图管线（assetThumbGet→convertFileSrc；未命中占位，thumbnailReady 重试）
 * - raw/video：后端恒无缩略图——永久 kind 占位（不请求不订阅，避免请求风暴）
 * - img onLoad 150ms 淡入；解码失败（缓存文件丢失等）回退占位
 */

/** kind 占位图形：photo=图片框(accent) / raw=扩展名大字+RAW 徽标(sky) / video=胶片(violet) */
function KindPlaceholder({ kind, name }: { kind: AssetKind; name: string }) {
  const dot = name.lastIndexOf(".");
  const ext = dot >= 0 ? name.slice(dot + 1).toUpperCase() : "";
  if (kind === "raw") {
    return (
      <div
        className="flex h-full w-full flex-col items-center justify-center gap-1 bg-panel/60"
        data-testid="thumb-raw"
      >
        <span className="font-mono text-base font-bold tracking-wide text-sky-400">
          {ext || "RAW"}
        </span>
        <span className="rounded bg-bg px-1.5 py-0.5 text-[10px] font-medium text-text-secondary">
          RAW
        </span>
      </div>
    );
  }
  if (kind === "video") {
    return (
      <div
        className="flex h-full w-full flex-col items-center justify-center gap-1.5 bg-panel/60"
        data-testid="thumb-video"
      >
        <svg
          viewBox="0 0 24 24"
          width="28"
          height="28"
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
        {ext && <span className="font-mono text-[10px] text-text-muted">{ext}</span>}
      </div>
    );
  }
  return (
    <div
      className="flex h-full w-full flex-col items-center justify-center gap-1.5 bg-panel/60"
      data-testid="thumb-photo"
    >
      <svg
        viewBox="0 0 24 24"
        width="28"
        height="28"
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
      {ext && <span className="font-mono text-[10px] text-text-muted">{ext}</span>}
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
  testId?: string;
}

export default function AssetThumb({ asset, size, alt, className = "", testId }: AssetThumbProps) {
  // raw/video 恒占位（后端缩略图恒 None）：短路管线，不产生请求与重试订阅
  const { url } = useAssetThumbUrl(asset.id, size, asset.kind === "photo");
  const [loaded, setLoaded] = useState(false);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    setLoaded(false);
    setFailed(false);
  }, [url]);

  const showImg = url !== null && !failed;
  return (
    <div
      className={`relative overflow-hidden bg-panel/40 ${className}`}
      data-testid={testId}
      data-kind={asset.kind}
    >
      {showImg ? (
        <img
          src={url ?? undefined}
          alt={alt ?? asset.name}
          loading="lazy"
          decoding="async"
          onLoad={() => setLoaded(true)}
          onError={() => setFailed(true)}
          data-testid={testId ? `${testId}-img` : undefined}
          className={`h-full w-full object-cover transition-opacity duration-150 ${
            loaded ? "opacity-100" : "opacity-0"
          }`}
        />
      ) : (
        <KindPlaceholder kind={asset.kind} name={asset.name} />
      )}
    </div>
  );
}
