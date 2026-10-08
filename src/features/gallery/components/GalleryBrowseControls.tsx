import { useTranslation } from "react-i18next";
import FloatingToolbar from "@/shared/components/FloatingToolbar";
import TileSizeSwitch from "./TileSizeSwitch";
import type { GalleryTileSize } from "../lib/useGalleryTileSize";

export default function GalleryBrowseControls({ filterOpen, filterCount, onToggleFilter, tileSize, onTileSize, onImport, onCull, cullBusy, cullDisabled }: {
  filterOpen: boolean;
  filterCount: number;
  onToggleFilter: () => void;
  tileSize: GalleryTileSize;
  onTileSize: (size: GalleryTileSize) => void;
  onImport: () => void;
  onCull: () => void;
  cullBusy: boolean;
  cullDisabled: boolean;
}) {
  const { t } = useTranslation();
  return (
    <FloatingToolbar label={t("ui.browseActions")} locked={filterOpen || cullBusy}>
      <button type="button" className="ui-icon-button relative" onClick={onToggleFilter} aria-label={t("search.moreFilters")} title={t("search.moreFilters")} aria-expanded={filterOpen} data-testid="search-filter-toggle">
        <svg viewBox="0 0 20 20" width="17" height="17" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="M3 4h14l-5.5 6v5l-3 1v-6z" /></svg>
        {filterCount > 0 && <span className="absolute -right-1 -top-1 rounded-full bg-accent px-1 text-[10px] text-black" data-testid="search-filter-count">{filterCount}</span>}
      </button>
      <TileSizeSwitch value={tileSize} onChange={onTileSize} />
      <button type="button" onClick={onCull} disabled={cullDisabled || cullBusy}
        className="ui-icon-button disabled:cursor-not-allowed disabled:opacity-40"
        aria-label={cullBusy ? t("gallery.cullBusy") : t("gallery.cullFromFilter")}
        title={cullBusy ? t("gallery.cullBusy") : t("gallery.cullFromFilter")}
        data-testid="gallery-cull-start" data-busy={cullBusy}>
        {cullBusy ? <span className="h-4 w-4 animate-spin rounded-full border border-edge border-t-accent" aria-hidden="true" /> :
          <svg viewBox="0 0 20 20" width="17" height="17" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><rect x="3" y="4" width="14" height="12" rx="2" /><path d="m6 10 3 3 5-6" /></svg>}
      </button>
      <button type="button" className="ui-icon-button ui-primary" onClick={onImport} aria-label={t("nav.import")} title={t("nav.import")} data-testid="gallery-import">
        <svg viewBox="0 0 20 20" width="17" height="17" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="M10 2v10m-4-4 4 4 4-4M3 13v4h14v-4" /></svg>
      </button>
    </FloatingToolbar>
  );
}
