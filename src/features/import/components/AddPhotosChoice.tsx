import { useTranslation } from "react-i18next";
import { DeviceGlyph } from "./ImportSourcePicker";

export default function AddPhotosChoice({ onImport, onExisting, busy }: {
  onImport: () => void; onExisting: () => void; busy: boolean;
}) {
  const { t } = useTranslation();
  return <div className="sp-scroll min-h-0 flex-1 overflow-y-auto px-5 py-10" data-testid="add-photos-choice">
    <div className="mx-auto w-full max-w-4xl">
      <h2 className="text-2xl font-semibold text-text-primary">{t("addPhotos.heading")}</h2>
      <p className="mt-3 text-sm leading-relaxed text-text-secondary">{t("addPhotos.description")}</p>
      <div className="mt-8 grid gap-5 md:grid-cols-2">
        {(["import", "existing"] as const).map(kind => <section key={kind} className="flex flex-col rounded-2xl border border-edge bg-panel/40 p-6">
          <div className="mb-5 flex h-12 w-12 items-center justify-center rounded-xl bg-accent/10 text-accent"><DeviceGlyph kind={kind === "import" ? "camera" : "folder"} size={28} /></div>
          <h3 className="text-base font-semibold text-text-primary">{t(`addPhotos.${kind}.title`)}</h3>
          <p className="mb-6 mt-3 text-sm leading-relaxed text-text-secondary">{t(`addPhotos.${kind}.description`)}</p>
          <button type="button" disabled={busy} onClick={kind === "import" ? onImport : onExisting}
            className="ui-primary mt-auto self-start rounded-xl px-4 py-2.5 text-sm font-medium disabled:opacity-40" data-testid={`add-photos-${kind}`}>
            {t(`addPhotos.${kind}.button`)}
          </button>
        </section>)}
      </div>
    </div>
  </div>;
}
