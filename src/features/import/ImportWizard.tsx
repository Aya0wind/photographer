import { useTranslation } from "react-i18next";
import { formatBytes } from "@/lib/format";
import ErrorModal from "@/shared/components/ErrorModal";
import ImportSourcePicker from "./components/ImportSourcePicker";
import ImportPhotoPicker from "./components/ImportPhotoPicker";
import ImportReviewDrawer from "./components/ImportReviewDrawer";
import ImportReviewOptions from "./components/ImportReviewOptions";
import { useImportWizard } from "./lib/useImportWizard";

export { fetchThumbUrl, resetThumbCacheForTests } from "./lib/sourceThumbs";
export { VIEW_MODE_STORAGE_KEY, PANEL_COLLAPSE_KEY, COL_WIDTHS_KEY, TILE_SIZE_KEY } from "./lib/useImportLayout";
export type { WizardViewMode } from "./lib/useImportLayout";

export default function ImportWizard() {
  const { t } = useTranslation();
  const flow = useImportWizard();
  const { step, setStep, starting, device, canReview, visibleDevices, recentSources, selectedId, scanningFolder, platformCaps, selectDevice, browseFolder, selectRecent, selectedCount, selectedBytes, blockedReason, canStart, startImport, sourceError, setSourceError } = flow;

  return (
    <div className="flex h-full min-h-0 flex-col bg-bg" data-testid="import-wizard" data-step={step}>
      <header className="ui-glass flex min-h-16 shrink-0 flex-wrap items-center gap-4 border-b border-edge px-5 py-3">
        <h1 className="text-base font-semibold tracking-tight text-text-primary">{t("wizard.title")}</h1>
        <nav className="ml-auto flex items-center gap-2 text-xs" aria-label={t("wizard.flow.steps")}>
          {(["source", "photos", "review"] as const).map((item, index) => <button key={item} type="button" disabled={starting || (item === "photos" && (!device || scanningFolder !== null)) || (item === "review" && !canReview)} onClick={() => setStep(item)} aria-current={step === item ? "step" : undefined}
            className={`flex items-center gap-2 rounded-full px-3 py-1.5 transition-colors disabled:opacity-40 ${step === item ? "bg-accent/10 text-accent" : "text-text-muted hover:bg-panel"}`}>
            <span className={`flex h-5 w-5 items-center justify-center rounded-full font-mono text-[10px] ${step === item ? "bg-accent text-black" : "bg-panel"}`}>{index + 1}</span>{t(`wizard.flow.step.${item}`)}
          </button>)}
        </nav>
      </header>

      {step === "source" ? <ImportSourcePicker devices={visibleDevices} recentSources={recentSources} selectedId={selectedId} scanningPath={scanningFolder}
        nativeUnavailable={Boolean(platformCaps && !platformCaps.volumeDevices && !platformCaps.portableDevices)}
        onDevice={selectDevice} onFolder={() => void browseFolder()} onRecent={selectRecent} /> : <ImportPhotoPicker flow={flow} />}

      {step === "review" && <ImportReviewDrawer busy={starting} onBack={() => setStep("photos")} footer={<div data-testid="wizard-import-summary">
        <div className="mb-3 flex items-baseline justify-between text-sm font-semibold"><span>{t("wizard.readyCount", { count: selectedCount })}</span><span className="text-xs font-normal text-text-secondary">{formatBytes(selectedBytes)}</span></div>
        <p role="status" className="mb-3 text-xs leading-relaxed text-text-muted">{blockedReason ?? t("wizard.backgroundHint")}</p>
        <button type="button" disabled={!canStart} aria-label={t("wizard.start")} onClick={() => void startImport()} className="ui-primary w-full rounded-xl px-4 py-3 text-sm font-semibold disabled:cursor-not-allowed disabled:opacity-40">{starting ? t("wizard.starting") : t("wizard.startCount", { count: selectedCount })}</button>
      </div>}>
        <ImportReviewOptions flow={flow} />
      </ImportReviewDrawer>}
      <ErrorModal message={sourceError} onClose={() => setSourceError(null)} />
    </div>
  );
}
