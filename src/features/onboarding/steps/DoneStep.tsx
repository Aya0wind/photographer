import { useTranslation } from "react-i18next";

import { previewTemplate } from "../onboardingConfig";
import type { OnboardingDraft } from "../types";

interface Props {
  draft: OnboardingDraft;
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-center justify-between gap-4 border-t border-panel pt-3 text-left">
      <span className="shrink-0 text-sm text-text-secondary">{label}</span>
      <span className="truncate font-mono text-xs text-text-primary" title={value}>
        {value}
      </span>
    </div>
  );
}

/** 步骤 4：确认摘要——提交动作由向导底部"开始使用"触发 */
export default function DoneStep({ draft }: Props) {
  const { t } = useTranslation();

  return (
    <div className="flex flex-col gap-3">
      <Row label={t("onboarding.done.library")} value={`${draft.libraryName}（${draft.photoRoot}）`} />
      <Row label={t("onboarding.done.dbDir")} value={draft.dbDir} />
      <Row label={t("onboarding.done.template")} value={draft.dirTemplate} />
      <Row label={t("onboarding.done.preview")} value={previewTemplate(draft.dirTemplate, draft.photoRoot)} />
      <Row label={t("onboarding.done.duplicate")} value={t(`onboarding.scheme.dup.${draft.duplicatePolicy}`)} />
      <Row label={t("onboarding.done.ai")} value={t(`onboarding.ai.choice.${draft.aiChoice}`)} />
      <p className="mt-2 text-xs text-text-muted">{t("onboarding.done.note")}</p>
    </div>
  );
}
