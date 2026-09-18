import { useTranslation } from "react-i18next";

import { AI_CHOICES, type AiChoice } from "../types";

interface Props {
  value: AiChoice;
  onChange: (choice: AiChoice) => void;
}

/** 步骤 3：AI 三选一——全部开启 / 仅语义搜索 / 全部关闭 */
export default function AiStep({ value, onChange }: Props) {
  const { t } = useTranslation();

  return (
    <div className="flex flex-col gap-4">
      <div className="grid gap-3">
        {AI_CHOICES.map((choice) => {
          const selected = value === choice;
          return (
            <button
              key={choice}
              type="button"
              onClick={() => onChange(choice)}
              aria-pressed={selected}
              className={`rounded-lg border p-4 text-left transition-colors ${
                selected
                  ? "border-accent bg-panel/60"
                  : "border-panel bg-surface hover:border-text-muted"
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
      <p className="text-xs text-text-muted">{t("onboarding.ai.note")}</p>
    </div>
  );
}
