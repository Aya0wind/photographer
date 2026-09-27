import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

import {
  albumRemoveAssets,
  assetFlagSet,
  assetLabelSet,
  assetRatingSet,
  assetRejectSet,
  revealInExplorer,
  type AssetDto,
} from "@/ipc/api";
import { COLOR_DOT_CLASS, COLOR_DOT_RING, COLOR_LABELS, type ColorLabel } from "../lib/colorLabels";

/**
 * 多选浮动操作条（M4.5，画廊选择模式）：顶部居中浮条——已选 N 张 |
 * 收藏（星标=rating 5）/ 旗标 / 颜色标签（LR 五色）/ 拒绝旗标 / 分享
 * （在资源管理器中显示 = opener reveal、复制文件路径）/ 加入相册（③ 全局
 * 入口；弹窗由上层挂载）/ 反选（当前数据窗口取补集）/ 移入回收站（红色，
 * 上层确认一步）/ 取消。动作对全部选中资产批量调用；失败静默（乐观 UI）。
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

function GlyphTrash() {
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
      <path d="M2.5 4h11M6.5 2h3M4 4l.7 9a1.5 1.5 0 0 0 1.5 1.4h3.6a1.5 1.5 0 0 0 1.5-1.4L12 4" />
    </svg>
  );
}

function GlyphLr() {
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
      <rect x="1.5" y="2.5" width="13" height="11" rx="1.5" />
      <path d="M4.5 5.5v5h3M9 10.5h3.5M9 10.5v-5" />
    </svg>
  );
}

function GlyphInvert() {
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
      <path d="M13.5 8a5.5 5.5 0 1 1-5.5-5.5v11z" fill="currentColor" stroke="none" />
      <circle cx="8" cy="8" r="5.5" />
    </svg>
  );
}

/** 色点（popup 选项/清除行共用） */
function ColorDot({ label }: { label: ColorLabel }) {
  return (
    <span
      className={`h-3.5 w-3.5 rounded-full ${COLOR_DOT_CLASS[label]} ${COLOR_DOT_RING}`}
      aria-hidden="true"
    />
  );
}

