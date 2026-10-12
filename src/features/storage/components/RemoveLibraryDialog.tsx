import { useState } from "react";
import { useTranslation } from "react-i18next";

import { photoLibraryRemove, type PhotoLibrary, type PhotoLibraryRemoveResult } from "@/ipc/api";

/** 删除照片库（单级确认，2026-10-12 用户定案）：打开即见「同时删除原文件」
 * 勾选 + 输入库名确认——不再套两级待确认步骤；不勾选只删登记记录，
 * 磁盘照片文件永不被动。 */
export default function RemoveLibraryDialog({
  library,
  onClose,
  onRemoved,
}: {
  library: PhotoLibrary;
  onClose: () => void;
  /** 删除成功回调（参数=命令结果，含连带删除的记录/文件数） */
  onRemoved: (result: PhotoLibraryRemoveResult) => void;
}) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [name, setName] = useState("");
  /** 同时删除磁盘上的照片文件（引用库不适用） */
  const [deleteFiles, setDeleteFiles] = useState(false);

  const nameMatched = name.trim() === library.name && library.name.length > 0;

  async function remove(): Promise<void> {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const result = await photoLibraryRemove(library.id, deleteFiles);
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
    >
      <div className="w-full max-w-md rounded-xl border border-edge bg-surface p-4 shadow-2xl">
        <h2 className="text-sm font-semibold text-text-primary" data-testid="storage-remove-title">
          {t("storage.removeDialog.title", { name: library.name })}
        </h2>
        <p className="mt-1 font-mono text-[11px] text-text-muted">{library.rootPath}</p>
        <p className="mt-2 text-xs leading-relaxed text-text-secondary">
          {t("storage.removeDialog.desc", { count: library.assetCount })}
        </p>

        {error !== null && (
          <p className="mt-2 text-[11px] text-red-400" role="alert" data-testid="storage-remove-error">
            {error}
          </p>
        )}

        <div className="mt-4 flex flex-col gap-2 rounded-md border border-red-400/40 bg-red-400/5 p-2.5" data-testid="storage-remove-confirm">
          <label className="flex items-center gap-1.5 text-xs text-red-400">
            <input
              type="checkbox"
              checked={deleteFiles}
              onChange={(e) => setDeleteFiles(e.target.checked)}
              disabled={busy}
              className="h-3.5 w-3.5 accent-red-500 disabled:opacity-40"
              data-testid="storage-remove-delete-files"
            />
            {t("storage.removeDialog.deleteFiles")}
          </label>
          {deleteFiles && (
            <p
              className="text-[11px] leading-relaxed text-red-400"
              data-testid="storage-remove-files-hint"
            >
              {t("storage.removeDialog.deleteFilesHint", { count: library.assetCount })}
            </p>
          )}
          <label className="flex flex-col gap-1 text-xs text-red-400">
            {t("storage.removeDialog.typeHint", { name: library.name })}
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              autoFocus
              disabled={busy}
              className="rounded-md border border-edge bg-bg px-2.5 py-1.5 font-mono text-xs text-text-primary outline-none transition-colors focus:border-red-400 disabled:opacity-40"
              data-testid="storage-remove-name"
            />
          </label>
          <button
            type="button"
            disabled={busy || !nameMatched}
            onClick={() => void remove()}
            className="rounded-md bg-red-500/90 px-3 py-2 text-xs font-medium text-white transition-colors hover:bg-red-500 disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="storage-remove-confirm-delete"
          >
            {t("storage.removeDialog.confirmDelete")}
          </button>
        </div>

        <button
          type="button"
          onClick={onClose}
          className="mt-2 w-full rounded-md border border-edge px-3 py-2 text-xs text-text-secondary transition-colors hover:bg-panel hover:text-text-primary"
          data-testid="storage-remove-cancel"
        >
          {t("common.cancel")}
        </button>
      </div>
    </div>
  );
}
