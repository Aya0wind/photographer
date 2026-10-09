import { useState } from "react";
import { useTranslation } from "react-i18next";

import { useDatabases } from "@/lib/useDatabases";
import {
  databaseSwitch,
  type DatabaseEntry,
  type DatabaseRemoveResult,
} from "@/ipc/api";
import CreateDatabaseDialog from "./CreateDatabaseDialog";
import RemoveDatabaseDialog from "./RemoveDatabaseDialog";

/** 与设置页 SELECT_CLASS 同款下拉样式（本组件局部声明，避免页↔组件循环依赖） */
const SELECT_CLASS =
  "rounded-md border border-edge bg-bg px-2 py-1 text-xs text-text-primary outline-none transition-colors focus:border-accent disabled:opacity-40";

/**
 * 数据库卡片（2026-10-09 多数据库修正，设置页「常规」tab 顶部）：
 * 当前库名 + 切换下拉（database_switch）+ 新建（CreateDatabaseDialog）+
 * 移除两档确认（RemoveDatabaseDialog）。切换成功后端发 databasesChanged，
 * AppShell 据此全量重挂内容区（画廊/相册/照片库列表重拉——换库语义）。
 * 数据源 useDatabases：挂载拉一次 + databasesChanged 事件重拉。
 */
export default function DatabaseSettings() {
  const { t } = useTranslation();
  const databases = useDatabases();
  const [createOpen, setCreateOpen] = useState(false);
  const [removeTarget, setRemoveTarget] = useState<DatabaseEntry | null>(null);
  const [switching, setSwitching] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const list = databases?.databases ?? [];
  const active =
    list.find((db) => db.id === databases?.activeId) ?? list[0] ?? null;

  async function switchTo(id: string): Promise<void> {
    if (switching || id === databases?.activeId) return;
    setSwitching(true);
    setError(null);
    try {
      await databaseSwitch(id);
      // 成功后 databasesChanged 事件驱动 useDatabases/AppShell 全量刷新
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSwitching(false);
    }
  }

  return (
    <div className="border-b border-edge/40 py-2" data-testid="settings-database-card">
      <div className="text-xs text-text-primary">{t("settings.database.current")}</div>
      <div className="mt-0.5 text-[11px] leading-relaxed text-text-muted">
        {t("settings.database.desc")}
      </div>
      <div className="mt-3 flex flex-wrap items-center gap-2">
        <select
          value={active?.id ?? ""}
          onChange={(event) => void switchTo(event.target.value)}
          disabled={switching || list.length === 0}
          aria-label={t("settings.database.current")}
          className={SELECT_CLASS}
          data-testid="settings-database-select"
        >
          {list.length === 0 && (
            <option value="">{t("settings.database.none")}</option>
          )}
          {list.map((db) => (
            <option key={db.id} value={db.id}>
              {db.name}
            </option>
          ))}
        </select>
        <button
          type="button"
          onClick={() => setCreateOpen(true)}
          className="h-7 rounded-md border border-edge px-3 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
          data-testid="settings-database-new"
        >
          {t("settings.database.new")}
        </button>
        <button
          type="button"
          onClick={() => active !== null && setRemoveTarget(active)}
          disabled={active === null}
          className="h-7 rounded-md border border-edge px-3 text-xs text-text-secondary transition-colors hover:border-red-400/60 hover:text-red-400 disabled:cursor-not-allowed disabled:opacity-40"
          data-testid="settings-database-remove"
        >
          {t("settings.database.remove")}
        </button>
      </div>
      {active !== null && (
        <p
          className="mt-2 break-all font-mono text-[11px] text-text-muted"
          data-testid="settings-database-dir"
        >
          {active.dbDir}
        </p>
      )}
      {error !== null && (
        <p className="mt-2 text-[11px] text-red-400" role="alert" data-testid="settings-database-error">
          {error}
        </p>
      )}

      {createOpen && (
        <CreateDatabaseDialog
          onClose={() => setCreateOpen(false)}
          onCreated={() => setCreateOpen(false)}
        />
      )}
      {removeTarget !== null && (
        <RemoveDatabaseDialog
          database={removeTarget}
          onClose={() => setRemoveTarget(null)}
          onRemoved={(_result: DatabaseRemoveResult) => setRemoveTarget(null)}
        />
      )}
    </div>
  );
}
