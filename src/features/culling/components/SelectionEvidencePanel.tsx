import { useTranslation } from "react-i18next";
import type { AssetDetailDto } from "@/ipc/api/types";

/** Human-readable region evidence. Raw, uncalibrated outputs stay out of the
 * verdict UI; developers can inspect them in the saved analysis payload. */
export function SelectionEvidencePanel({ analysis, onRegionSelect }: {
  analysis: AssetDetailDto["aiAnalysis"]; onRegionSelect?: (bounds: number[]) => void;
}) {
  const { t } = useTranslation();
  const channels = [analysis?.eyes, analysis?.blur].filter(channel => channel?.details);
  if (!channels.length) return null;
  return <details className="w-full min-w-0 text-xs text-text-muted" data-testid="selection-evidence">
    <summary className="cursor-pointer text-text">{t("viewer.ai.evidence")}</summary>
    {channels.map((channel, index) => {
      const d = channel!.details!;
      return <div key={index} className="mt-2 space-y-1 text-left">
        {d.source && <p>{t(`viewer.ai.source.${d.source}`)} · {d.width} × {d.height}</p>}
        {d.reason && <p>{t(`viewer.ai.reason.${d.reason}`)}</p>}
        {d.regions.map((region, i) => <button type="button" key={i}
          disabled={!onRegionSelect}
          className="block w-full text-left enabled:hover:text-accent" onClick={() => onRegionSelect?.(region.bounds)}>
          {region.person > 0 && <>{t("viewer.ai.person", { count: region.person })} · </>}{t(`viewer.ai.region.${region.side ?? region.kind}`)}
          {"："}{t(`viewer.ai.state.${region.state}`)}
          {region.reason && <> · {t(`viewer.ai.reason.${region.reason}`)}</>}
        </button>)}
      </div>;
    })}
  </details>;
}
