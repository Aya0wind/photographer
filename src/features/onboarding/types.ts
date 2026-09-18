import type { Settings } from "@/stores/settingsStore";

/** 向导草稿：四个步骤收集的全部输入，完成时一次性提交 */
export interface OnboardingDraft {
  // 步骤 1：库设置（达芬奇式库模型，spec §5.11）
  libraryName: string;
  dbDir: string;
  photoRoot: string;
  // 步骤 2：导入方案
  dirTemplate: string;
  duplicatePolicy: Settings["import"]["duplicatePolicy"];
  notifyMilestones: boolean;
  // 步骤 3：AI 三选一
  aiChoice: AiChoice;
}

export type AiChoice = "all" | "semantic" | "none";

/** AI 三选一 → 设置开关映射 */
export const AI_CHOICE_FLAGS: Record<AiChoice, Pick<Settings["ai"], "enableClip" | "enableFace" | "enableSceneTags">> = {
  all: { enableClip: true, enableFace: true, enableSceneTags: true },
  semantic: { enableClip: true, enableFace: false, enableSceneTags: false },
  none: { enableClip: false, enableFace: false, enableSceneTags: false },
};

export const AI_CHOICES: AiChoice[] = ["all", "semantic", "none"];
