import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

import { albumRemoveAssets, assetFlagSet, assetRatingSet, revealInExplorer, type AssetDto } from "@/ipc/api";

/**
 * 多选浮动操作条（M4.5，画廊选择模式）：顶部居中浮条——已选 N 张 |
 * 收藏（星标=rating 5）/ 旗标 / 分享（在资源管理器中显示 = opener reveal、
 * 复制文件路径）/ 加入相册（③ 全局入口；弹窗由上层挂载）/ 取消。动作对
 * 全部选中资产循环调用；失败静默（乐观 UI）。
 * 相册上下文（相册详情页）：额外多一项「从相册移除」——只删引用，照片保留图库。
 */

/** 相册上下文（相册详情页传入）：操作条多一项「从相册移除」 */
export interface SelectionAlbumContext {
  albumId: number;
  albumName: string;
  /** 移除完成回调（详情页刷新列表与计数） */
  onRemoved: () => void;
}

function GlyphStar({ filled }: { filled: boolean }) {
  return (
    <svg
      viewBox="0 0 16 16"
      width="12"
      height="12"
      fill={filled ? "currentColor" : "none"}
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M8 1.8l1.8 3.7 4 .6-2.9 2.8.7 4L8 11l-3.6 1.9.7-4L2.2 6.1l4-.6z" />
    </svg>
  );
}

function GlyphFlag() {
  return (
    <svg
      viewBox="0 0 16 16"
      width="12"
      height="12"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M3.5 14V2.5M3.5 3h7l-1 2.5 1 2.5h-7" />
    </svg>
  );
}

function GlyphShare() {
  return (
    <svg
      viewBox="0 0 16 16"
      width="12"
      height="12"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M8 10V2.5M5.5 5L8 2.5 10.5 5" />
      <path d="M3 8.5v4a1.5 1.5 0 0 0 1.5 1.5h7a1.5 1.5 0 0 0 1.5-1.5v-4" />
    </svg>
  );
}

function GlyphAlbum() {
  return (
    <svg
      viewBox="0 0 16 16"
      width="12"
      height="12"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <rect x="1.5" y="3" width="13" height="10.5" rx="1.5" />
      <path d="M1.5 6h13M5 1.5h6" />
    </svg>
  );
}

