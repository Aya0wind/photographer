import { useState } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { AnimatePresence, motion } from "motion/react";
import { useTranslation } from "react-i18next";

import TitleBar from "@/app/shell/TitleBar";
import AiStep from "../steps/AiStep";
import DoneStep from "../steps/DoneStep";
import ImportSchemeStep from "../steps/ImportSchemeStep";
import LibraryStep from "../steps/LibraryStep";
import {
  DEFAULT_IMPORT_SUBDIR,
  SUGGESTED_DB_DIR,
  SUGGESTED_LIBRARY_NAME,
  SUGGESTED_PHOTO_ROOT,
  unknownTokens,
} from "../onboardingConfig";
import { AI_CHOICE_FLAGS, type OnboardingDraft } from "../types";
import { useSettingsStore, type Library } from "@/stores/settingsStore";

const STEP_TITLES = [
  "onboarding.step.library",
  "onboarding.step.scheme",
  "onboarding.step.ai",
  "onboarding.step.done",
] as const;

function makeLibraryId(): string {
  return typeof crypto !== "undefined" && "randomUUID" in crypto
    ? crypto.randomUUID()
    : `lib-${Date.now().toString(36)}`;
}

/** 新建库草稿：本机默认值（名称/目录预填，可改） */
function newLibraryDraft(): OnboardingDraft {
  return {
    libraryName: SUGGESTED_LIBRARY_NAME,
    dbDir: SUGGESTED_DB_DIR,
    photoRoot: SUGGESTED_PHOTO_ROOT,
    importSubdir: DEFAULT_IMPORT_SUBDIR,
    dirTemplate: "{YYYY}/{MM-DD}/{原文件名}",
    duplicatePolicy: "skip",
    notifyMilestones: true,
    aiChoice: "all",
  };
}

/** 补完未配置库的草稿：预填既有库字段（?library=<id> 进入） */
function draftFromLibrary(library: Library): OnboardingDraft {
  return {
    libraryName: library.name,
    dbDir: library.dbDir,
    photoRoot: library.photoRoot,
    importSubdir: library.importSubdir || DEFAULT_IMPORT_SUBDIR,
    dirTemplate: library.dirTemplate || "{YYYY}/{MM-DD}/{原文件名}",
    duplicatePolicy: "skip",
    notifyMilestones: true,
    aiChoice: "all",
  };
}

function canProceed(step: number, draft: OnboardingDraft): boolean {
  switch (step) {
    case 0:
      return (
        draft.libraryName.trim().length > 0 &&
        draft.dbDir.trim().length > 0 &&
        draft.photoRoot.trim().length > 0
      );
    case 1:
      return draft.dirTemplate.trim().length > 0 && unknownTokens(draft.dirTemplate).length === 0;
    case 2:
      return true;
    default:
      return true;
  }
}

/**
 * 新建库配置链（达芬奇式启动流）：/library-picker「新建库」或未配置库补完（?library=<id>）
 * 进入。四步：库位置（含导入子目录）→ 库的整理规则 → AI → 确认。
 * 完成写 Library{...,configured:true} + activeLibraryId（onboardingCompleted 仅兼容保留）。
 */
