import { useEffect, useState } from "react";
import { useNavigate } from "react-router";
import { useTranslation } from "react-i18next";

import TitleBar from "@/app/shell/TitleBar";
import { useSettingsStore, type Library } from "@/stores/settingsStore";
import { resetLibrarySession } from "@/lib/librarySession";
import DeleteLibraryDialog from "../DeleteLibraryDialog";

/**
 * 库选择器（达芬奇式启动首屏，/library-picker）：
 * 库卡片网格（名称/照片根/数据库目录/模板摘要 + 上次使用标记），当前激活库预选高亮；
 * 单击选中、双击或「打开」进入——configured=true 直进主界面，false 引导向导补完
 * （/onboarding?library=<id>）；「新建库」走 /onboarding 配置链；首启无库直送向导。
 * 打开库 = 前端 set activeLibraryId + save（无新命令）。
 */
export default function LibraryPickerPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const loaded = useSettingsStore((s) => s.loaded);
  const libraries = useSettingsStore((s) => s.settings.libraries);
  const activeLibraryId = useSettingsStore((s) => s.settings.activeLibraryId);

  // 当前激活库预选；列表变化后仍指向有效库
  const [selectedId, setSelectedId] = useState<string | null>(activeLibraryId);
  /** 删除库对话框目标（卡片垃圾桶按钮打开） */
  const [deleting, setDeleting] = useState<Library | null>(null);
  useEffect(() => {
    setSelectedId((prev) =>
      prev !== null && libraries.some((lib) => lib.id === prev)
        ? prev
        : (activeLibraryId ?? libraries[0]?.id ?? null),
    );
  }, [libraries, activeLibraryId]);
  const selected = libraries.find((lib) => lib.id === selectedId) ?? null;

  // 首启无任何库：直接进入新建库配置链（不显示空列表）
  useEffect(() => {
    if (loaded && libraries.length === 0) navigate("/onboarding", { replace: true });
  }, [loaded, libraries.length, navigate]);

  function openLibrary(library: Library): void {
    const { save, setLibraryChosen } = useSettingsStore.getState();
    const settings = useSettingsStore.getState().settings;
    // 切库先清会话态（快照/缩略图缓存/store）——不清=新库看到上一个库的
    // 照片（快照跨库沿用）与张冠李戴的缩略图（assetId 跨库撞号）
    if (settings.activeLibraryId !== library.id) resetLibrarySession();
    void save({ ...settings, activeLibraryId: library.id });
    setLibraryChosen(true);
    // 配置链被中断过的库：先进向导补完（提交时置 configured=true）
    if (library.configured === false) {
      navigate(`/onboarding?library=${encodeURIComponent(library.id)}`);
      return;
    }
    navigate("/gallery", { replace: true });
  }

  if (!loaded) return null;

  return (
    <div className="flex h-full w-full flex-col bg-bg font-sans text-text-primary">
      {/* 无边框窗口：主壳外全屏页也要有自绘标题栏（拖动/最大化/关闭） */}
      <TitleBar />
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-8 overflow-y-auto px-8 py-6">
      <div className="flex flex-col items-center gap-1.5">
        <h1 className="text-2xl font-semibold">{t("picker.title")}</h1>
        <p className="text-sm text-text-muted">{t("picker.subtitle")}</p>
      </div>

      {libraries.length === 0 ? (
        // 兜底空态（首启通常已被 effect 直送向导）
        <div className="flex flex-col items-center gap-4" data-testid="picker-empty">
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
            data-testid="picker-grid"
          >
            {libraries.map((lib) => {
              const isSelected = lib.id === selectedId;
              const wasActive = lib.id === activeLibraryId;
              const unconfigured = lib.configured === false;
              return (
                <div
                  key={lib.id}
                  role="button"
                  tabIndex={0}
                  onClick={() => setSelectedId(lib.id)}
                  onDoubleClick={() => openLibrary(lib)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") openLibrary(lib);
                  }}
                  className={`flex cursor-pointer flex-col gap-2 rounded-xl border-2 bg-surface p-4 text-left transition-colors ${
                    isSelected ? "border-accent" : "border-edge hover:border-text-muted"
                  }`}
                  data-testid="picker-card"
                  data-library-id={lib.id}
                  data-selected={isSelected}
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="truncate text-sm font-semibold text-text-primary" title={lib.name}>
                      {lib.name}
                    </span>
                    <span className="flex shrink-0 items-center gap-1.5">
                      {wasActive && (
                        <span className="rounded bg-accent/15 px-1.5 py-0.5 text-[11px] text-accent">
                          {t("picker.lastActive")}
                        </span>
                      )}
                      <button
                        type="button"
                        aria-label={t("picker.deleteLib")}
                        title={t("picker.deleteLib")}
                        onClick={(e) => {
                          e.stopPropagation();
                          setDeleting(lib);
                        }}
                        className="flex h-6 w-6 items-center justify-center rounded text-text-muted transition-colors hover:bg-red-400/10 hover:text-red-400"
                        data-testid="picker-delete-lib"
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
                    {/* 照片数占位：M4 统计接入前用图片图标位 */}
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
                        title={lib.photoRoot}
                      >
                        {lib.photoRoot}
                      </span>
                      <span className="truncate font-mono text-[11px] text-text-muted" title={lib.dbDir}>
                        {lib.dbDir}
                      </span>
                    </div>
                  </div>
                  {unconfigured && (
                    <span className="self-start rounded bg-yellow-400/15 px-1.5 py-0.5 text-[11px] text-yellow-300">
                      {t("picker.unconfigured")}
                    </span>
                  )}
                </div>
              );
            })}
          </div>
          <div className="flex items-center gap-3">
            <button
              type="button"
              disabled={!selected}
              onClick={() => selected && openLibrary(selected)}
              className="rounded-md bg-accent px-5 py-2 text-sm font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
              data-testid="picker-open"
            >
              {t("picker.open")}
            </button>
            <button
              type="button"
              onClick={() => navigate("/onboarding")}
              className="rounded-md border border-edge px-5 py-2 text-sm text-text-secondary transition-colors hover:border-accent hover:text-accent"
              data-testid="picker-create"
            >
              {t("picker.newLibrary")}
            </button>
          </div>
        </>
      )}
      </div>
      {deleting !== null && (
        <DeleteLibraryDialog
          library={deleting}
          onClose={() => setDeleting(null)}
          onDeleted={() => setDeleting(null)}
        />
      )}
    </div>
  );
}
