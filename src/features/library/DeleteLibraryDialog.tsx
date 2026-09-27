import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { libraryDelete } from "@/ipc/api";
import type { Library } from "@/stores/settingsStore";

/**
 * 删除库对话框（用户定案 2026-09-27）：
 * - 输入库全名逐字匹配才能点确认（GitHub 式防误删闸）
 * - 库数据目录（数据库/缩略图/索引/向量）必然删除
 * - 勾选「同时删除照片目录」才连照片一起删（默认不勾，保留照片）
 * - 删除流程：若目标为当前激活库，先保存注册表清 activeLibraryId（后端
 *   拒删活跃库是双保险）→ 物理删除 → 从 libraries 摘除注册
 */
export default function DeleteLibraryDialog({
  library,
  onClose,
  onDeleted,
}: {
  library: Library;
  onClose: () => void;
  /** 删除成功回调（注册表已更新，调用方刷新列表） */
  onDeleted: () => void;
}) {
  const { t } = useTranslation();
  const [typed, setTyped] = useState("");
  const [alsoPhotos, setAlsoPhotos] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setTyped("");
    setAlsoPhotos(false);
    setBusy(false);
    setError(null);
  }, [library.id]);

  const nameMatches = typed.trim() === library.name.trim();

  async function confirmDelete(): Promise<void> {
    if (!nameMatches || busy) return;
    setBusy(true);
    setError(null);
    try {
      const settingsStore = await import("@/stores/settingsStore");
      const { useSettingsStore } = settingsStore;
      const { save, setLibraryChosen, settings } = useSettingsStore.getState();
      // 活跃库先清激活（后端拒删活跃库）；同时在册摘除该库
      const wasActive = settings.activeLibraryId === library.id;
      const next = {
        ...settings,
        libraries: settings.libraries.filter((l) => l.id !== library.id),
        activeLibraryId: wasActive ? null : settings.activeLibraryId,
      };
      await save(next);
      if (wasActive) setLibraryChosen(false);
      // 物理删除（库数据目录必删；勾选才连照片目录）
      await libraryDelete(library.dbDir, alsoPhotos ? library.photoRoot : undefined);
      onDeleted();
      onClose();
    } catch (e) {
      setError(String(e).replace(/^Error:\s*/, ""));
      setBusy(false);
    }
  }

  return (
    <div
      className="fixed inset-0 z-[80] flex items-center justify-center bg-black/60"
      onClick={onClose}
      data-testid="delete-library-dialog"
    >
      <div
        className="w-[420px] rounded-xl border border-edge bg-surface p-5"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="text-sm font-semibold text-red-400">
          {t("picker.deleteTitle", { name: library.name })}
        </h2>
        <p className="mt-2 text-xs leading-relaxed text-text-secondary">
          {t("picker.deleteWarningData")}
          <span className="mt-1 block font-mono text-[11px] text-text-muted">{library.dbDir}</span>
        </p>

        <label className="mt-3 flex cursor-pointer items-start gap-2 rounded-md border border-red-400/40 bg-red-400/5 p-2.5 text-xs text-text-secondary">
          <input
            type="checkbox"
            checked={alsoPhotos}
            onChange={(e) => setAlsoPhotos(e.target.checked)}
            className="mt-0.5 accent-red-500"
            data-testid="delete-library-photos"
          />
          <span>
            {t("picker.deleteAlsoPhotos")}
            <span className="mt-0.5 block font-mono text-[11px] text-red-400">
              {library.photoRoot}
            </span>
          </span>
        </label>

        <label className="mt-3 flex flex-col gap-1 text-xs text-text-secondary">
          {t("picker.deleteConfirmHint", { name: library.name })}
          <input
            type="text"
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
            className="w-full rounded-md border border-edge bg-bg px-2.5 py-1.5 text-xs text-text-primary outline-none transition-colors focus:border-red-400"
            autoFocus
            data-testid="delete-library-input"
          />
        </label>

        {error && (
          <p className="mt-2 text-[11px] text-red-400" role="alert" data-testid="delete-library-error">
            {error}
          </p>
        )}

        <div className="mt-4 flex justify-end gap-2">
          <button
            type="button"
            onClick={onClose}
            disabled={busy}
            className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent disabled:opacity-40"
            data-testid="delete-library-cancel"
          >
            {t("common.cancel")}
          </button>
          <button
            type="button"
            onClick={() => void confirmDelete()}
            disabled={!nameMatches || busy}
            className="rounded-md bg-red-500 px-3 py-1.5 text-xs font-medium text-white transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="delete-library-confirm"
          >
            {busy ? t("picker.deleting") : t("picker.deleteConfirm")}
          </button>
        </div>
      </div>
    </div>
  );
}
