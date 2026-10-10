import { useEffect, useState } from "react";
import { useNavigate, useLocation } from "react-router";
import { isMacPlatform } from "@/lib/platform";
import { useTranslation } from "react-i18next";

import ErrorModal from "@/shared/components/ErrorModal";
import TitleBar from "@/app/shell/TitleBar";
import { useSettingsStore } from "@/stores/settingsStore";
import { useWindowReveal } from "@/lib/windowReveal";
import { resetLibrarySession } from "@/lib/librarySession";
import { useDatabases } from "@/lib/useDatabases";
import { databaseSwitch, type DatabaseEntry } from "@/ipc/api";
import RemoveDatabaseDialog from "../components/RemoveDatabaseDialog";

/**
 * 数据库选择器（达芬奇式启动首屏，/database-picker；老 LibraryPickerPage
 * 的 UI 结构与交互直系）：数据库卡片网格（名称/dbDir + 上次使用标记），
 * 当前激活库预选高亮；单击选中、双击或「打开」进入；「新建数据库」走
 * onboarding；首启无库直送向导。
 * 打开数据库 = database_switch（后端校验 + 广播 databasesChanged，AppShell
 * 据此全量重挂内容区）；切换前清会话态（快照/缩略图缓存/store，防跨库
 * 张冠李戴——快照跨库沿用=看到上一个库的照片，assetId 撞号=错的缩略图）。
 */
