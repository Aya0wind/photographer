import { useState } from "react";
import { Navigate, useNavigate } from "react-router";
import { AnimatePresence, motion } from "motion/react";
import { useTranslation } from "react-i18next";

import AiStep from "../steps/AiStep";
import DoneStep from "../steps/DoneStep";
import ImportSchemeStep from "../steps/ImportSchemeStep";
import LibraryStep from "../steps/LibraryStep";
import {
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

function initialDraft(): OnboardingDraft {
  return {
    libraryName: SUGGESTED_LIBRARY_NAME,
    dbDir: SUGGESTED_DB_DIR,
    photoRoot: SUGGESTED_PHOTO_ROOT,
    dirTemplate: "{YYYY}/{MM-DD}/{原文件名}",
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

/** 首次引导向导：库设置 → 导入方案 → AI 三选一 → 确认。完成即写入设置并进入主界面。 */
export default function OnboardingPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const loaded = useSettingsStore((s) => s.loaded);
  const completed = useSettingsStore((s) => s.settings.onboardingCompleted);

  const [step, setStep] = useState(0);
  const [draft, setDraft] = useState<OnboardingDraft>(initialDraft);

  // 已完成引导的用户再访问 /onboarding：直接回主界面
  if (loaded && completed) return <Navigate to="/gallery" replace />;

  const patch = (p: Partial<OnboardingDraft>) => setDraft((d) => ({ ...d, ...p }));

  const commit = async () => {
    const current = useSettingsStore.getState().settings;
    const libraryId =
      typeof crypto !== "undefined" && "randomUUID" in crypto
        ? crypto.randomUUID()
        : `lib-${Date.now().toString(36)}`;
    const library: Library = {
      id: libraryId,
      name: draft.libraryName.trim(),
      dbDir: draft.dbDir.trim(),
      photoRoot: draft.photoRoot.trim(),
    };
    await useSettingsStore.getState().save({
      ...current,
      onboardingCompleted: true,
      libraries: [...current.libraries, library],
      activeLibraryId: library.id,
      import: {
        ...current.import,
        dirTemplate: draft.dirTemplate.trim(),
        duplicatePolicy: draft.duplicatePolicy,
        notifyMilestones: draft.notifyMilestones,
      },
      ai: { ...current.ai, ...AI_CHOICE_FLAGS[draft.aiChoice] },
    });
    navigate("/gallery", { replace: true });
  };

  const isLast = step === STEP_TITLES.length - 1;

  return (
    <div className="flex h-full w-full items-center justify-center bg-bg font-sans text-text-primary">
      <div className="flex w-full max-w-xl flex-col gap-6 px-8">
        {/* 标题与步骤指示 */}
        <div className="flex flex-col gap-3">
          <h1 className="text-xl font-semibold">{t("onboarding.title")}</h1>
          <div className="flex items-center gap-2" role="tablist" aria-label="onboarding steps">
            {STEP_TITLES.map((titleKey, i) => (
              <div key={titleKey} className="flex items-center gap-2">
                <span
                  className={`h-1.5 w-8 rounded-full transition-colors ${
                    i <= step ? "bg-accent" : "bg-panel"
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
        <div className="rounded-xl border border-panel bg-surface p-6">
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
  );
}
