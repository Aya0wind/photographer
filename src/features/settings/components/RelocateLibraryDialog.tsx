import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import { libraryRelocate, type LibraryRelocateResult } from "@/ipc/api";
import { useSettingsStore, type Library } from "@/stores/settingsStore";

/**
 * 库照片存储目录整体重定位（用户定案 2026-09-28）：前提是用户已在文件
 * 管理器把整棵照片树搬到新根；应用改配置 + 重写库内路径前缀 + 重排缩略图。
 * 打开即预检（dry-run）显示受影响计数；新根不在盘时提示但不阻止
 * （可先改后挂载，缺失走 missing 终态提示）。
 */
export default function RelocateLibraryDialog({
  library,
  onClose,
}: {
  library: Library;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [root, setRoot] = useState(library.photoRoot);
  const [preview, setPreview] = useState<LibraryRelocateResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [applying, setApplying] = useState(false);
  const [done, setDone] = useState<LibraryRelocateResult | null>(null);

  const changed = root.trim().length > 0 && root.trim() !== library.photoRoot;

  // 预检（打开时对当前根一次；输入停顿 600ms 重查）
  useEffect(() => {
    if (!changed || applying || done) return;
    const timer = window.setTimeout(() => {
      void libraryRelocate(library.id, root.trim(), false)
        .then((r) => {
          setPreview(r);
          setError(null);
        })
        .catch((e) => {
          setPreview(null);
          setError(e instanceof Error ? e.message : String(e));
        });
    }, 600);
    return () => window.clearTimeout(timer);
  }, [root, changed, applying, done, library.id]);

  async function apply(): Promise<void> {
    setApplying(true);
    try {
      const r = await libraryRelocate(library.id, root.trim(), true);
      setDone(r);
      // 后端已更新内存快照并落盘 + 发 settings://changed；这里再主动拉
      // 一次，确保本会话 store 立即反映新根
      await useSettingsStore.getState().load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setApplying(false);
    }
  }

  const field =
    "w-full rounded-md border border-edge bg-bg px-2.5 py-1.5 font-mono text-xs text-text-primary outline-none transition-colors focus:border-accent";

  return (
    <div
      className="fixed inset-0 z-[70] flex items-center justify-center bg-black/60"
      onClick={onClose}
      role="dialog"
      data-testid="relocate-library-dialog"
    >
      <div
        className="w-[460px] rounded-xl border border-edge bg-surface p-5"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="text-sm font-semibold text-text-primary">
          {t("settings.relocate.title", { name: library.name })}
        </h2>

        {done === null ? (
          <>
            <p className="mt-2 text-xs leading-relaxed text-text-secondary">
              {t("settings.relocate.hint")}
            </p>
            <p className="mt-1 font-mono text-[11px] text-text-muted">{library.photoRoot}</p>

            <label className="mt-3 flex flex-col gap-1 text-xs text-text-secondary">
              {t("settings.relocate.newRoot")}
              <div className="flex gap-2">
                <input
                  type="text"
                  value={root}
                  onChange={(e) => setRoot(e.target.value)}
                  className={field}
                  autoFocus
                  data-testid="relocate-new-root"
                />
                <button
                  type="button"
                  className="shrink-0 rounded-md border border-edge px-3 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
                  onClick={() => {
                    void openDialog({ directory: true, defaultPath: root || undefined }).then(
                      (dir) => {
                        if (typeof dir === "string" && dir.length > 0) setRoot(dir);
                      },
                    );
                  }}
                >
                  {t("settings.relocate.browse")}
                </button>
              </div>
            </label>

            {error !== null && (
              <p className="mt-2 text-[11px] text-red-400" role="alert" data-testid="relocate-error">
                {error}
              </p>
            )}
            {preview !== null && (
              <div
                className="mt-3 rounded-md border border-edge bg-bg p-2.5 text-[11px] leading-relaxed text-text-secondary"
                data-testid="relocate-preview"
              >
                <p>
                  {t("settings.relocate.affected", { count: preview.affected })}
                  {preview.unaffected > 0 &&
                    t("settings.relocate.unaffected", { count: preview.unaffected })}
                </p>
                {!preview.rootExists && (
                  <p className="mt-1 text-amber-300">{t("settings.relocate.rootMissing")}</p>
                )}
              </div>
            )}

            <div className="mt-4 flex justify-end gap-2">
              <button
                type="button"
                onClick={onClose}
                className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
                data-testid="relocate-cancel"
              >
                {t("common.cancel")}
              </button>
              <button
                type="button"
                disabled={!changed || applying || error !== null}
                onClick={() => void apply()}
                className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
                data-testid="relocate-confirm"
              >
                {applying ? t("settings.relocate.applying") : t("settings.relocate.confirm")}
              </button>
            </div>
          </>
        ) : (
          <>
            <p
              className="mt-3 rounded-md border border-edge bg-bg p-2.5 text-[11px] leading-relaxed text-text-secondary"
              data-testid="relocate-done"
            >
              {t("settings.relocate.done", {
                affected: done.affected,
                unaffected: done.unaffected,
                root: root.trim(),
              })}
            </p>
            <div className="mt-4 flex justify-end">
              <button
                type="button"
                onClick={() => {
                  onClose();
                  // 整页刷新：清掉缩略图管线的会话级终态缓存（missing
                  // failedCache 等），画廊按新路径全量重拉（重定位是罕见
                  // 重操作，刷新成本可接受；仅 Tauri 环境）
                  if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) {
                    window.location.reload();
                  }
                }}
                className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110"
                data-testid="relocate-close"
              >
                {t("common.close")}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
