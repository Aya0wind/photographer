import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  databaseRemove,
  type DatabaseEntry,
  type DatabaseRemoveResult,
} from "@/ipc/api";
import ErrorModal from "@/shared/components/ErrorModal";

/**
 * 删除数据库对话框（老 DeleteLibraryDialog 的 UI 形态 + 新两档内核）：
 * - 输入数据库名逐字匹配才能点确认（GitHub 式防误删闸）
 * - 默认仅从注册表摘登记（数据目录原样留在磁盘，可日后重新登记）
 * - 勾选「同时删除数据目录」才连 library.db/缩略图/向量一起删（不可恢复）
 * - **绝不删除照片库文件夹**（用户红线，后端 database_remove 另有照片库
 *   根重叠安全闸双保险）；删除后 databasesChanged 广播，调用方列表自动刷新
 */
export default function RemoveDatabaseDialog({
  database,
  onClose,
  onRemoved,
}: {
  database: DatabaseEntry;
  onClose: () => void;
  /** 删除成功回调（参数=命令结果，含实际删除的数据目录数） */
  onRemoved: (result: DatabaseRemoveResult) => void;
}) {
  const { t } = useTranslation();
  const [typed, setTyped] = useState("");
  const [alsoData, setAlsoData] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setTyped("");
    setAlsoData(false);
    setBusy(false);
    setError(null);
  }, [database.id]);

  const nameMatches = typed.trim() === database.name.trim();

  async function confirmRemove(): Promise<void> {
    if (!nameMatches || busy) return;
    setBusy(true);
    setError(null);
    try {
      const result = await databaseRemove(database.id, alsoData);
      onRemoved(result);
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
      data-testid="delete-database-dialog"
    >
      <div
        className="w-[420px] rounded-xl border border-edge bg-surface p-5"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="text-sm font-semibold text-red-400">
          {t("picker.deleteTitle", { name: database.name })}
        </h2>
        <p className="mt-2 text-xs leading-relaxed text-text-secondary">
          {t("picker.deleteWarning")}
          <span className="mt-1 block font-mono text-[11px] text-text-muted">{database.dbDir}</span>
        </p>

        <label className="mt-3 flex cursor-pointer items-start gap-2 rounded-md border border-red-400/40 bg-red-400/5 p-2.5 text-xs text-text-secondary">
          <input
            type="checkbox"
            checked={alsoData}
            onChange={(e) => setAlsoData(e.target.checked)}
            className="mt-0.5 accent-red-500"
            data-testid="delete-database-data"
          />
          <span>
            {t("picker.deleteAlsoData")}
            <span className="mt-0.5 block font-mono text-[11px] text-red-400">
              {database.dbDir}
            </span>
          </span>
        </label>

        {/* 用户红线：删除数据库绝不删照片库文件夹 */}
        <p className="mt-2 text-[11px] text-amber-300" data-testid="delete-database-never">
          {t("picker.neverDelete")}
        </p>

        <label className="mt-3 flex flex-col gap-1 text-xs text-text-secondary">
          {t("picker.deleteConfirmHint", { name: database.name })}
          <input
            type="text"
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
            className="w-full rounded-md border border-edge bg-bg px-2.5 py-1.5 text-xs text-text-primary outline-none transition-colors focus:border-red-400"
            autoFocus
            data-testid="delete-database-input"
          />
        </label>

        <div className="mt-4 flex justify-end gap-2">
          <button
            type="button"
            onClick={onClose}
            disabled={busy}
            className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent disabled:opacity-40"
            data-testid="delete-database-cancel"
          >
            {t("common.cancel")}
          </button>
          <button
            type="button"
            onClick={() => void confirmRemove()}
            disabled={!nameMatches || busy}
            className="rounded-md bg-red-500 px-3 py-1.5 text-xs font-medium text-white transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="delete-database-confirm"
          >
            {busy ? t("picker.deleting") : t("picker.deleteConfirm")}
          </button>
        </div>
      </div>
      <ErrorModal message={error} onClose={() => setError(null)} />
    </div>
  );
}
