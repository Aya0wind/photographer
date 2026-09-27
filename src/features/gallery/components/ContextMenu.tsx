import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

import {
  albumRemoveAssets,
  assetFlagSet,
  assetLabelSet,
  assetRejectSet,
  clipboardCopyFiles,
  revealInExplorer,
  type AssetDto,
} from "@/ipc/api";
import { COLOR_LABELS } from "../lib/colorLabels";

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
  }, [at.x, at.y]);

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

  return createPortal(
    <div
      ref={ref}
      role="menu"
      style={{ left: pos.left, top: pos.top }}
      className="fixed z-[80] min-w-[190px] overflow-hidden rounded-md border border-edge bg-surface py-1 shadow-xl"
      data-testid={testId}
      onContextMenu={(e) => e.preventDefault()}
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
            <div className="mb-1 ml-4 border-l border-edge/60 pl-2" data-testid={`${testId}-submenu-${entry.key}`}>
              {entry.children.map((child) => (
                <button
                  key={child.key}
                  type="button"
                  role="menuitem"
                  disabled={child.disabled}
                  onClick={() => {
                    onClose();
                    child.onSelect?.();
                  }}
                  className={`flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-xs transition-colors ${
                    child.danger
                      ? "text-red-400 hover:bg-red-400/10"
                      : "text-text-secondary hover:bg-panel hover:text-text-primary"
                  } disabled:cursor-default disabled:opacity-50`}
                  data-testid={`${testId}-item-${entry.key}-${child.key}`}
                >
                  {child.label}
                </button>
              ))}
            </div>
          )}
        </div>
      ))}
    </div>,
    document.body,
  );
}

/**
 * 资产右键菜单（画廊瓦片 / 查看器大图共用）：在资源管理器中显示 /
 * 复制文件到剪贴板 / 旗标 / 颜色标签（LR 五色+清除子项，作用于 targets 全部）/
 * 拒绝旗标（智能切换：全部已拒绝=取消拒绝）/ 加入相册（③ 全局入口，
 * onAddToAlbum 提供时显示）。
 * 相册上下文（相册详情页，albumContext 提供时）额外多一项「从相册移除」。
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
  albumContext,
  onColorLabeled,
  onRejected,
  onTrashRequest,
}: {
  at: { x: number; y: number };
  assets: AssetDto[];
  onClose: () => void;
  testId?: string;
  /** 「加入相册」入口回调（弹窗由上层挂载）；不传则不显示该项 */
  onAddToAlbum?: (assets: AssetDto[]) => void;
  /** 相册上下文（相册详情页）：显示「从相册移除」（danger） */
  albumContext?: { albumId: number; onRemoved: () => void };
  /** 颜色标签设置完成回调（IPC 后同步本地列表态） */
  onColorLabeled?: (assets: AssetDto[], label: string | null) => void;
  /** 拒绝旗标切换完成回调 */
  onRejected?: (assets: AssetDto[], rejected: boolean) => void;
  /** 「移入回收站」请求回调（确认弹窗由上层挂载） */
  onTrashRequest?: (assets: AssetDto[]) => void;
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
    for (const asset of assets) {
      try {
        await assetFlagSet(asset.id, true);
      } catch {
        // 静默（乐观 UI）
      }
    }
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

  /** 相册上下文：从相册移除引用（仅删引用，照片保留图库），完成后上层刷新 */
  async function removeFromAlbum(): Promise<void> {
    if (!albumContext) return;
    const ok = await albumRemoveAssets(
      albumContext.albumId,
      assets.map((a) => a.id),
    );
    if (ok) albumContext.onRemoved();
  }

  const count = assets.length;
  const entries: ContextMenuEntry[] = [
    { key: "reveal", label: t("context.reveal"), onSelect: () => void reveal() },
    { key: "copy", label: t("context.copyFiles"), onSelect: () => void copyFiles() },
    { key: "flag", label: count > 1 ? t("context.flagMany", { count }) : t("context.flag"), onSelect: () => void flag() },
    {
      key: "color",
      label: count > 1 ? t("context.colorLabelMany", { count }) : t("context.colorLabel"),
      children: [
        ...COLOR_LABELS.map((label) => ({
          key: label,
          label: t(`gallery.color.${label}`),
          onSelect: () => void colorLabel(label),
        })),
        { key: "clear", label: t("context.colorClear"), onSelect: () => void colorLabel(null) },
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
  if (onAddToAlbum) {
    entries.push({
      key: "add-album",
      label: count > 1 ? t("albums.addManyTo", { count }) : t("albums.addToAlbum"),
      onSelect: () => onAddToAlbum(assets),
    });
  }
  if (albumContext) {
    entries.push({
      key: "remove-album",
      label: count > 1 ? t("albums.removeManyFrom", { count }) : t("albums.removeFromAlbum"),
      onSelect: () => void removeFromAlbum(),
      danger: true,
    });
  }
  if (onTrashRequest) {
    entries.push({
      key: "trash",
      label: count > 1 ? t("context.trashMany", { count }) : t("context.trash"),
      onSelect: () => onTrashRequest(assets),
      danger: true,
    });
  }
  return <ContextMenu at={at} onClose={onClose} testId={testId} entries={entries} />;
}
