import { useState } from "react";
import { useTranslation } from "react-i18next";

import { photoLibraryRemove, type PhotoLibrary, type PhotoLibraryRemoveResult } from "@/ipc/api";

/** 统一照片库：已有文件原位建立索引，新增文件按库的组织规则存放。 */
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
  /** 删除的待确认态（true=已展开勾选与短语输入） */
  const [armDelete, setArmDelete] = useState(false);
  const [phrase, setPhrase] = useState("");
  /** 同时删除磁盘上的照片文件（引用库不适用） */
  const [deleteFiles, setDeleteFiles] = useState(false);

  const expected = t("storage.removeDialog.confirmPhrase");
  const phraseMatched = phrase.trim() === expected && expected.length > 0;

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
        {/* 默认不动磁盘照片文件；进入待确认态后由删文件勾选/引用库说明行
            接管文件语义提示 */}
        {!armDelete && (
          <p className="mt-1 text-xs text-amber-300" data-testid="storage-remove-never-delete">
            {t("storage.removeDialog.neverDelete")}
          </p>
        )}

        {error !== null && (
          <p className="mt-2 text-[11px] text-red-400" role="alert" data-testid="storage-remove-error">
            {error}
          </p>
        )}

        <div className="mt-4 flex flex-col gap-2">
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
              {(                <>
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
                </>
              )}
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
                onClick={() => void remove()}
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
