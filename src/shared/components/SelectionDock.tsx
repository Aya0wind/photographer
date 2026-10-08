import { useTranslation } from "react-i18next";
import type { MouseEvent } from "react";

export default function SelectionDock({ count, onMove, onMore, onClear }: {
  count: number;
  onMove: () => void;
  onMore: (event: MouseEvent<HTMLButtonElement>) => void;
  onClear: () => void;
}) {
  const { t } = useTranslation();
  if (count === 0) return null;
  return (
    <div role="toolbar" aria-label={t("ui.selectionActions")} className="ui-glass absolute bottom-5 left-1/2 z-20 flex max-w-[calc(100%_-_24px)] -translate-x-1/2 items-center gap-3 rounded-2xl border px-4 py-2 shadow-lg" data-testid="selection-dock">
      <span className="whitespace-nowrap text-xs text-text-secondary">{t("ui.selectedCount", { count })}</span>
      <span className="h-5 w-px bg-edge" aria-hidden="true" />
      <button type="button" onClick={onMove} className="whitespace-nowrap rounded-lg px-2 py-1.5 text-xs text-text-primary hover:bg-panel">{t("ui.moveToAlbum")}</button>
      <button type="button" onClick={onMore} className="ui-icon-button" aria-label={t("ui.more")} title={t("ui.more")}>···</button>
      <button type="button" onClick={onClear} className="ui-icon-button" aria-label={t("ui.clearSelection")} title={t("ui.clearSelection")}>×</button>
    </div>
  );
}
