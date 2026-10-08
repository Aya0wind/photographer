import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { motion } from "motion/react";

import { motionInitial, TRANS, useMotionOn } from "@/lib/motion";
import { useTranslation } from "react-i18next";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

import {
  assetFlagSet,
  assetLabelSet,
  assetRatingSet,
  assetRejectSet,
  clipboardCopyFiles,
  revealInExplorer,
  type AssetDto,
} from "@/ipc/api";
import { COLOR_LABELS, COLOR_DOT_CLASS, COLOR_DOT_RING, type ColorLabel } from "../lib/colorLabels";

/**
 * 自定义右键菜单（全局 contextmenu 已被 nativeBehaviorGuard 屏蔽）：
 * - 光标定位 + 视口钳制（测量后向内收回，paint 前完成不闪烁）
 * - 点击外部 / Escape 关闭；createPortal 到 body（不受变换祖先影响）
 */

export interface ContextMenuEntry {
  key: string;
  label: string;
  /** 叶子项点击动作；有 children 的父项可省略（点击=展开/收起子项） */
  onSelect?: () => void;
  danger?: boolean;
  disabled?: boolean;
  /** 子项（如「颜色标签」的五色+清除）：点击父项原地展开，子项 testid 为
   *  `${testId}-item-${父key}-${子key}`（jsdom 可测、无需悬停定位） */
  children?: ContextMenuEntry[];
  palette?: boolean;
  colorDot?: ColorLabel | "clear";
  checked?: boolean;
}

