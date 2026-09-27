import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

import { lrStagingCreate, revealInExplorer, type AssetDto, type LrStagingResult } from "@/ipc/api";

/**
 * 生成 LR 暂存夹（B1 追加包，多选操作条/瓦片右键共用，弹窗由图库页挂载）：
 * - 名称输入（默认 `MMDD-选中数`，如 0927-6；可改）→ lr_staging_create
 * - 结果浮层：目录路径 + 「硬链接 N 张 / 复制 M 张」 + 「打开文件夹」
 *   （reveal_in_explorer 批量单窗定位，失败回退逐个 opener）
 * - 失败（命令在途/后端不可用）行内提示，不静默
 */

/** 本地今天 → "MMDD"（暂存夹默认名前缀） */
function mmddOf(now = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(now.getMonth() + 1)}${pad(now.getDate())}`;
}

export default function LrStagingDialog({
  assets,
  onClose,
  defaultName,
}: {
  assets: AssetDto[];
  onClose: () => void;
  /** 覆盖默认名（相册上下文可用相册名）；缺省 `MMDD-选中数` */
  defaultName?: string;
}) {
  const { t } = useTranslation();
  const fallbackName = useMemo(() => `${mmddOf()}-${assets.length}`, [assets.length]);
  const [name, setName] = useState(defaultName?.trim() ? defaultName : fallbackName);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<LrStagingResult | null>(null);

  // Esc 关闭（结果浮层打开时同样生效）
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  async function create(): Promise<void> {
    if (creating) return;
    setCreating(true);
    setError(null);
    const created = await lrStagingCreate(
      assets.map((a) => a.id),
      name,
    );
    setCreating(false);
    if (!created) {
      setError(t("lr.failed"));
      return;
    }
    setResult(created);
  }

  async function openFolder(): Promise<void> {
    if (result === null) return;
    try {
      await revealInExplorer([result.dir]);
    } catch {
      try {
        await revealItemInDir(result.dir);
      } catch {
        // 非 Tauri 环境静默
      }
    }
  }

  return (
    <div
      className="fixed inset-0 z-[70] flex items-center justify-center bg-black/45"
      onClick={onClose}
      data-testid="lr-staging-overlay"
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={t("lr.title")}
        className="w-[380px] overflow-hidden rounded-xl border border-edge bg-surface shadow-2xl"
        onClick={(e) => e.stopPropagation()}
        data-testid="lr-staging-dialog"
      >
        {result === null ? (
          <>
            <div className="flex shrink-0 items-center justify-between border-b border-edge px-4 py-3">
              <h2 className="text-sm font-semibold text-text-primary">
                {t("lr.title")}
                <span className="ml-2 font-mono text-[11px] font-normal text-text-muted">
                  {t("albums.addCount", { count: assets.length })}
                </span>
              </h2>
              <button
                type="button"
                onClick={onClose}
                aria-label={t("common.close")}
                className="rounded px-1.5 text-lg leading-none text-text-muted transition-colors hover:text-text-primary"
                data-testid="lr-staging-close"
              >
                ×
              </button>
            </div>
            <div className="px-4 py-3">
              <p className="text-[11px] leading-relaxed text-text-muted">{t("lr.hint")}</p>
              <label className="mt-3 flex items-center gap-2 text-[11px] text-text-muted">
                <span className="shrink-0">{t("lr.nameLabel")}</span>
                <input
                  autoFocus
                  type="text"
                  value={name}
                  onChange={(e) => {
                    setName(e.target.value);
                    setError(null);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      void create();
                    }
                  }}
                  aria-label={t("lr.namePlaceholder")}
                  placeholder={t("lr.namePlaceholder")}
                  className="min-w-0 flex-1 rounded-md border border-edge bg-bg px-2.5 py-1.5 text-xs text-text-primary outline-none transition-colors placeholder:text-text-muted/60 focus:border-accent"
                  data-testid="lr-staging-name"
                />
              </label>
              {error !== null && (
                <p className="mt-2 text-[11px] text-red-400" role="alert" data-testid="lr-staging-error">
                  {error}
                </p>
              )}
            </div>
            <div className="flex shrink-0 items-center justify-end gap-2 border-t border-edge px-4 py-3">
              <button
                type="button"
                onClick={onClose}
                className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
                data-testid="lr-staging-cancel"
              >
                {t("common.cancel")}
              </button>
              <button
                type="button"
                onClick={() => void create()}
                disabled={creating || name.trim() === ""}
                className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
                data-testid="lr-staging-confirm"
              >
                {creating ? t("lr.creating") : t("lr.confirm")}
              </button>
            </div>
          </>
        ) : (
          <div className="px-4 py-4" data-testid="lr-staging-result">
            <p className="text-sm font-semibold text-text-primary">{t("lr.done")}</p>
            <p
              className="mt-2 break-all rounded-md bg-bg px-2.5 py-2 font-mono text-[11px] text-text-secondary"
              data-testid="lr-staging-path"
              title={result.dir}
            >
              {result.dir}
            </p>
            <p className="mt-2 text-[11px] text-text-muted" data-testid="lr-staging-counts">
              {t("lr.counts", { hardlinked: result.hardlinked, copied: result.copied })}
            </p>
            <div className="mt-4 flex items-center justify-end gap-2">
              <button
                type="button"
                onClick={() => void openFolder()}
                className="rounded-md border border-accent/60 bg-accent/10 px-3 py-1.5 text-xs font-medium text-accent transition-colors hover:bg-accent/20"
                data-testid="lr-staging-open"
              >
                {t("lr.openFolder")}
              </button>
              <button
                type="button"
                onClick={onClose}
                className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:bg-panel hover:text-text-primary"
                data-testid="lr-staging-done"
              >
                {t("common.done")}
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
