import { useTranslation } from "react-i18next";

import { importRootOf, previewTemplate, unknownTokens } from "../onboardingConfig";
import type { OnboardingDraft } from "../types";

interface Props {
  draft: OnboardingDraft;
  onChange: (patch: Partial<OnboardingDraft>) => void;
}

const DUPLICATE_OPTIONS: OnboardingDraft["duplicatePolicy"][] = ["skip", "rename", "ask"];

/** 步骤 2：导入方案——目录模板 + 实时预览 + 查重策略 + 里程碑通知 */
export default function ImportSchemeStep({ draft, onChange }: Props) {
  const { t } = useTranslation();
  const badTokens = unknownTokens(draft.dirTemplate);
  const preview = previewTemplate(draft.dirTemplate, importRootOf(draft.photoRoot));

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-col gap-1.5">
        <label htmlFor="onboarding.scheme.template" className="text-sm font-medium text-text-primary">
          {t("onboarding.scheme.template")}
        </label>
        <input
          id="onboarding.scheme.template"
          type="text"
          value={draft.dirTemplate}
          onChange={(e) => onChange({ dirTemplate: e.target.value })}
          className="w-full rounded-md border border-edge bg-bg px-3 py-2 font-mono text-xs text-text-primary outline-none transition-colors focus:border-accent"
        />
        <p className="text-xs text-text-muted">{t("onboarding.scheme.templateDesc")}</p>
        <p className="text-xs text-text-muted">{t("onboarding.scheme.storageNote")}</p>
        <div className="rounded-md border border-edge bg-bg px-3 py-2 font-mono text-xs text-text-secondary">
          <span className="mr-2 text-text-muted">{t("onboarding.scheme.preview")}</span>
          {preview}
        </div>
        {badTokens.length > 0 && (
          <p className="text-xs text-red-400" role="alert">
            {t("onboarding.scheme.unknownToken", { tokens: badTokens.map((t2) => `{${t2}}`).join(" ") })}
          </p>
        )}
      </div>

      <fieldset className="flex flex-col gap-1.5">
        <legend className="mb-1 text-sm font-medium text-text-primary">
          {t("onboarding.scheme.duplicate")}
        </legend>
        {DUPLICATE_OPTIONS.map((option) => (
          <label key={option} className="flex cursor-pointer items-center gap-2.5 text-sm text-text-secondary">
            <input
              type="radio"
              name="duplicatePolicy"
              value={option}
              checked={draft.duplicatePolicy === option}
              onChange={() => onChange({ duplicatePolicy: option })}
              className="h-3.5 w-3.5 accent-[#F0A83C]"
            />
            {t(`onboarding.scheme.dup.${option}`)}
          </label>
        ))}
      </fieldset>

      <label className="flex cursor-pointer items-center gap-2.5 text-sm text-text-secondary">
        <input
          type="checkbox"
          checked={draft.notifyMilestones}
          onChange={(e) => onChange({ notifyMilestones: e.target.checked })}
          className="h-3.5 w-3.5 accent-[#F0A83C]"
        />
        {t("onboarding.scheme.notify")}
      </label>
    </div>
  );
}
