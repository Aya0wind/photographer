import { useState } from "react";
import { useTranslation } from "react-i18next";

import { databaseRemove, type DatabaseEntry, type DatabaseRemoveResult } from "@/ipc/api";

/**
 * 移除数据库确认（2026-10-09 多数据库修正，设置页数据库卡片入口）：
 * 两级——仅摘注册表登记（数据目录原样留在磁盘）或连数据目录整棵删
 *（library.db/thumbs/向量）。**绝不删除照片库文件夹**（用户红线，后端
 * database_remove 另有照片库根重叠安全闸双保险）；是否删激活库后的回退
 * 切换由后端处理。失败文案透传内联展示；成功后 onRemoved 回调刷新列表。
 */
export default function RemoveDatabaseDialog({
  database,
  onClose,
  onRemoved,
}: {
  database: DatabaseEntry;
  onClose: () => void;
  /** 移除成功回调（参数=命令结果，含实际删除的数据目录数） */
  onRemoved: (result: DatabaseRemoveResult) => void;
}) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function remove(deleteData: boolean): Promise<void> {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const result = await databaseRemove(database.id, deleteData);
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
      data-testid="database-remove-dialog"
    >
      <div className="w-full max-w-md rounded-xl border border-edge bg-surface p-4 shadow-2xl">
        <h2 className="text-sm font-semibold text-text-primary" data-testid="database-remove-title">
          {t("settings.dbRemoveDialog.title", { name: database.name })}
        </h2>
        <p className="mt-1 break-all font-mono text-[11px] text-text-muted">{database.dbDir}</p>
        <p className="mt-2 text-xs leading-relaxed text-text-secondary">
          {t("settings.dbRemoveDialog.desc")}
        </p>
        {/* 用户红线：移除数据库绝不删照片库文件夹 */}
        <p className="mt-1 text-xs text-amber-300" data-testid="database-remove-never-delete">
          {t("settings.dbRemoveDialog.neverDelete")}
        </p>

        {error !== null && (
          <p className="mt-2 text-[11px] text-red-400" role="alert" data-testid="database-remove-error">
            {error}
          </p>
        )}

        <div className="mt-4 flex flex-col gap-2">
          <button
            type="button"
            disabled={busy}
            onClick={() => void remove(false)}
            className="rounded-md border border-edge px-3 py-2 text-xs text-text-secondary transition-colors hover:bg-panel hover:text-text-primary disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="database-remove-keep-data"
          >
            {t("settings.dbRemoveDialog.keep")}
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() => void remove(true)}
            className="rounded-md border border-red-400/60 px-3 py-2 text-xs text-red-400 transition-colors hover:bg-red-400/10 disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="database-remove-delete-data"
          >
            {t("settings.dbRemoveDialog.delete")}
          </button>
          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-edge px-3 py-2 text-xs text-text-secondary transition-colors hover:bg-panel hover:text-text-primary"
            data-testid="database-remove-cancel"
          >
            {t("common.cancel")}
          </button>
        </div>
      </div>
    </div>
  );
}
