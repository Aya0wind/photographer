import { useState } from "react";
import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import { databaseCreate, type DatabaseEntry } from "@/ipc/api";

/**
 * 新建数据库对话框（2026-10-09 多数据库修正，设置页数据库卡片入口）：
 * 名称必填 + 位置可选（缺省 = 后端约定路径 `<应用配置目录>/databases/<id>`）。
 * 首个库自动激活；注册表内 db_dir 互斥与照片库 root 重叠校验在后端
 * （ipc/databases.rs），业务错误透传内联展示；invoke 不可用提示后端未连接。
 */

const FIELD_CLASS =
  "w-full rounded-md border border-edge bg-bg px-2.5 py-1.5 font-mono text-xs text-text-primary outline-none transition-colors focus:border-accent";

/** 从绝对路径取末段作缺省名（Windows/Unix 分隔符都认；取不到回退全路径） */
function baseName(path: string): string {
  const parts = path.split(/[\\/]+/).filter((s) => s.length > 0);
  return parts.length > 0 ? parts[parts.length - 1] : path;
}

export default function CreateDatabaseDialog({
  onClose,
  onCreated,
}: {
  onClose: () => void;
  /** 创建成功回调（卡片刷新列表；注册表变更另发 databasesChanged） */
  onCreated: (database: DatabaseEntry) => void;
}) {
  const { t } = useTranslation();
  const [name, setName] = useState("");
  const [dbDir, setDbDir] = useState("");
  /** 名称未被手改过时，选完文件夹自动带出末段作缺省名 */
  const [nameTouched, setNameTouched] = useState(false);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const ready = name.trim().length > 0 && !creating;

  async function pickDirectory(): Promise<void> {
    try {
      const dir = await openDialog({
        directory: true,
        defaultPath: dbDir.trim() || undefined,
      });
      if (typeof dir === "string" && dir.length > 0) {
        setDbDir(dir);
        if (!nameTouched && name.trim().length === 0) setName(baseName(dir));
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function create(): Promise<void> {
    if (!ready) return;
    setCreating(true);
    setError(null);
    const result = await databaseCreate(name.trim(), dbDir.trim() || undefined);
    setCreating(false);
    if (result.ok) {
      onCreated(result.database);
      return;
    }
    // null=invoke 不可用（后端未连接）；字符串=后端业务 Err 文案透传
    setError(
      result.error === null ? t("settings.dbCreateDialog.unavailable") : result.error,
    );
  }

  return (
    <div
      className="fixed inset-0 z-[70] flex items-center justify-center bg-black/60"
      onClick={onClose}
      role="dialog"
      aria-modal="true"
      data-testid="database-create-dialog"
    >
      <div
        className="w-[460px] rounded-xl border border-edge bg-surface p-5"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="text-sm font-semibold text-text-primary">
          {t("settings.dbCreateDialog.title")}
        </h2>
        <p className="mt-2 text-xs leading-relaxed text-text-secondary">
          {t("settings.dbCreateDialog.hint")}
        </p>

        <label className="mt-3 flex flex-col gap-1 text-xs text-text-secondary">
          {t("settings.dbCreateDialog.name")}
          <input
            type="text"
            value={name}
            onChange={(e) => {
              setName(e.target.value);
              setNameTouched(true);
            }}
            className={FIELD_CLASS}
            autoFocus
            data-testid="database-create-name"
          />
        </label>

        <label className="mt-3 flex flex-col gap-1 text-xs text-text-secondary">
          {t("settings.dbCreateDialog.location")}
          <div className="flex gap-2">
            <input
              type="text"
              value={dbDir}
              onChange={(e) => setDbDir(e.target.value)}
              className={FIELD_CLASS}
              data-testid="database-create-location"
            />
            <button
              type="button"
              onClick={() => void pickDirectory()}
              className="shrink-0 rounded-md border border-edge px-3 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
              data-testid="database-create-browse"
            >
              {t("settings.dbCreateDialog.browse")}
            </button>
          </div>
          <span className="text-[11px] leading-relaxed text-text-muted">
            {t("settings.dbCreateDialog.locationHint")}
          </span>
        </label>

        {error !== null && (
          <p className="mt-2 text-[11px] text-red-400" role="alert" data-testid="database-create-error">
            {error}
          </p>
        )}

        <div className="mt-4 flex justify-end gap-2">
          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
            data-testid="database-create-cancel"
          >
            {t("common.cancel")}
          </button>
          <button
            type="button"
            disabled={!ready}
            onClick={() => void create()}
            className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="database-create-confirm"
          >
            {creating ? t("settings.dbCreateDialog.creating") : t("settings.dbCreateDialog.confirm")}
          </button>
        </div>
      </div>
    </div>
  );
}
