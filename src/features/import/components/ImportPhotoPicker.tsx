import { useTranslation } from "react-i18next";
import ActionPopover from "@/shared/components/ActionPopover";
import { formatBytes } from "@/lib/format";
import { devicePresentationKind } from "../devicePresentation";
import { FolderGlyph, sourceBasePath, FileListView, FileGridView } from "../SourceFileViews";
import { type WizardViewMode, type TileSizeKey, TILE_SIZE_ORDER, TILE_SIZE_ICON, TILE_SIZE_SPECS } from "../lib/useImportLayout";
import type { ImportWizardFlow } from "../lib/useImportWizard";
import { DeviceGlyph } from "./ImportSourcePicker";

function ViewControls({
  viewMode,
  tileSize,
  onViewMode,
  onTileSize,
}: {
  viewMode: WizardViewMode;
  tileSize: TileSizeKey;
  onViewMode: (mode: WizardViewMode) => void;
  onTileSize: (size: TileSizeKey) => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex items-center gap-1.5">
      <div className="flex rounded-lg border border-edge bg-bg/70 p-0.5" role="radiogroup"
        aria-label={t("wizard.view.label")} data-testid="wizard-view">
        {(["list", "grid"] as const).map((option) => (
          <button key={option} type="button" role="radio" aria-checked={viewMode === option}
            onClick={() => onViewMode(option)}
            className={`rounded-md px-2.5 py-1 text-[11px] font-medium transition-colors ${
              viewMode === option ? "bg-panel text-text-primary shadow-sm" : "text-text-muted hover:text-text-primary"
            }`} data-testid={`wizard-view-${option}`}>
            {t(`wizard.view.${option}`)}
          </button>
        ))}
      </div>
      {viewMode === "grid" && (
        <div className="flex rounded-lg border border-edge bg-bg/70 p-0.5" role="radiogroup"
          aria-label={t("wizard.tileSize.label")} data-testid="wizard-tile-size">
          {TILE_SIZE_ORDER.map((option) => (
            <button key={option} type="button" role="radio" aria-checked={tileSize === option}
              aria-label={t(`wizard.tileSize.${option}`)} title={t(`wizard.tileSize.${option}`)}
              onClick={() => onTileSize(option)}
              className={`flex h-6 w-7 items-center justify-center rounded-md transition-colors ${
                tileSize === option ? "bg-panel text-accent shadow-sm" : "text-text-muted hover:text-text-primary"
              }`} data-testid={`wizard-tile-size-${option}`}>
              <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
                <rect x={(16 - TILE_SIZE_ICON[option]) / 2} y={(16 - TILE_SIZE_ICON[option]) / 2}
                  width={TILE_SIZE_ICON[option]} height={TILE_SIZE_ICON[option]} rx="1" />
              </svg>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

export default function ImportPhotoPicker({ flow }: { flow: ImportWizardFlow }) {
  const { t } = useTranslation();
  const { setStep, device, filesLoading, files, selected, selectedCount, selectedBytes, groups, collapsed, toggleFile, toggleGroup, toggleCollapse, clearSelection, selectAll, invertSelection, refreshDevice, viewMode, setViewMode, tileSize, setTileSize, canReview, blockedReason } = flow;
  return <>
        <div className="flex shrink-0 flex-wrap items-center gap-3 px-5 py-4" data-testid="wizard-table-stats">
          <button type="button" className="ui-glass flex min-w-0 max-w-md items-center gap-3 rounded-xl border px-3 py-2 text-left" onClick={() => setStep("source")} data-testid="wizard-source-toggle">
            <DeviceGlyph kind={device ? devicePresentationKind(device) : "folder"} size={18} className="text-accent" />
            <span className="min-w-0"><span className="block truncate text-xs font-medium text-text-primary" title={device ? sourceBasePath(device) ?? device.name : undefined}>{device?.name ?? t("wizard.chooseSource")}</span><span className="block text-[10px] text-text-muted">{t("wizard.flow.changeSource")}</span></span>
          </button>
          <p role="status" className="min-w-32 flex-1 text-xs text-text-muted">
            {device?.scanStatus === "scanning" ? t("wizard.scanningPhotos", { count: files.length }) : t("wizard.selectedStats", { selected: selectedCount, total: files.length, size: formatBytes(selectedBytes) })}
          </p>
          <button type="button" onClick={selected.size === files.length ? clearSelection : selectAll} disabled={files.length === 0} className="rounded-lg px-3 py-2 text-xs text-text-secondary hover:bg-panel disabled:opacity-40">{t(selected.size === files.length && files.length > 0 ? "wizard.flow.clearSelection" : "wizard.selectAll")}</button>
          <ViewControls viewMode={viewMode} tileSize={tileSize} onViewMode={setViewMode} onTileSize={setTileSize} />
          <ActionPopover label={t("ui.more")} closeOnAction><button type="button" onClick={invertSelection} disabled={files.length === 0} className="rounded-lg px-2 py-2 text-left text-xs text-text-secondary hover:bg-panel">{t("wizard.invert")}</button><button type="button" onClick={() => { if (device) void refreshDevice(device.id); }} disabled={!device || device.scanStatus === "scanning"} className="rounded-lg px-2 py-2 text-left text-xs text-text-secondary hover:bg-panel">{t("wizard.flow.refresh")}</button></ActionPopover>
        </div>

        <section className="mx-5 mb-3 flex min-h-0 flex-1 flex-col overflow-hidden rounded-2xl border border-edge bg-surface" aria-label={t("wizard.fileTable")}>
          {files.length === 0 ? <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 px-8 text-center">
            {filesLoading || device?.scanStatus === "scanning" ? <span className="h-8 w-8 animate-spin rounded-full border-2 border-edge border-t-accent" aria-hidden="true" /> : <FolderGlyph size={40} className="text-text-muted" />}
            <p className="max-w-md text-sm leading-relaxed text-text-secondary">{filesLoading || device?.scanStatus === "scanning" ? t("wizard.tableLoading") : !device ? t("wizard.flow.sourceLost") : device.scanStatus === "failed" ? t("wizard.deviceScanFailed") : t("wizard.tableEmpty")}</p>
            {!device && <button type="button" className="ui-primary rounded-xl px-4 py-2 text-xs" onClick={() => setStep("source")}>{t("wizard.chooseSource")}</button>}
          </div> : viewMode === "list" ? <FileListView groups={groups} collapsed={collapsed} selected={selected} onToggleFile={toggleFile} onToggleGroup={toggleGroup} onToggleCollapse={toggleCollapse} rootDirLabel={t("wizard.rootDir")} /> :
            <FileGridView groups={groups} collapsed={collapsed} selected={selected} onToggleFile={toggleFile} onToggleGroup={toggleGroup} onToggleCollapse={toggleCollapse} basePath={sourceBasePath(device)} rootDirLabel={t("wizard.rootDir")} tile={TILE_SIZE_SPECS[tileSize]} device={device} />}
        </section>
        <footer className="ui-glass flex shrink-0 items-center gap-4 border-t border-edge px-5 py-4">
          <div className="min-w-0 flex-1"><p className="text-sm font-medium text-text-primary">{t("wizard.readyCount", { count: selectedCount })}<span className="ml-3 text-xs font-normal text-text-muted">{formatBytes(selectedBytes)}</span></p><p className="mt-1 text-xs text-text-muted">{canReview ? t("wizard.flow.nextHint") : blockedReason}</p></div>
          <button type="button" disabled={!canReview} onClick={() => setStep("review")} className="ui-primary shrink-0 rounded-xl px-6 py-2.5 text-sm font-semibold disabled:cursor-not-allowed disabled:opacity-40" data-testid="wizard-review-next">{t("wizard.flow.continue")}<span className="ml-2" aria-hidden="true">→</span></button>
        </footer>
      </>;
}
