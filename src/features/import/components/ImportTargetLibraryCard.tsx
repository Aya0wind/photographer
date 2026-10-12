import { useTranslation } from "react-i18next";
import type { ImportWizardFlow } from "../lib/useImportWizard";
import { FolderGlyph } from "../SourceFileViews";

export default function ImportTargetLibraryCard({ flow }: { flow: ImportWizardFlow }) {
  const { t } = useTranslation();
  const { targetLibrary, targetLibraryId, setTargetLibraryId, photoLibraries, starting } = flow;
  return (
    <section className="rounded-2xl border border-edge bg-panel/40 p-4" data-testid="wizard-target-library-card">
      <div className="mb-3 flex items-center gap-3">
        <FolderGlyph size={22} className="shrink-0 text-accent" />
        <h2 className="text-sm font-semibold text-text-primary">{t("storage.organization.targetLibrary")}</h2>
      </div>
      <select
        aria-label={t("storage.organization.targetLibrary")}
        value={targetLibraryId}
        disabled={starting}
        onChange={e => setTargetLibraryId(e.target.value)}
        className="w-full rounded-xl border border-edge bg-bg px-3 py-2.5 text-sm font-medium text-text-primary outline-none focus:border-accent"
      >
        <option value="">{t("storage.organization.selectLibrary")}</option>
        {photoLibraries.map(lib => (
          <option key={lib.id} value={lib.id} disabled={lib.status !== "online" || lib.organizationPending}>{lib.name}</option>
        ))}
      </select>
      {targetLibrary && <div className="mt-3 space-y-1">
        <p className="text-[11px] text-text-muted">{t("wizard.location.title")}</p>
        <p className="break-all text-xs leading-relaxed text-text-secondary" title={targetLibrary.rootPath}>{targetLibrary.rootPath}</p>
      </div>}
    </section>
  );
}
