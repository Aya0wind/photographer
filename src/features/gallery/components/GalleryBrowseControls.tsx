import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import FloatingToolbar from "@/shared/components/FloatingToolbar";
import TileSizeSwitch from "./TileSizeSwitch";
import LayoutSwitch from "./LayoutSwitch";
import type { GalleryTileSize } from "../lib/useGalleryTileSize";
import type { GalleryLayout } from "../lib/useGalleryLayout";

export default function GalleryBrowseControls({ filterOpen, filterCount, onToggleFilter, tileSize, onTileSize, layout, onLayout, onImport, onCull, cullBusy, cullDisabled, inactive = false, children, inline = false, filterTestId = "search-filter-toggle", cullTestId = "gallery-cull-start", cullLabel }: {
  children?: ReactNode;
  inline?: boolean;
  filterTestId?: string;
  cullTestId?: string;
  cullLabel?: string;
  filterOpen: boolean;
  filterCount: number;
  onToggleFilter: () => void;
  tileSize: GalleryTileSize;
  onTileSize: (size: GalleryTileSize) => void;
  /** 布局模式（四档循环切换，与尺寸开关相邻同组） */
  layout: GalleryLayout;
  onLayout: (next: GalleryLayout) => void;
  onImport: () => void;
  onCull: () => void;
  cullBusy: boolean;
  cullDisabled: boolean;
  inactive?: boolean;
}) {
  const { t } = useTranslation();
  return (
    <FloatingToolbar label={t("ui.browseActions")} locked={filterOpen || cullBusy} inactive={inactive} inline={inline}>
      {/* 胶囊宽按钮（图标 + 文字说明；计数徽标随行内 flex 排） */}
      <button type="button" className="ui-pill-button" onClick={onToggleFilter} aria-label={t("search.moreFilters")} title={t("search.moreFilters")} aria-expanded={filterOpen} data-filter-toggle="" data-testid={filterTestId}>
        <svg viewBox="0 0 20 20" width="17" height="17" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="M3 4h14l-5.5 6v5l-3 1v-6z" /></svg>
        {t("search.moreFilters")}
        {filterCount > 0 && <span className="rounded-full bg-accent px-1.5 text-[10px] font-bold leading-4 text-black" data-testid="search-filter-count">{filterCount}</span>}
      </button>
      <TileSizeSwitch value={tileSize} onChange={onTileSize} />
      <LayoutSwitch value={layout} onChange={onLayout} />
      {children}
      <button type="button" onClick={onCull} disabled={cullDisabled || cullBusy}
        className="ui-pill-button disabled:cursor-not-allowed disabled:opacity-40"
        aria-label={cullBusy ? t("gallery.cullBusy") : (cullLabel ?? t("gallery.cullFromFilter"))}
        title={cullBusy ? t("gallery.cullBusy") : (cullLabel ?? t("gallery.cullFromFilter"))}
        data-testid={cullTestId} data-busy={cullBusy}>
        {cullBusy ? <span className="h-4 w-4 animate-spin rounded-full border border-edge border-t-accent" aria-hidden="true" /> :
        <svg viewBox="0 0 20 20" width="17" height="17" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><rect x="3" y="4" width="14" height="12" rx="2" /><path d="m6 10 3 3 5-6" /></svg>}
        {cullBusy ? t("gallery.cullBusy") : (cullLabel ?? t("gallery.cullFromFilter"))}
      </button>
      <button type="button" className="ui-pill-button ui-primary" onClick={onImport} aria-label={t("nav.import")} title={t("nav.import")} data-testid="gallery-import">
        <svg viewBox="0 0 20 20" width="17" height="17" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="M10 2v10m-4-4 4 4 4-4M3 13v4h14v-4" /></svg>
        {t("nav.import")}
      </button>
    </FloatingToolbar>
  );
}
