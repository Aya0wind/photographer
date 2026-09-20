import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

import { assetFlagSet, clipboardCopyFiles, revealInExplorer, type AssetDto } from "@/ipc/api";

/**
 * 自定义右键菜单（全局 contextmenu 已被 nativeBehaviorGuard 屏蔽）：
 * - 光标定位 + 视口钳制（测量后向内收回，paint 前完成不闪烁）
 * - 点击外部 / Escape 关闭；createPortal 到 body（不受变换祖先影响）
 */

export interface ContextMenuEntry {
  key: string;
  label: string;
  onSelect: () => void;
  danger?: boolean;
  disabled?: boolean;
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
        <button
          key={entry.key}
          type="button"
          role="menuitem"
          disabled={entry.disabled}
          onClick={() => {
            onClose();
            entry.onSelect();
          }}
          className={`block w-full px-3 py-1.5 text-left text-xs transition-colors ${
            entry.danger
              ? "text-red-400 hover:bg-red-400/10"
              : "text-text-secondary hover:bg-panel hover:text-text-primary"
          } disabled:cursor-default disabled:opacity-50`}
          data-testid={`${testId}-item-${entry.key}`}
        >
          {entry.label}
        </button>
      ))}
    </div>,
    document.body,
  );
}

/**
 * 资产右键菜单（画廊瓦片 / 查看器大图共用）：在资源管理器中显示 /
 * 复制文件到剪贴板 / 旗标。动作对 targets 全部资产逐个执行，单个失败
 * 不中断其余（reveal 非 Tauri 环境静默；clipboard 后端在途，失败静默）。
 */
export function AssetContextMenu({
  at,
  assets,
  onClose,
  testId = "asset-context-menu",
}: {
  at: { x: number; y: number };
  assets: AssetDto[];
  onClose: () => void;
  testId?: string;
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

  const count = assets.length;
  return (
    <ContextMenu
      at={at}
      onClose={onClose}
      testId={testId}
      entries={[
        { key: "reveal", label: t("context.reveal"), onSelect: () => void reveal() },
        { key: "copy", label: t("context.copyFiles"), onSelect: () => void copyFiles() },
        { key: "flag", label: count > 1 ? t("context.flagMany", { count }) : t("context.flag"), onSelect: () => void flag() },
      ]}
    />
  );
}