export default function SelectionBar({
  count,
  assets,
  onDone,
  onAddToAlbum,
  onFavoritesChanged,
  album,
  onColorLabeled,
  onRejected,
  onTrashRequest,
  onLrStaging,
  windowIds,
  onInvert,
}: {
  count: number;
  assets: AssetDto[];
  onDone: () => void;
  /** 「加入相册」入口回调（弹窗由上层挂载）；不传则不显示该按钮 */
  onAddToAlbum?: (assets: AssetDto[]) => void;
  onFavoritesChanged?: (assets: AssetDto[]) => void;
  /** 相册上下文（相册详情页）：额外显示「从相册移除」 */
  album?: SelectionAlbumContext;
  /** 颜色标签设置完成（IPC 后同步本地列表态）；不传则仅写后端 */
  onColorLabeled?: (assets: AssetDto[], label: string | null) => void;
  /** 拒绝旗标切换完成（IPC 后同步本地列表态） */
  onRejected?: (assets: AssetDto[], rejected: boolean) => void;
  /** 「移入回收站」请求（确认弹窗由上层挂载）；不传则不显示该按钮 */
  onTrashRequest?: (assets: AssetDto[]) => void;
  /** 「生成 LR 暂存夹」请求（命名弹窗由上层挂载）；不传则不显示该按钮 */
  onLrStaging?: (assets: AssetDto[]) => void;
  /** 反选的数据窗口（当前已加载资产 id 全集）；与 onInvert 同给才显示按钮 */
  windowIds?: number[];
  /** 反选完成（上层以补集替换选中集） */
  onInvert?: (ids: number[]) => void;
}) {
  const { t } = useTranslation();
  const [shareOpen, setShareOpen] = useState(false);
  const [colorOpen, setColorOpen] = useState(false);
  const [toast, setToast] = useState<string | null>(null);
  const shareRef = useRef<HTMLDivElement | null>(null);
  const colorRef = useRef<HTMLDivElement | null>(null);

  // 点击浮层菜单外关闭
  useEffect(() => {
    if (!shareOpen && !colorOpen) return;
    const onDown = (e: MouseEvent) => {
      if (shareOpen && shareRef.current && e.target instanceof Node && !shareRef.current.contains(e.target)) {
        setShareOpen(false);
      }
      if (colorOpen && colorRef.current && e.target instanceof Node && !colorRef.current.contains(e.target)) {
        setColorOpen(false);
      }
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [shareOpen, colorOpen]);

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

  /** 颜色标签批量设置（label=null 清除）；再点同色 = 取消该色 */
  async function colorLabel(label: string | null): Promise<void> {
    setColorOpen(false);
    const ids = assets.map((a) => a.id);
    await assetLabelSet(ids, label);
    onColorLabeled?.(assets, label);
    flash(label === null ? t("selection.colorCleared") : t("selection.done"));
  }

  // 拒绝旗标智能切换：全部已拒绝 → 取消拒绝；否则批量拒绝
  const allRejected = assets.length > 0 && assets.every((a) => a.rejected === true);

  async function toggleReject(): Promise<void> {
    const next = !allRejected;
    const ids = assets.map((a) => a.id);
    await assetRejectSet(ids, next);
    onRejected?.(assets, next);
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
        {/* 颜色标签（LR 五色）：弹出五色点 + 清除行，批量作用于选中集 */}
        <div ref={colorRef} className="relative">
          <button
            type="button"
            onClick={() => setColorOpen((v) => !v)}
            disabled={count === 0}
            aria-expanded={colorOpen}
            className="flex items-center gap-1 rounded-full px-2 py-1 text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent disabled:opacity-40"
            data-testid="selection-color"
          >
            <span className={`h-3 w-3 rounded-full ${COLOR_DOT_RING} bg-panel`} aria-hidden="true" />
            {t("selection.colorLabel")}
          </button>
          {colorOpen && (
            <div
              className="absolute left-0 top-8 z-40 flex w-max items-center gap-1.5 rounded-lg border border-edge bg-surface p-2 shadow-xl"
              data-testid="selection-color-menu"
            >
              {COLOR_LABELS.map((label) => (
                <button
                  key={label}
                  type="button"
                  title={t(`gallery.color.${label}`)}
                  aria-label={t(`gallery.color.${label}`)}
                  onClick={() => void colorLabel(label)}
                  className="rounded-full p-1 transition-transform hover:scale-110"
                  data-testid="selection-color-option"
                  data-label={label}
                >
                  <ColorDot label={label} />
                </button>
              ))}
              <span className="h-4 w-px bg-edge" aria-hidden="true" />
              <button
                type="button"
                onClick={() => void colorLabel(null)}
                title={t("selection.colorClear")}
                aria-label={t("selection.colorClear")}
                className="rounded-full p-1 text-text-muted transition-colors hover:bg-panel hover:text-text-primary"
                data-testid="selection-color-clear"
              >
                <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true">
                  <path d="M4 4l8 8M12 4l-8 8" />
                </svg>
              </button>
            </div>
          )}
        </div>
        {/* 拒绝旗标（与星级分层）：全部已拒绝时显示「取消拒绝」 */}
        <button
          type="button"
          onClick={() => void toggleReject()}
          disabled={count === 0}
          aria-pressed={allRejected}
          className={`flex items-center gap-1 rounded-full px-2 py-1 text-[11px] transition-colors disabled:opacity-40 ${
            allRejected
              ? "bg-red-400/15 text-red-400"
              : "text-text-secondary hover:bg-red-400/10 hover:text-red-400"
          }`}
          data-testid="selection-reject"
        >
          <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <circle cx="8" cy="8" r="5.6" />
            <path d="M4.2 11.8l7.6-7.6" />
          </svg>
          {allRejected ? t("selection.unreject") : t("selection.reject")}
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
        {/* 反选：当前数据窗口内取补集（窗口 id 全集由上层传入） */}
        {windowIds !== undefined && onInvert && (
          <button
            type="button"
            onClick={() => {
              const selectedSet = new Set(assets.map((a) => a.id));
              onInvert(windowIds.filter((id) => !selectedSet.has(id)));
              flash(t("selection.done"));
            }}
            disabled={count === 0 && windowIds.length === 0}
            className="flex items-center gap-1 rounded-full px-2 py-1 text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent disabled:opacity-40"
            data-testid="selection-invert"
          >
            <GlyphInvert />
            {t("selection.invert")}
          </button>
        )}
        {onTrashRequest && (
          <button
            type="button"
            onClick={() => onTrashRequest(assets)}
            disabled={count === 0}
            title={t("selection.trashHint")}
            className="flex items-center gap-1 rounded-full px-2 py-1 text-[11px] text-red-400 transition-colors hover:bg-red-400/10 disabled:opacity-40"
            data-testid="selection-trash"
          >
            <GlyphTrash />
            {t("selection.trash")}
          </button>
        )}
        {onLrStaging && (
          <button
            type="button"
            onClick={() => onLrStaging(assets)}
            disabled={count === 0}
            title={t("lr.hint")}
            className="flex items-center gap-1 rounded-full px-2 py-1 text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent disabled:opacity-40"
            data-testid="selection-lr"
          >
            <GlyphLr />
            {t("selection.lr")}
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
