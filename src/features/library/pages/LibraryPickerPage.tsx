import { useEffect, useState } from "react";
import { useNavigate } from "react-router";
import { useTranslation } from "react-i18next";

import { useSettingsStore, type Library } from "@/stores/settingsStore";

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
    <div className="flex h-full w-full flex-col items-center justify-center gap-8 bg-bg px-8 font-sans text-text-primary">
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
                    {wasActive && (
                      <span className="shrink-0 rounded bg-accent/15 px-1.5 py-0.5 text-[11px] text-accent">
                        {t("picker.lastActive")}
                      </span>
                    )}
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
                      <span
                        className="truncate font-mono text-[11px] text-text-muted"
                        title={lib.dirTemplate}
                      >
                        {lib.dirTemplate}
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
  );
}