export default function OnboardingPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const loaded = useSettingsStore((s) => s.loaded);

  // 补完模式：URL ?library=<id> 指向既有库；无效/缺失时按新建处理
  const editId = searchParams.get("library");
  const [step, setStep] = useState(0);
  const [draft, setDraft] = useState<OnboardingDraft>(() => {
    if (!editId) return newLibraryDraft();
    const library = useSettingsStore
      .getState()
      .settings.libraries.find((lib) => lib.id === editId);
    return library ? draftFromLibrary(library) : newLibraryDraft();
  });

  // 设置未加载完成前等待（补完模式需要既有库数据；避免用默认值覆盖真实设置）
  if (!loaded) return null;

  const patch = (p: Partial<OnboardingDraft>) => setDraft((d) => ({ ...d, ...p }));

  const commit = async () => {
    const current = useSettingsStore.getState().settings;
    const library: Library = {
      id: editId ?? makeLibraryId(),
      name: draft.libraryName.trim(),
      dbDir: draft.dbDir.trim(),
      photoRoot: draft.photoRoot.trim(),
      dirTemplate: draft.dirTemplate.trim(),
      importSubdir: draft.importSubdir.trim(),
      configured: true,
    };
    const libraries = editId
      ? current.libraries.map((lib) => (lib.id === editId ? library : lib))
      : [...current.libraries, library];
    await useSettingsStore.getState().save({
      ...current,
      // 兼容保留：后端旧迁移逻辑可能仍读该字段；不再作前端门禁
      onboardingCompleted: true,
      libraries,
      activeLibraryId: library.id,
      // 全局 ImportSettings 仅作后续新建库的默认值
      import: {
        ...current.import,
        dirTemplate: draft.dirTemplate.trim(),
        duplicatePolicy: draft.duplicatePolicy,
        notifyMilestones: draft.notifyMilestones,
      },
      ai: { ...current.ai, ...AI_CHOICE_FLAGS[draft.aiChoice] },
    });
    useSettingsStore.getState().setLibraryChosen(true);
    navigate("/gallery", { replace: true });
  };

  const isLast = step === STEP_TITLES.length - 1;

  return (
    <div className="flex h-full w-full flex-col bg-bg font-sans text-text-primary">
      {/* 无边框窗口：主壳外全屏页也要有自绘标题栏（拖动/最大化/关闭） */}
      <TitleBar />
      <div className="flex min-h-0 flex-1 items-center justify-center overflow-y-auto px-8 py-6">
        <div className="flex w-full max-w-xl flex-col gap-6">
        {/* 标题与步骤指示 */}
        <div className="flex flex-col gap-3">
          <h1 className="text-xl font-semibold">{t("onboarding.title")}</h1>
          <div className="flex items-center gap-2" role="tablist" aria-label="onboarding steps">
            {STEP_TITLES.map((titleKey, i) => (
              <div key={titleKey} className="flex items-center gap-2">
                {/* 当前步=accent 实心；已完成=accent/60 弱化；未来=panel 底 */}
                <span
                  className={`h-1.5 w-8 rounded-full transition-colors ${
                    i === step ? "bg-accent" : i < step ? "bg-accent/60" : "bg-panel"
                  }`}
                />
                <span className={`text-xs ${i === step ? "text-accent" : "text-text-muted"}`}>
                  {t(titleKey)}
                </span>
              </div>
            ))}
          </div>
        </div>

        {/* 步骤内容 */}
        <div className="rounded-xl border border-edge bg-surface p-6">
          <AnimatePresence mode="wait" initial={false}>
            <motion.div
              key={step}
              initial={{ opacity: 0, x: 12 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -12 }}
              transition={{ duration: 0.18, ease: "easeOut" }}
            >
              {step === 0 && <LibraryStep draft={draft} onChange={patch} />}
              {step === 1 && <ImportSchemeStep draft={draft} onChange={patch} />}
              {step === 2 && <AiStep value={draft.aiChoice} onChange={(c) => patch({ aiChoice: c })} />}
              {step === 3 && <DoneStep draft={draft} />}
            </motion.div>
          </AnimatePresence>
        </div>

        {/* 底部操作条 */}
        <div className="flex items-center justify-between">
          <button
            type="button"
            onClick={() => setStep((s) => Math.max(0, s - 1))}
            disabled={step === 0}
            className="rounded-md px-4 py-2 text-sm text-text-secondary transition-colors hover:text-text-primary disabled:invisible"
          >
            {t("common.back")}
          </button>
          {isLast ? (
            <button
              type="button"
              onClick={() => void commit()}
              className="rounded-md bg-accent px-5 py-2 text-sm font-medium text-bg transition-opacity hover:opacity-90"
            >
              {t("onboarding.done.start")}
            </button>
          ) : (
            <button
              type="button"
              onClick={() => setStep((s) => s + 1)}
              disabled={!canProceed(step, draft)}
              className="rounded-md bg-accent px-5 py-2 text-sm font-medium text-bg transition-opacity hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-40"
            >
              {t("common.next")}
            </button>
          )}
        </div>
        </div>
      </div>
    </div>
  );
}
