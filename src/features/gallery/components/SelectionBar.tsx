import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

import { assetFlagSet, assetRatingSet, type AssetDto } from "@/ipc/api";

/**
 * 多选浮动操作条（M4.5，画廊选择模式）：顶部居中浮条——已选 N 张 |
 * 收藏（星标=rating 5）/ 旗标 / 分享（在资源管理器中显示 = opener reveal、
 * 复制文件路径）/ 取消。动作对全部选中资产循环调用；失败静默（乐观 UI）。
 */

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

export default function SelectionBar({
  count,
  assets,
  onDone,
}: {
  count: number;
  assets: AssetDto[];
  onDone: () => void;
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
    flash(t("selection.done"));
  }

  async function flag(): Promise<void> {
    for (const asset of assets) await assetFlagSet(asset.id, true);
    flash(t("selection.done"));
  }

  async function reveal(): Promise<void> {
    setShareOpen(false);
    let ok = 0;
    for (const asset of assets) {
      try {
        await revealItemInDir(asset.path);
        ok += 1;
      } catch {
        // 非 Tauri 环境静默
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