export default function SelectionBar({
  count,
  assets,
  onDone,
  onAddToAlbum,
  onFavoritesChanged,
  album,
}: {
  count: number;
  assets: AssetDto[];
  onDone: () => void;
  /** 「加入相册」入口回调（弹窗由上层挂载）；不传则不显示该按钮 */
  onAddToAlbum?: (assets: AssetDto[]) => void;
  onFavoritesChanged?: (assets: AssetDto[]) => void;
  /** 相册上下文（相册详情页）：额外显示「从相册移除」 */
  album?: SelectionAlbumContext;
}) {
  const { t } = useTranslation();
  const [shareOpen, setShareOpen] = useState(false);
  const [toast, setToast] = useState<string | null>(null);
  const shareRef = useRef<HTMLDivElement | null>(null);

  // 点击分享菜单外关闭
  useEffect(() => {
    if (!shareOpen) return;
    const onDown = (e: MouseEvent) => {
      if (shareRef.current && e.target instanceof Node && !shareRef.current.contains(e.target)) {
        setShareOpen(false);
      }
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [shareOpen]);

  function flash(message: string): void {
    setToast(message);
    window.setTimeout(() => setToast(null), 1500);
  }

  async function favorite(): Promise<void> {
    for (const asset of assets) await assetRatingSet(asset.id, 5);
    onFavoritesChanged?.(assets);
    flash(t("selection.done"));
  }

  async function flag(): Promise<void> {
    for (const asset of assets) await assetFlagSet(asset.id, true);
    flash(t("selection.done"));
  }

  async function reveal(): Promise<void> {
    setShareOpen(false);
    let ok = 0;
    try {
      // 批量单窗定位（同目录多文件=1 窗多选）；失败回退逐个
      ok = await revealInExplorer(assets.map((a) => a.path));
    } catch {
      for (const asset of assets) {
        try {
          await revealItemInDir(asset.path);
          ok += 1;
        } catch {
          // 非 Tauri 环境静默
        }
      }
    }
    flash(ok > 0 ? t("selection.done") : t("selection.revealUnavailable"));
  }

  async function copyPaths(): Promise<void> {
    setShareOpen(false);
    const text = assets.map((a) => a.path).join("\n");
    try {
      await navigator.clipboard.writeText(text);
      flash(t("selection.copied"));
    } catch {
      flash(t("selection.copyFailed"));
    }
  }

  /** 相册上下文：从相册移除引用（只删引用，照片保留图库），完成后上层刷新 */
  async function removeFromAlbum(): Promise<void> {
    if (!album) return;
    const ok = await albumRemoveAssets(
      album.albumId,
      assets.map((a) => a.id),
    );
    if (!ok) {
      flash(t("albums.removeFailed"));
      return;
    }
    flash(t("albums.removedToast", { count: assets.length }));
    album.onRemoved();
  }

  return (
    <div
      className="fixed left-1/2 top-12 z-30 -translate-x-1/2"
      data-testid="selection-bar"
      data-count={count}
    >
      <div className="flex items-center gap-1.5 rounded-full border border-edge bg-surface px-3 py-1.5 shadow-xl">
        <span className="shrink-0 font-mono text-[11px] tabular-nums text-accent" data-testid="selection-count">
          {t("selection.count", { count })}
        </span>
        <span className="h-4 w-px bg-edge" aria-hidden="true" />
        <button
          type="button"
          onClick={() => void favorite()}
          disabled={count === 0}
          className="flex items-center gap-1 rounded-full px-2 py-1 text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent disabled:opacity-40"
          data-testid="selection-favorite"
        >
          <GlyphStar filled />
          {t("selection.favorite")}
        </button>
        <button
          type="button"
          onClick={() => void flag()}
          disabled={count === 0}
          className="flex items-center gap-1 rounded-full px-2 py-1 text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent disabled:opacity-40"
          data-testid="selection-flag"
        >
          <GlyphFlag />
          {t("selection.flag")}
        </button>
        <div ref={shareRef} className="relative">
          <button
            type="button"
            onClick={() => setShareOpen((v) => !v)}
            disabled={count === 0}
            aria-expanded={shareOpen}
            className="flex items-center gap-1 rounded-full px-2 py-1 text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent disabled:opacity-40"
            data-testid="selection-share"
          >
            <GlyphShare />
            {t("selection.share")}
          </button>
          {shareOpen && (
            <div
              className="absolute left-0 top-8 z-40 w-44 overflow-hidden rounded-lg border border-edge bg-surface p-1 shadow-xl"
              data-testid="selection-share-menu"
            >
              <button
                type="button"
                onClick={() => void reveal()}
                className="block w-full rounded px-2 py-1.5 text-left text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent"
                data-testid="selection-share-reveal"
              >
                {t("selection.reveal")}
              </button>
              <button
                type="button"
                onClick={() => void copyPaths()}
                className="block w-full rounded px-2 py-1.5 text-left text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent"
                data-testid="selection-share-copy"
              >
                {t("selection.copyPath")}
              </button>
            </div>
          )}
        </div>
        <span className="h-4 w-px bg-edge" aria-hidden="true" />
        {onAddToAlbum && (
          <button
            type="button"
            onClick={() => onAddToAlbum(assets)}
            disabled={count === 0}
            className="flex items-center gap-1 rounded-full px-2 py-1 text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent disabled:opacity-40"
            data-testid="selection-add-album"
          >
            <GlyphAlbum />
            {t("albums.addToAlbum")}
          </button>
        )}
        {album && (
          <button
            type="button"
            onClick={() => void removeFromAlbum()}
            disabled={count === 0}
            className="flex items-center gap-1 rounded-full px-2 py-1 text-[11px] text-red-400 transition-colors hover:bg-red-400/10 disabled:opacity-40"
            title={t("albums.removeFromAlbumHint")}
            data-testid="selection-remove-album"
          >
            {t("albums.removeFromAlbum")}
          </button>
        )}
        <span className="h-4 w-px bg-edge" aria-hidden="true" />
        <button
          type="button"
          onClick={onDone}
          className="rounded-full px-2 py-1 text-[11px] text-text-muted transition-colors hover:bg-panel hover:text-text-primary"
          data-testid="selection-cancel"
        >
          {t("common.cancel")}
        </button>
      </div>
      {toast !== null && (
        <p
          className="mt-1.5 text-center text-[11px] text-text-muted"
          data-testid="selection-toast"
          role="status"
        >
          {toast}
        </p>
      )}
    </div>
  );
}
