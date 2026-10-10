import { useState } from "react";
import { useTranslation } from "react-i18next";

import { photoLibraryRemove, type PhotoLibrary, type PhotoLibraryRemoveResult } from "@/ipc/api";

/**
 * 删除照片库确认（M3 存储页）：**永不删除照片文件**（用户红线）——只摘除
 * photos_libraries 登记；是否连库内资产记录一起删由用户在本对话框选择
 * （photoLibraryRemove deleteRecords 两档）。
 * 「连记录删」为两段式（2026-10-10 定案）：先点选进入待确认态，必须输入
 * 确认短语（locale 相关，zh=「我确认删除」）才能执行；「仅摘登记」保持一步。
 * 失败文案透传内联展示；成功后由 onRemoved 回调刷新列表 + 提示。
 */
export default function RemoveLibraryDialog({
  library,
  onClose,
  onRemoved,
}: {
  library: PhotoLibrary;
  onClose: () => void;
  /** 移除成功回调（参数=命令结果，含连带删除的记录数） */
  onRemoved: (result: PhotoLibraryRemoveResult) => void;
}) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** 连记录删的待确认态（true=已展开短语输入） */
  const [armDelete, setArmDelete] = useState(false);
  const [phrase, setPhrase] = useState("");

  const expected = t("storage.removeDialog.confirmPhrase");
  const phraseMatched = phrase.trim() === expected && expected.length > 0;

  async function remove(deleteRecords: boolean): Promise<void> {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const result = await photoLibraryRemove(library.id, deleteRecords);
      onRemoved(result);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setBusy(false);
    }
  }

  return (
    <div
      className="fixed inset-0 z-[70] flex items-center justify-center bg-black/55 p-6"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
      role="dialog"
      aria-modal="true"
      data-testid="storage-remove-dialog"
      data-arm-delete={armDelete}
    >
      <div className="w-full max-w-md rounded-xl border border-edge bg-surface p-4 shadow-2xl">
        <h2 className="text-sm font-semibold text-text-primary" data-testid="storage-remove-title">
          {t("storage.removeDialog.title", { name: library.name })}
        </h2>
        <p className="mt-1 font-mono text-[11px] text-text-muted">{library.rootPath}</p>
        <p className="mt-2 text-xs leading-relaxed text-text-secondary">
          {t("storage.removeDialog.desc", { count: library.assetCount })}
        </p>
        {/* 用户红线：删除登记永不删除照片文件 */}
        <p className="mt-1 text-xs text-amber-300" data-testid="storage-remove-never-delete">
          {t("storage.removeDialog.neverDelete")}
        </p>

        {error !== null && (
          <p className="mt-2 text-[11px] text-red-400" role="alert" data-testid="storage-remove-error">
            {error}
          </p>
        )}

        <div className="mt-4 flex flex-col gap-2">
          <button
            type="button"
            disabled={busy}
            onClick={() => void remove(false)}
            className="rounded-md border border-edge px-3 py-2 text-xs text-text-secondary transition-colors hover:bg-panel hover:text-text-primary disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="storage-remove-keep-records"
          >
            {t("storage.removeDialog.keep")}
          </button>

          {!armDelete ? (
            <button
              type="button"
              disabled={busy}
              onClick={() => setArmDelete(true)}
              className="rounded-md border border-red-400/60 px-3 py-2 text-xs text-red-400 transition-colors hover:bg-red-400/10 disabled:cursor-not-allowed disabled:opacity-40"
              data-testid="storage-remove-delete-records"
            >
              {t("storage.removeDialog.delete", { count: library.assetCount })}
            </button>
          ) : (
            <div
              className="flex flex-col gap-2 rounded-md border border-red-400/40 bg-red-400/5 p-2.5"
              data-testid="storage-remove-arm"
            >
              <label className="flex flex-col gap-1 text-xs text-red-400">
                {t("storage.removeDialog.typeHint", {
                  count: library.assetCount,
                  phrase: expected,
                })}
                <input
                  type="text"
                  value={phrase}
                  onChange={(e) => setPhrase(e.target.value)}
                  autoFocus
                  disabled={busy}
                  className="rounded-md border border-edge bg-bg px-2.5 py-1.5 font-mono text-xs text-text-primary outline-none transition-colors focus:border-red-400 disabled:opacity-40"
                  data-testid="storage-remove-phrase"
                />
              </label>
              <button
                type="button"
                disabled={busy || !phraseMatched}
                onClick={() => void remove(true)}
                className="rounded-md bg-red-500/90 px-3 py-2 text-xs font-medium text-white transition-colors hover:bg-red-500 disabled:cursor-not-allowed disabled:opacity-40"
                data-testid="storage-remove-confirm-delete"
              >
                {t("storage.removeDialog.confirmDelete")}
              </button>
            </div>
          )}

          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-edge px-3 py-2 text-xs text-text-secondary transition-colors hover:bg-panel hover:text-text-primary"
            data-testid="storage-remove-cancel"
          >
            {t("common.cancel")}
          </button>
        </div>
      </div>
    </div>
  );
}
