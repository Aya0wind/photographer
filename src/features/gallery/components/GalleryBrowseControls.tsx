import { useTranslation } from "react-i18next";
import FloatingToolbar from "@/shared/components/FloatingToolbar";
import TileSizeSwitch from "./TileSizeSwitch";
import type { GalleryTileSize } from "../lib/useGalleryTileSize";

/** 跨库重复折叠开关图标：两张叠起的同内容卡片（显示过滤语义） */
function GlyphDuplicates() {
  return (
    <svg viewBox="0 0 20 20" width="17" height="17" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <rect x="4.5" y="4.5" width="9" height="9" rx="1.5" />
      <path d="M8 2.5h7A1.5 1.5 0 0 1 16.5 4v7" />
    </svg>
  );
}

export default function GalleryBrowseControls({ filterOpen, filterCount, onToggleFilter, tileSize, onTileSize, onImport, onCull, cullBusy, cullDisabled, hideDuplicates, onToggleDuplicates, duplicatesActive, inactive = false }: {
  filterOpen: boolean;
  filterCount: number;
  onToggleFilter: () => void;
  tileSize: GalleryTileSize;
  onTileSize: (size: GalleryTileSize) => void;
  onImport: () => void;
  onCull: () => void;
  cullBusy: boolean;
  cullDisabled: boolean;
  /** 「隐藏跨库重复」开关（M5 §四纯显示过滤；语义态不显示） */
  hideDuplicates: boolean;
  onToggleDuplicates: () => void;
  /** 当前结果里存在跨库重复组（无重复时开关降为可点但无效果，样式弱化） */
  duplicatesActive: boolean;
  inactive?: boolean;
}) {
  const { t } = useTranslation();
  return (
    <FloatingToolbar label={t("ui.browseActions")} locked={filterOpen || cullBusy} inactive={inactive}>
      <button type="button" className="ui-icon-button relative" onClick={onToggleFilter} aria-label={t("search.moreFilters")} title={t("search.moreFilters")} aria-expanded={filterOpen} data-testid="search-filter-toggle">
        <svg viewBox="0 0 20 20" width="17" height="17" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="M3 4h14l-5.5 6v5l-3 1v-6z" /></svg>
        {filterCount > 0 && <span className="absolute -right-1 -top-1 rounded-full bg-accent px-1 text-[10px] text-black" data-testid="search-filter-count">{filterCount}</span>}
      </button>
      <TileSizeSwitch value={tileSize} onChange={onTileSize} />
      <button
        type="button"
        onClick={onToggleDuplicates}
        aria-pressed={hideDuplicates}
        aria-label={t("gallery.hideDuplicates")}
        title={hideDuplicates ? t("gallery.hideDuplicatesOnHint") : t("gallery.hideDuplicatesHint")}
        data-testid="gallery-hide-duplicates"
        data-pressed={hideDuplicates}
        data-has-duplicates={duplicatesActive}
        className={`ui-icon-button transition-opacity ${hideDuplicates ? "text-accent" : ""} ${duplicatesActive || hideDuplicates ? "" : "opacity-45"}`}
      >
        <GlyphDuplicates />
      </button>
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
