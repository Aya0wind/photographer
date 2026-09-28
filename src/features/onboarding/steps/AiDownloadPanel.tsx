import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { aiModelDownload } from "@/ipc/api";
import { useAiStore } from "@/stores/aiStore";
import { gapsForIds, normalizeModelId } from "@/features/settings/lib/qualityTier";
import { formatBytes } from "@/lib/format";
import { aiSetupPackages } from "../aiSetup";
import type { OnboardingDraft } from "../types";

export default function AiDownloadPanel({ draft, preparing }: { draft: OnboardingDraft; preparing: boolean }) {
  const { t } = useTranslation();
  const models = useAiStore((s) => s.models);
  const progress = useAiStore((s) => s.downloadProgress);
  const refresh = useAiStore((s) => s.refresh);
  const [error, setError] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);
  const packages = aiSetupPackages(draft);
  const ids = packages.flatMap((group) => group.ids);
  const key = ids.join(",");

  useEffect(() => {
    void refresh();
    const timer = setInterval(() => { void refresh(); }, 2000);
    return () => clearInterval(timer);
  }, [refresh]);

  useEffect(() => {
    if (!preparing) return;
    let cancelled = false;
    setError(null);
    void (async () => {
      await refresh();
      if (cancelled) return;
      const gaps = gapsForIds(ids, useAiStore.getState().models);
      if (gaps.some((gap) => !gap.status)) {
        setError(t("onboarding.ai.downloadUnavailable"));
        return;
      }
      for (const gap of gaps) {
        if (cancelled) return;
        if (gap.status?.state === "downloading" || gap.status?.state === "verifying") continue;
        try { await aiModelDownload(gap.status!.id, true); }
        catch { if (!cancelled) setError(t("onboarding.ai.downloadFailed")); }
      }
      if (!cancelled) await refresh();
    })();
    return () => { cancelled = true; };
    // 状态刷新不重新发起下载；重试按钮只补缺失资源。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [preparing, key, retry, refresh, t]);

  if (!preparing || packages.length === 0) return null;
  const failed = ids.some((id) => models.find((m) => normalizeModelId(m.id) === id)?.state === "failed");
  const ready = gapsForIds(ids, models).length === 0;
  return (
    <section className="mt-4 space-y-3 rounded-lg border border-edge bg-bg/50 p-4" data-testid="onboarding-ai-download">
      <h3 className="text-sm font-semibold">{t(ready ? "onboarding.ai.downloadReady" : "onboarding.ai.downloadTitle")}</h3>
      <p className="text-xs leading-relaxed text-text-muted">{t("onboarding.ai.downloadHint")}</p>
      {packages.map(({ feature, ids: groupIds }) => {
        const group = groupIds.map((id) => models.find((m) => normalizeModelId(m.id) === id));
        const done = group.filter((m) => m?.state === "done").length;
        const verifying = group.some((m) => m?.state === "verifying");
        const bytes = group.reduce((sum, m) => sum + (m?.bytesTotal ?? 0), 0);
        const downloaded = group.reduce((sum, m) => sum + (!m ? 0 : m.state === "done" ? m.bytesTotal : progress[m.id]?.doneBytes ?? m.downloadedBytes), 0);
        const pct = done === groupIds.length ? 100 : bytes > 0 ? Math.min(99, downloaded / bytes * 100) : 0;
        return <div key={feature} data-testid={`onboarding-ai-package-${feature}`}>
          <div className="mb-1.5 flex justify-between gap-3 text-xs">
            <span>{t(`settings.ai.package.${feature}`)}</span>
            <span className="text-text-muted">{done === groupIds.length ? t("onboarding.ai.packageReady") : verifying ? t("onboarding.ai.verifying") : `${formatBytes(downloaded)} / ${formatBytes(bytes)}`}</span>
          </div>
          <div role="progressbar" aria-label={t(`settings.ai.package.${feature}`)} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(pct)} className="h-1.5 overflow-hidden rounded bg-panel">
            <div className="h-full bg-accent transition-[width]" style={{ width: `${pct}%` }} />
          </div>
        </div>;
      })}
      {(error || failed) && <div role="alert" className="flex items-center justify-between gap-3 text-xs text-red-400">
        <span>{error ?? t("onboarding.ai.downloadFailed")}</span>
        <button type="button" onClick={() => setRetry((n) => n + 1)} className="shrink-0 rounded border border-edge px-3 py-1.5 text-text-primary">{t("onboarding.ai.retryDownload")}</button>
      </div>}
    </section>
  );
}