export default function ContextMenu({
  at,
  entries,
  onClose,
  testId = "context-menu",
}: {
  /** 打开位置（光标 client 坐标） */
  at: { x: number; y: number };
  entries: ContextMenuEntry[];
  onClose: () => void;
  testId?: string;
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  const [pos, setPos] = useState({ left: at.x, top: at.y });
  /** 当前展开的父项 key（子项原地展开；同时只展开一个） */
  const [expandedKey, setExpandedKey] = useState<string | null>(null);

  // 视口钳制：菜单超出右/下边缘时向内收回（最小留 8px 边距）
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    setPos({
      left: Math.max(8, Math.min(at.x, window.innerWidth - rect.width - 8)),
      top: Math.max(8, Math.min(at.y, window.innerHeight - rect.height - 8)),
    });
  }, [at.x, at.y, expandedKey]);

  // 点击外部 / Esc 关闭（mousedown：右键另一处会先关旧菜单再由目标开新菜单；
  // Esc 挂 window——document 层监听收不到 window 目标事件）
  useEffect(() => {
    function onPointerDown(e: MouseEvent): void {
      if (ref.current && e.target instanceof Node && !ref.current.contains(e.target)) onClose();
    }
    function onKeyDown(e: KeyboardEvent): void {
      if (e.key === "Escape") onClose();
    }
    document.addEventListener("mousedown", onPointerDown);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [onClose]);

  const motionOn = useMotionOn();
  return createPortal(
    <motion.div
      ref={ref}
      role="menu"
      style={{ left: pos.left, top: pos.top, transformOrigin: "top left" }}
      className="ui-glass fixed z-[80] min-w-[190px] overflow-hidden rounded-xl border border-edge py-1.5 shadow-xl"
      data-testid={testId}
      onContextMenu={(e) => e.preventDefault()}
      initial={motionInitial(motionOn, { opacity: 0, scale: 0.96 })}
      animate={{ opacity: 1, scale: 1 }}
      transition={TRANS.quick}
    >
      {entries.map((entry) => (
        <div key={entry.key}>
          <button
            type="button"
            role="menuitem"
            disabled={entry.disabled}
            aria-expanded={entry.children ? expandedKey === entry.key : undefined}
            onClick={() => {
              if (entry.children) {
                setExpandedKey((prev) => (prev === entry.key ? null : entry.key));
                return;
              }
              onClose();
              entry.onSelect?.();
            }}
            className={`flex w-full items-center justify-between gap-2 px-3 py-1.5 text-left text-xs transition-colors ${
              entry.danger
                ? "text-red-400 hover:bg-red-400/10"
                : "text-text-secondary hover:bg-panel hover:text-text-primary"
            } disabled:cursor-default disabled:opacity-50`}
            data-testid={`${testId}-item-${entry.key}`}
          >
            {entry.label}
            {entry.children && (
              <svg
                viewBox="0 0 16 16"
                width="9"
                height="9"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.6"
                strokeLinecap="round"
                strokeLinejoin="round"
                className={`shrink-0 transition-transform ${expandedKey === entry.key ? "rotate-90" : ""}`}
                aria-hidden="true"
              >
                <path d="M6 3.5L10.5 8 6 12.5" />
              </svg>
            )}
          </button>
          {entry.children && expandedKey === entry.key && (
            <div className={entry.palette ? "flex items-center gap-1 px-3 py-2" : "mb-1 ml-4 border-l border-edge/60 pl-2"} data-testid={`${testId}-submenu-${entry.key}`}>
              {entry.children.map((child) => (
                <button
                  key={child.key}
                  type="button"
                  role={entry.palette ? "menuitemradio" : "menuitem"}
                  aria-label={child.label}
                  title={child.label}
                  aria-checked={entry.palette ? Boolean(child.checked) : undefined}
                  disabled={child.disabled}
                  onClick={() => {
                    onClose();
                    child.onSelect?.();
                  }}
                  className={entry.palette ? `rounded-full p-1 transition-transform hover:scale-110 ${child.checked ? "ring-2 ring-accent" : ""}` : `flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-xs transition-colors ${
                    child.danger
                      ? "text-red-400 hover:bg-red-400/10"
                      : "text-text-secondary hover:bg-panel hover:text-text-primary"
                  } disabled:cursor-default disabled:opacity-50`}
                  data-testid={`${testId}-item-${entry.key}-${child.key}`}
                  data-label={child.colorDot && child.colorDot !== "clear" ? child.colorDot : undefined}
                >
                  {child.colorDot && child.colorDot !== "clear" ? <span className={`block h-3.5 w-3.5 rounded-full ${COLOR_DOT_CLASS[child.colorDot]} ${COLOR_DOT_RING}`} aria-hidden="true" /> : child.colorDot === "clear" ? <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8" /></svg> : child.label}
                </button>
              ))}
            </div>
          )}
        </div>
      ))}
    </motion.div>,
    document.body,
  );
}

/**
 * 资产右键菜单（画廊瓦片 / 查看器大图共用）：在文件管理器中显示 /
 * 复制文件到剪贴板 / 旗标 / 颜色标签（LR 五色+清除子项，作用于 targets 全部）/
 * 拒绝旗标（智能切换：全部已拒绝=取消拒绝）/ 加入相册（③ 全局入口，
 * onAddToAlbum 提供时显示）。
 * onTrashRequest 提供时多「移入回收站」（红色；确认一步由上层弹窗承担）。
 * 动作对 targets 全部资产逐个执行，单个失败不中断其余（reveal 非 Tauri 环境
 * 静默；clipboard 后端在途，失败静默）。onColorLabeled/onRejected 回调供
 * 上层在 IPC 后同步本地列表（乐观 UI）。
 */
export function AssetContextMenu({
  at,
  assets,
  onClose,
  testId = "asset-context-menu",
  onAddToAlbum,
  onColorLabeled,
  onRejected,
  onTrashRequest,
  onFavoritesChanged,
  onFlagged,
  windowIds,
  selectedIds = [],
  onSelectIds,
  extraEntries = [],
}: {
  at: { x: number; y: number };
  assets: AssetDto[];
  onClose: () => void;
  testId?: string;
  /** 「加入相册」入口回调（弹窗由上层挂载）；不传则不显示该项 */
  onAddToAlbum?: (assets: AssetDto[]) => void;
  /** 相册上下文（相册详情页）：显示「从相册移除」（danger） */
  /** 颜色标签设置完成回调（IPC 后同步本地列表态） */
  onColorLabeled?: (assets: AssetDto[], label: string | null) => void;
  /** 拒绝旗标切换完成回调 */
  onRejected?: (assets: AssetDto[], rejected: boolean) => void;
  /** 「移入回收站」请求回调（确认弹窗由上层挂载） */
  onTrashRequest?: (assets: AssetDto[]) => void;
  onFavoritesChanged?: (assets: AssetDto[], favorite: boolean) => void;
  onFlagged?: (assets: AssetDto[], flagged: boolean) => void;
  windowIds?: number[];
  selectedIds?: number[];
  onSelectIds?: (ids: number[]) => void;
  extraEntries?: ContextMenuEntry[];
}) {
  const { t } = useTranslation();

  async function reveal(): Promise<void> {
    try {
      // 批量单窗定位（同目录多文件=1 窗多选）；失败回退逐个定位
      await revealInExplorer(assets.map((asset) => asset.path));
    } catch {
      for (const asset of assets) {
        try {
          await revealItemInDir(asset.path);
        } catch {
          // 非 Tauri 环境静默
        }
      }
    }
  }

  async function copyFiles(): Promise<void> {
    try {
      await clipboardCopyFiles(assets.map((asset) => asset.path));
    } catch {
      // 后端命令在途/未注册：静默（真机落地后生效）
    }
  }

  async function flag(): Promise<void> {
    const next = !assets.every((asset) => asset.flagged);
    for (const asset of assets) {
      try {
        await assetFlagSet(asset.id, next);
      } catch {
        // 静默（乐观 UI）
      }
    }
    onFlagged?.(assets, next);
  }
  async function copyPaths(): Promise<void> {
    try { await navigator.clipboard.writeText(assets.map((asset) => asset.path).join("\n")); }
    catch { /* Clipboard may be unavailable in the browser preview. */ }
  }

  const allFavorite = assets.length > 0 && assets.every((asset) => (asset.rating ?? 0) >= 5);
  async function favorite(): Promise<void> {
    const next = !allFavorite;
    await Promise.all(assets.map((asset) => assetRatingSet(asset.id, next ? 5 : 0)));
    onFavoritesChanged?.(assets, next);
  }

  /** 颜色标签批量设置（label=null 清除） */
  async function colorLabel(label: string | null): Promise<void> {
    await assetLabelSet(
      assets.map((a) => a.id),
      label,
    );
    onColorLabeled?.(assets, label);
  }

  // 拒绝旗标智能切换：全部已拒绝 → 取消拒绝；否则批量拒绝
  const allRejected = assets.length > 0 && assets.every((a) => a.rejected === true);

  async function toggleReject(): Promise<void> {
    const next = !allRejected;
    await assetRejectSet(
      assets.map((a) => a.id),
      next,
    );
    onRejected?.(assets, next);
  }


  const count = assets.length;
  const entries: ContextMenuEntry[] = [
    { key: "reveal", label: t("context.reveal"), onSelect: () => void reveal() },
    { key: "copy", label: t("context.copyFiles"), onSelect: () => void copyFiles() },
    { key: "copy-path", label: t("selection.copyPath"), onSelect: () => { void copyPaths(); } },
    { key: "favorite", label: t(allFavorite ? "selection.unfavorite" : "selection.favorite"), onSelect: () => void favorite() },
    { key: "flag", label: assets.every((asset) => asset.flagged) ? t("selection.unflag") : count > 1 ? t("context.flagMany", { count }) : t("context.flag"), onSelect: () => void flag() },
    {
      key: "color",
      label: count > 1 ? t("context.colorLabelMany", { count }) : t("context.colorLabel"),
      palette: true,
      children: [
        ...COLOR_LABELS.map((label) => ({
          key: label,
          label: t(`gallery.color.${label}`),
          colorDot: label,
          checked: assets.every((asset) => asset.colorLabel === label),
          onSelect: () => void colorLabel(label),
        })),
        { key: "clear", label: t("context.colorClear"), colorDot: "clear", onSelect: () => void colorLabel(null) },
      ],
    },
    {
      key: "reject",
      label: count > 1
        ? allRejected
          ? t("context.unrejectMany", { count })
          : t("context.rejectMany", { count })
        : allRejected
          ? t("context.unreject")
          : t("context.reject"),
      onSelect: () => void toggleReject(),
    },
  ];
  if (windowIds && onSelectIds) {
    const all = windowIds.length > 0 && windowIds.every((id) => selectedIds.includes(id));
    entries.push(
      { key: "select-all", label: t(all ? "selection.deselectAll" : "selection.all"), onSelect: () => onSelectIds(all ? [] : windowIds) },
      { key: "invert", label: t("selection.invert"), onSelect: () => onSelectIds(windowIds.filter((id) => !selectedIds.includes(id))) },
    );
  }
  entries.push(...extraEntries);
  if (onAddToAlbum) {
    entries.push({
      key: "add-album",
      label: count > 1 ? t("albums.addManyTo", { count }) : t("albums.addToAlbum"),
      onSelect: () => onAddToAlbum(assets),
    });
  }  if (onTrashRequest) {
    entries.push({
      key: "trash",
      label: count > 1 ? t("context.trashMany", { count }) : t("context.trash"),
      onSelect: () => onTrashRequest(assets),
      danger: true,
    });
  }
  return <ContextMenu at={at} onClose={onClose} testId={testId} entries={entries} />;
}
