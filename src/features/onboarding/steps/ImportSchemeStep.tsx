import { useTranslation } from "react-i18next";

import { FIXED_ALBUM_LAYOUT } from "../onboardingConfig";
import type { OnboardingDraft } from "../types";

interface Props {
  draft: OnboardingDraft;
  onChange: (patch: Partial<OnboardingDraft>) => void;
}

const DUPLICATE_OPTIONS: OnboardingDraft["duplicatePolicy"][] = ["skip", "rename", "ask"];

/** 步骤 2：整理规则——目录布局固定（时间/相册+平铺，2026-09-28 定案不再可配置），
 *  仅收集查重策略与里程碑通知偏好 */
export default function ImportSchemeStep({ draft, onChange }: Props) {
  const { t } = useTranslation();

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-col gap-1.5">
        <p className="text-sm font-medium text-text-primary">{t("onboarding.scheme.layout")}</p>
        <div className="rounded-md border border-edge bg-bg px-3 py-2 font-mono text-xs text-text-secondary">
          {FIXED_ALBUM_LAYOUT}
        </div>
        <p className="text-xs text-text-muted">{t("onboarding.scheme.layoutNote")}</p>
        <p className="text-xs text-text-muted">{t("onboarding.scheme.storageNote")}</p>
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