export default function DatabasePickerPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const location=useLocation();
  const preferredId=useSettingsStore(s=>s.settings.system.autoOpenDatabaseId);
  const [autoOpen,setAutoOpen]=useState(false);
  const loaded = useSettingsStore((s) => s.loaded);
  useWindowReveal(loaded);
  const databases = useDatabases();
  const list = databases?.databases ?? [];
  const activeId = databases?.activeId ?? null;

  // 当前激活库预选；列表变化后仍指向有效库
  const [selectedId, setSelectedId] = useState<string | null>(activeId);
  /** 删除数据库对话框目标（卡片垃圾桶按钮打开） */
  const [deleting, setDeleting] = useState<DatabaseEntry | null>(null);
  const [opening, setOpening] = useState(false);
  const [error, setError] = useState<string | null>((location.state as {autoOpenError?:string}|null)?.autoOpenError ?? null);
  useEffect(()=>setAutoOpen(!!selectedId && selectedId===preferredId),[selectedId,preferredId]);
  useEffect(() => {
    setSelectedId((prev) =>
      prev !== null && list.some((db) => db.id === prev)
        ? prev
        : (activeId ?? list[0]?.id ?? null),
    );
  }, [list, activeId]);
  const selected = list.find((db) => db.id === selectedId) ?? null;

  // 首启无任何数据库：直接进入新建数据库引导链（不显示空列表）
  useEffect(() => {
    if (loaded && databases !== null && databases.databases.length === 0) {
      navigate("/onboarding", { replace: true });
    }
  }, [loaded, databases, navigate]);

  async function openDatabase(database: DatabaseEntry): Promise<void> {
    if (opening) return;
    setOpening(true);
    setError(null);
    // 切库先清会话态（快照/缩略图缓存/store）——不清=新库看到上一个库的
    // 照片（快照跨库沿用）与张冠李戴的缩略图（assetId 跨库撞号）
    if (activeId !== database.id) resetLibrarySession();
    try {
      // Save against the current database before switching: AI settings are
      // database-scoped and must not be copied from the old store into the new DB.
      const current=useSettingsStore.getState().settings;
      const nextId=autoOpen ? database.id : null;
      if((current.system.autoOpenDatabaseId??null)!==nextId) {
        await useSettingsStore.getState().save({...current,system:{...current.system,autoOpenDatabaseId:nextId}});
      }
      await databaseSwitch(database.id);
      useSettingsStore.getState().setDatabaseChosen(true);
      navigate("/gallery", { replace: true });
    } catch (e) {
      // 失败文案（数据库不存在/不可用/导入进行中）透传展示，留在选择页
      setError(e instanceof Error ? e.message : String(e));
      setOpening(false);
    }
  }

  if (!loaded || databases === null) return null;

  return (
    <div className={`flex h-full w-full flex-col bg-bg font-sans text-text-primary ${isMacPlatform() ? "mac-window-content" : ""}`}>
      {/* 无边框窗口：主壳外全屏页也要有自绘标题栏（拖动/最大化/关闭） */}
      <TitleBar />
      <div className="sp-scroll flex min-h-0 flex-1 flex-col items-center justify-center gap-8 overflow-y-auto px-8 py-6">
      <div className="flex flex-col items-center gap-1.5">
        <h1 className="text-2xl font-semibold">{t("picker.title")}</h1>
        <p className="text-sm text-text-muted">{t("picker.subtitle")}</p>
      </div>

      {list.length === 0 ? (
        // 兜底空态（首启通常已被 effect 直送向导）
        <div className="flex flex-col items-center gap-4" data-testid="database-picker-empty">
          <p className="text-base text-text-secondary">{t("picker.emptyTitle")}</p>
          <button
            type="button"
            onClick={() => navigate("/onboarding")}
            className="rounded-md bg-accent px-5 py-2 text-sm font-medium text-black transition-colors hover:brightness-110"
          >
            {t("picker.createFirst")}
          </button>
        </div>
      ) : (
        <>
          <div
            className="grid w-full max-w-3xl grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-4"
            data-testid="database-picker-grid"
          >
            {list.map((db) => {
              const isSelected = db.id === selectedId;
              const wasActive = db.id === activeId;
              return (
                <div
                  key={db.id}
                  role="button"
                  tabIndex={0}
                  onClick={() => setSelectedId(db.id)}
                  onDoubleClick={() => void openDatabase(db)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") void openDatabase(db);
                  }}
                  className={`flex cursor-pointer flex-col gap-2 rounded-xl border-2 bg-surface p-4 text-left transition-colors ${
                    isSelected ? "border-accent" : "border-edge hover:border-text-muted"
                  }`}
                  data-testid="database-picker-card"
                  data-db-id={db.id}
                  data-selected={isSelected}
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="truncate text-sm font-semibold text-text-primary" title={db.name}>
                      {db.name}
                    </span>
                    <span className="flex shrink-0 items-center gap-1.5">
                      {wasActive && (
                        <span className="rounded bg-accent/15 px-1.5 py-0.5 text-[11px] text-accent">
                          {t("picker.lastActive")}
                        </span>
                      )}
                      <button
                        type="button"
                        aria-label={t("picker.deleteDatabase")}
                        title={t("picker.deleteDatabase")}
                        onClick={(e) => {
                          e.stopPropagation();
                          setDeleting(db);
                        }}
                        className="flex h-6 w-6 items-center justify-center rounded text-text-muted transition-colors hover:bg-red-400/10 hover:text-red-400"
                        data-testid="database-picker-delete"
                      >
                        <svg
                          viewBox="0 0 24 24"
                          width="13"
                          height="13"
                          fill="none"
                          stroke="currentColor"
                          strokeWidth="1.6"
                          strokeLinecap="round"
                          strokeLinejoin="round"
                          aria-hidden="true"
                        >
                          <path d="M4 7h16M9 7V5a1 1 0 011-1h4a1 1 0 011 1v2m-9 0l1 13a1 1 0 001 1h8a1 1 0 001-1l1-13" />
                        </svg>
                      </button>
                    </span>
                  </div>
                  <div className="flex items-start gap-2.5">
                    {/* 照片数占位：统计接入前用图片图标位 */}
                    <span
                      className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-panel text-text-muted"
                      aria-hidden="true"
                    >
                      <svg
                        viewBox="0 0 24 24"
                        width="20"
                        height="20"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="1.4"
                        strokeLinecap="round"
                        strokeLinejoin="round"
                        aria-hidden="true"
                      >
                        <rect x="3.5" y="4.5" width="17" height="15" rx="2" />
                        <circle cx="9" cy="10" r="1.8" />
                        <path d="M4.5 17l4.5-4.5 3.5 3.5 3-3 4 4" />
                      </svg>
                    </span>
                    <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                      <span
                        className="truncate font-mono text-[11px] text-text-secondary"
                        title={db.dbDir}
                      >
                        {db.dbDir}
                      </span>
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
          <div className="flex flex-col items-center gap-3">
            <label className="flex cursor-pointer items-center gap-2 text-sm text-text-secondary">
              <input type="checkbox" className="accent-accent" checked={autoOpen} disabled={!selected || opening}
                onChange={event=>setAutoOpen(event.target.checked)} />
              {t("picker.autoOpen")}
            </label>
            <div className="flex items-center gap-3">
              <button
                type="button"
                disabled={!selected || opening}
                onClick={() => selected && void openDatabase(selected)}
                className="rounded-md bg-accent px-5 py-2 text-sm font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
                data-testid="database-picker-open"
              >
                {t("picker.open")}
              </button>
              <button
                type="button"
                onClick={() => navigate("/onboarding")}
                className="rounded-md border border-edge px-5 py-2 text-sm text-text-secondary transition-colors hover:border-accent hover:text-accent"
                data-testid="database-picker-create"
              >
                {t("picker.newDatabase")}
              </button>
            </div>

          </div>
        </>
      )}
      </div>
      <ErrorModal message={error} onClose={()=>setError(null)} />
      {deleting !== null && (
        <RemoveDatabaseDialog
          database={deleting}
          onClose={() => setDeleting(null)}
          onRemoved={() => setDeleting(null)}
        />
      )}
    </div>
  );
}
