import { useState } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { AnimatePresence, motion } from "motion/react";
import { useTranslation } from "react-i18next";

import TitleBar from "@/app/shell/TitleBar";
import {
  clearDraftLibraryId,
  readDraftLibraryId,
} from "@/features/library/NewLibraryDialog";
import AiStep from "../steps/AiStep";
import DoneStep from "../steps/DoneStep";
import ImportSchemeStep from "../steps/ImportSchemeStep";
import LibraryStep from "../steps/LibraryStep";
import {
  DEFAULT_IMPORT_SUBDIR,
  SUGGESTED_DB_DIR,
  SUGGESTED_LIBRARY_NAME,
  SUGGESTED_PHOTO_ROOT,
} from "../onboardingConfig";
import { AI_CHOICE_FLAGS, type OnboardingDraft } from "../types";
import { useSettingsStore, type Library } from "@/stores/settingsStore";
import { resetLibrarySession } from "@/lib/librarySession";

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

/** 新建库草稿：本机默认值（名称/目录预填，可改）；目录布局固定不再收集 */
function newLibraryDraft(): OnboardingDraft {
  return {
    libraryName: SUGGESTED_LIBRARY_NAME,
    dbDir: SUGGESTED_DB_DIR,
    photoRoot: SUGGESTED_PHOTO_ROOT,
    importSubdir: DEFAULT_IMPORT_SUBDIR,
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
    // 步骤 2 只收集查重策略/通知偏好，无必填校验（目录布局已固定）
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
    // 并发流数是库属性：补完模式保留库既有值（NewLibraryDialog 新建的带过来），新建默认 4
    const existing = editId
      ? current.libraries.find((lib) => lib.id === editId) ?? undefined
      : undefined;
    const library: Library = {
      id: editId ?? makeLibraryId(),
      name: draft.libraryName.trim(),
      dbDir: draft.dbDir.trim(),
      photoRoot: draft.photoRoot.trim(),
      importSubdir: draft.importSubdir.trim(),
      streams: existing?.streams ?? 4,
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
      // 全局 ImportSettings 仅作后续新建库的默认值（目录布局固定，不再写模板）
      import: {
        ...current.import,
        duplicatePolicy: draft.duplicatePolicy,
        notifyMilestones: draft.notifyMilestones,
      },
      ai: { ...current.ai, ...AI_CHOICE_FLAGS[draft.aiChoice] },
    });
    useSettingsStore.getState().setLibraryChosen(true);
    // 新库启用：清上一个库的会话态（快照/缩略图缓存/store）
    resetLibrarySession();
    // 配置完成：清除「本次会话新建」标记（该库已不再是可删空库）
    clearDraftLibraryId();
    navigate("/gallery", { replace: true });
  };

  /**
   * 取消（退出新建库补完流）：
   * - 本次会话新建（sessionStorage 标记匹配 ?library=<id>）且仍未配置 →
   *   该空库无任何资产，直接从 libraries 移除并 save，激活回落到其他已配置库；
   * - 存量未配置库（从选择器点进来的老库）→ 保留不动，仅退出。
   * 导航：settings.activeLibraryId 指向某个已配置库 → /gallery；否则回 /library-picker。
   */
  const cancel = async () => {
    const draftId = readDraftLibraryId();
    const current = useSettingsStore.getState().settings;
    if (editId !== null && draftId === editId) {
      const target = current.libraries.find((lib) => lib.id === editId);
      // 防呆：仅删「仍未配置完成」的本次新建空库；已完成配置的库不删
      if (target && target.configured === false) {
        const libraries = current.libraries.filter((lib) => lib.id !== editId);
        const restore = libraries.find((lib) => lib.configured) ?? null;
        await useSettingsStore.getState().save({
          ...current,
          libraries,
          activeLibraryId: restore ? restore.id : null,
        });
      }
      clearDraftLibraryId();
    }
    const after = useSettingsStore.getState().settings;
    const activeConfigured =
      after.activeLibraryId !== null &&
      after.libraries.some((lib) => lib.id === after.activeLibraryId && lib.configured);
    if (activeConfigured) {
      navigate("/gallery", { replace: true });
    } else {
      // 没有可用的已配置激活库：回选择器重选（会话选库标志一并复位）
      useSettingsStore.getState().setLibraryChosen(false);
      navigate("/library-picker", { replace: true });
    }
  };

  const isLast = step === STEP_TITLES.length - 1;

  return (
    <div className="flex h-full w-full flex-col bg-bg font-sans text-text-primary">
      {/* 无边框窗口：主壳外全屏页也要有自绘标题栏（拖动/最大化/关闭） */}
      <TitleBar />
      <div className="flex min-h-0 flex-1 items-center justify-center overflow-y-auto px-8 py-6">
        {/* 固定尺寸框架：不同步骤高度/标题位置一致；内容超高在卡内滚动 */}
        <div className="flex h-[560px] w-full max-w-xl flex-col">
        {/* 标题与步骤指示（钉在顶部，不随步骤内容移动） */}
        <div className="flex shrink-0 flex-col gap-3 pb-4">
          <h1 className="text-xl font-semibold">{t("onboarding.title")}</h1>
          <div className="flex items-center gap-2" role="tablist" aria-label="onboarding steps">
            {STEP_TITLES.map((titleKey, i) => {
              // 已完成的步骤可点击直接跳回（草稿保留在内存）；当前/未来步不可点
              const reachable = i < step;
              return (
                <button
                  key={titleKey}
                  type="button"
                  disabled={!reachable}
                  onClick={() => reachable && setStep(i)}
                  aria-current={i === step ? "step" : undefined}
                  className={`flex items-center gap-2 rounded px-0.5 py-0.5 transition-colors ${
                    reachable ? "cursor-pointer" : "cursor-default"
                  }`}
                  data-testid={`onboarding-step-${i}`}
                >
                  {/* 当前步=accent 实心；已完成=accent/60 弱化；未来=panel 底 */}
                  <span
                    className={`h-1.5 w-8 rounded-full transition-colors ${
                      i === step ? "bg-accent" : i < step ? "bg-accent/60" : "bg-panel"
                    }`}
                  />
                  <span
                    className={`text-xs transition-colors ${
                      i === step
                        ? "text-accent"
                        : reachable
                          ? "text-text-secondary hover:text-accent"
                          : "text-text-muted"
                    }`}
                  >
                    {t(titleKey)}
                  </span>
                </button>
              );
            })}
          </div>
        </div>

        {/* 步骤内容：固定框架内的滚动区 */}
        <div className="sp-scroll min-h-0 flex-1 overflow-y-auto rounded-xl border border-edge bg-surface p-6">
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

        {/* 底部操作条（钉在底部）：[取消] …… [上一步] [下一步/开始使用] */}
        <div className="flex shrink-0 items-center justify-between pt-4">
          <button
            type="button"
            onClick={() => void cancel()}
            className="rounded-md px-4 py-2 text-sm text-text-muted transition-colors hover:text-text-primary"
            data-testid="onboarding-cancel"
          >
            {t("common.cancel")}
          </button>
          <div className="flex items-center gap-1">
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
    </div>
  );
}
