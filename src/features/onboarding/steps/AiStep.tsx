import { useTranslation } from "react-i18next";

import { QUALITY_TIERS } from "@/features/settings/lib/qualityTier";
import { AI_CHOICES, type OnboardingDraft } from "../types";
import AiDownloadPanel from "./AiDownloadPanel";

interface Props {
  draft: OnboardingDraft;
  onChange: (patch: Partial<OnboardingDraft>) => void;
  preparing?: boolean;
}

/** 步骤 3：选择 AI 功能和处理方案，沿用设置页的档位契约。 */
export default function AiStep({ draft, onChange, preparing = false }: Props) {
  const { t } = useTranslation();

  return (
    <div className="flex flex-col gap-4">
      <AiDownloadPanel draft={draft} preparing={preparing} />
      <div className="grid gap-3">
        {AI_CHOICES.map((choice) => {
          const selected = draft.aiChoice === choice;
          return (
            <button
              key={choice}
              type="button"
              onClick={() => onChange({ aiChoice: choice })}
              aria-pressed={selected}
              className={`rounded-lg border p-4 text-left transition-colors ${
                selected
                  ? "border-accent bg-panel/60"
                  : "border-edge bg-surface hover:border-text-muted"
              }`}
            >
              <span className={`text-sm font-medium ${selected ? "text-accent" : "text-text-primary"}`}>
                {t(`onboarding.ai.choice.${choice}`)}
              </span>
              <p className="mt-1 text-xs leading-relaxed text-text-secondary">
                {t(`onboarding.ai.choice.${choice}Desc`)}
              </p>
            </button>
          );
        })}
      </div>
      {draft.aiChoice !== "none" && (
        <fieldset className="flex flex-col gap-2">
          <legend className="mb-2 text-sm font-medium text-text-primary">
            {t("onboarding.ai.qualityTier")}
          </legend>
          <div className="grid grid-cols-3 gap-2">
            {QUALITY_TIERS.map((tier) => (
              <label
                key={tier}
                className={`cursor-pointer rounded-lg border p-3 ${
                  draft.qualityTier === tier ? "border-accent bg-panel/60" : "border-edge"
                }`}
              >
                <span className="flex items-center gap-2 text-sm font-medium">
                  <input
                    type="radio"
                    name="qualityTier"
                    value={tier}
                    checked={draft.qualityTier === tier}
                    onChange={() => onChange({ qualityTier: tier })}
                    className="h-3.5 w-3.5 accent-[#F0A83C]"
                  />
                  {t(`onboarding.ai.tier.${tier}`)}
                </span>
                <p className="mt-2 text-xs leading-relaxed text-text-secondary">
                  {t(`settings.ai.tier.${tier}.desc`)}
                </p>
              </label>
            ))}
          </div>
          <p className="text-xs text-text-muted">{t("onboarding.ai.qualityNote")}</p>
        </fieldset>
      )}
      <p className="text-xs text-text-muted">{t("onboarding.ai.note")}</p>
    </div>
  );
}
