import { useState } from "react";
import { useTranslation } from "react-i18next";
import ActionPopover from "@/shared/components/ActionPopover";
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
  const [menuOpen, setMenuOpen] = useState(false);
  return (
    <FloatingToolbar label={t("ui.browseActions")} locked={filterOpen || menuOpen}>
      <button type="button" className="ui-icon-button relative" onClick={onToggleFilter} aria-label={t("search.moreFilters")} title={t("search.moreFilters")} aria-expanded={filterOpen} data-testid="search-filter-toggle">
        <svg viewBox="0 0 20 20" width="17" height="17" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="M3 4h14l-5.5 6v5l-3 1v-6z" /></svg>
        {filterCount > 0 && <span className="absolute -right-1 -top-1 rounded-full bg-accent px-1 text-[10px] text-black" data-testid="search-filter-count">{filterCount}</span>}
      </button>
      <ActionPopover label={t("ui.more")} onOpenChange={setMenuOpen}>
          <div className="flex items-center justify-between gap-4 text-xs text-text-secondary"><span>{t("gallery.tileSize.label")}</span><TileSizeSwitch value={tileSize} onChange={onTileSize} /></div>
          <button type="button" onClick={onCull} disabled={cullDisabled || cullBusy} className="rounded-lg px-3 py-2 text-left text-xs text-text-primary hover:bg-panel disabled:opacity-40" data-testid="gallery-cull-start" data-busy={cullBusy}>
            {cullBusy ? t("gallery.cullBusy") : t("gallery.cullFromFilter")}
          </button>
      </ActionPopover>
      <button type="button" className="ui-icon-button ui-primary" onClick={onImport} aria-label={t("nav.import")} title={t("nav.import")} data-testid="gallery-import">
        <svg viewBox="0 0 20 20" width="17" height="17" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="M10 2v10m-4-4 4 4 4-4M3 13v4h14v-4" /></svg>
      </button>
    </FloatingToolbar>
  );
}
