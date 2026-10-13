import { useState } from "react";
import { useTranslation } from "react-i18next";

const DELETE_CONFIRMATION = "确认删除";

/** One safe default for selected deletion and emptying the complete trash. */
export default function TrashPurgeDialog({ targets, busy, error, onConfirm, onCancel }: {
  targets: number[] | "all";
  busy: boolean;
  error: string | null;
  onConfirm: (deleteFiles: boolean, confirmation: string) => Promise<void>;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const [deleteFiles, setDeleteFiles] = useState(false);
  const [confirmation, setConfirmation] = useState("");
  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/55 p-6"
      data-testid="trash-purge-dialog"
      role="dialog"
      aria-modal="true"
      aria-label={targets === "all" ? t("trash.emptyAll") : t("trash.purgeTitle", { count: targets.length })}
      onClick={(e) => {
        if (!busy && e.target === e.currentTarget) onCancel();
      }}
    >
      <div className="w-full max-w-md rounded-xl border border-edge bg-surface p-4 shadow-2xl">
        <h2 className="text-sm font-semibold text-text-primary" data-testid="trash-purge-title">
          {targets === "all" ? t("trash.emptyAll") : t("trash.purgeTitle", { count: targets.length })}
        </h2>
        <p className="mt-2 text-xs leading-relaxed text-text-secondary">
          {targets === "all" ? t("trash.emptyAllHint") : t("trash.purgeDesc", { count: targets.length })}
        </p>
        <p className="mt-1 text-xs text-red-400">{t("trash.purgeIrreversible")}</p>
        {error && <p role="alert" className="mt-2 text-xs text-red-400">{error}</p>}
        <label className="mt-4 flex items-center gap-2 text-xs text-text-primary">
          <input type="checkbox" checked={deleteFiles} disabled={busy} onChange={(event) => {
            setDeleteFiles(event.target.checked); setConfirmation("");
          }} data-testid="trash-delete-files" />
          {t("trash.deleteSourceFiles")}
        </label>
        {deleteFiles && <label className="mt-3 block text-xs text-red-400">
          {t("trash.confirmSourceDelete")}
          <input type="text" value={confirmation} disabled={busy} autoComplete="off" spellCheck={false}
            onChange={(event) => setConfirmation(event.target.value)} data-testid="trash-delete-confirmation"
            className="mt-2 block w-full rounded-md border border-edge bg-bg px-3 py-2 text-text-primary" />
        </label>}
        <div className="mt-4 flex flex-col gap-2">
          <button type="button" onClick={() => void onConfirm(deleteFiles, confirmation)}
            disabled={busy || (deleteFiles && confirmation !== DELETE_CONFIRMATION)}
            className="rounded-md bg-red-500 px-3 py-2 text-xs font-medium text-white hover:bg-red-500/85 disabled:opacity-40"
            data-testid="trash-purge-confirm">{t(targets === "all" ? "trash.emptyAll" : "trash.purge")}</button>
          <button
            type="button"
            onClick={() => onCancel()}
            disabled={busy}
            className="rounded-md border border-edge px-3 py-2 text-xs text-text-secondary transition-colors hover:bg-panel hover:text-text-primary"
            data-testid="trash-purge-cancel"
          >
            {t("common.cancel")}
          </button>
        </div>
      </div>
    </div>
  );
}
