import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { convertFileSrc } from "@tauri-apps/api/core";

import { openWithSystem } from "@/ipc/api";

import { supportsHevc } from "../lib/hevc";

/**
 * 查看器视频舞台（M8）：内建 <video> 播放（WebView2 用系统 MediaFoundation
 * 解码），失败回退系统播放器。
 * - src 走 asset 协议直读原文件；poster 用后端 ffmpeg 海报（未就绪时 preload
 *   metadata 由 WebView 自渲首帧）
 * - onError → 回退卡：系统缺 HEVC 扩展时提示安装，附「用系统播放器打开」
 *   （open_with_system / ShellExecute 默认程序）
 * - 指针事件阻断冒泡：视频控制条不触发舞台的拖拽/双击缩放
 */
export default function ViewerVideo({
  path,
  posterUrl,
}: {
  path: string;
  posterUrl: string | null;
}) {
  const { t } = useTranslation();
  const [failed, setFailed] = useState(false);
  // 资产切换复位失败态（同文件重试仍可再失败）
  useEffect(() => {
    setFailed(false);
  }, [path]);
  const src = useMemo(() => {
    try {
      return convertFileSrc(path) || null;
    } catch {
      return null;
    }
  }, [path]);

  if (src === null || failed) {
    const hevcMissing = !supportsHevc();
    return (
      <div
        className="flex max-h-full max-w-full flex-col items-center gap-3 rounded-lg border border-edge/60 bg-black/40 px-8 py-6 text-center"
        data-testid="viewer-video-fallback"
      >
        <svg
          viewBox="0 0 24 24"
          width="40"
          height="40"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.2"
          strokeLinecap="round"
          strokeLinejoin="round"
          className="text-violet-400"
          aria-hidden="true"
        >
          <rect x="3" y="5" width="18" height="14" rx="2" />
          <path d="M10 9.5l5 2.5-5 2.5v-5z" />
        </svg>
        <p className="max-w-xs text-xs leading-relaxed text-text-secondary">
          {hevcMissing ? t("viewer.videoHevcMissing") : t("viewer.videoFailed")}
        </p>
        <button
          type="button"
          onClick={() => void openWithSystem(path)}
          onPointerDown={(event) => event.stopPropagation()}
          className="rounded-md border border-edge bg-panel px-3 py-1.5 text-xs text-text-primary transition-colors hover:border-accent hover:text-accent"
          data-testid="viewer-video-system"
        >
          {t("viewer.openSystem")}
        </button>
      </div>
    );
  }

  return (
    <video
      controls
      preload="metadata"
      src={src}
      poster={posterUrl ?? undefined}
      onError={() => setFailed(true)}
      onPointerDown={(event) => event.stopPropagation()}
      onDoubleClick={(event) => event.stopPropagation()}
      data-testid="viewer-video"
      className="max-h-full max-w-full object-contain"
    />
  );
}
