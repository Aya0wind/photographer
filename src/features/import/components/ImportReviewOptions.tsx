import { useTranslation } from "react-i18next";
import type { ImportMode } from "@/ipc/api";
import { subgroupSuggestions } from "@/features/albums/lib/ungroupedAlbum";
import type { ImportWizardFlow } from "../lib/useImportWizard";

export default function ImportReviewOptions({ flow }: { flow: ImportWizardFlow }) {
  const { t } = useTranslation();
  const { starting, device, targetLibrary, albumChoice, setAlbumChoice, albumId, setAlbumId, albums, newAlbumName, setNewAlbumName, albumError, setAlbumError, startError, copyOnly, policyPending, mode, setMode, secondEnabled, setSecondEnabled, importTargetPreview, albumSubgroup, setAlbumSubgroup, albumSubgroupNames, navigate, targetRoot, duplicatePolicy, setDuplicatePolicy, skipImported, setSkipImported, secondRoot, setSecondRoot, browseSecondRoot, secondImportTargetPreview } = flow;
  return <>
        <div className="mb-5 rounded-xl bg-panel/60 px-4 py-3 text-xs"><p className="truncate text-text-primary">{device?.name}</p><p className="mt-1 text-text-muted">{t("wizard.flow.library", { name: targetLibrary?.name ?? "—" })}</p></div>
        <fieldset disabled={starting} className="min-w-0 space-y-5">
          <label className="flex flex-col gap-2 text-xs text-text-secondary" data-testid="wizard-album-section">
            {t("wizard.album.label")}
            <select value={albumChoice === "new" ? "new" : albumId === null ? "" : String(albumId)} aria-label={t("wizard.album.existing")} onChange={(event) => { const value = event.currentTarget.value; setAlbumError(null); if (value === "new") setAlbumChoice("new"); else { setAlbumChoice("existing"); setAlbumId(value ? Number(value) : null); } }} className="rounded-xl border border-edge bg-bg px-3 py-2.5 text-sm text-text-primary" data-testid="wizard-album-select">
              <option value="">{t("wizard.album.selectPlaceholder")}</option>{albums.map((album) => <option key={album.id} value={album.id}>{album.name}（{album.itemCount}）</option>)}<option value="new">＋ {t("wizard.album.new")}</option>
            </select>
          </label>
          {albumChoice === "new" && <input autoFocus type="text" value={newAlbumName} onChange={(event) => { setNewAlbumName(event.currentTarget.value); setAlbumError(null); }} placeholder={t("albums.newNamePlaceholder")} aria-label={t("wizard.album.new")} className="w-full rounded-xl border border-edge bg-bg px-3 py-2.5 text-sm text-text-primary" data-testid="wizard-album-new-name" />}
          <label className="flex flex-col gap-2 text-xs text-text-secondary" data-testid="wizard-mode">
            {t("wizard.mode.label")}
            <select value={mode} disabled={policyPending || copyOnly} aria-label={t("wizard.mode.label")} onChange={(event) => { const next = event.currentTarget.value as ImportMode; setMode(next); if (next !== "copy") setSecondEnabled(false); }} className="rounded-xl border border-edge bg-bg px-3 py-2 text-text-primary">
              {(["copy", "move", "reference"] as const).map((option) => <option key={option} value={option} disabled={option !== "copy" && copyOnly}>{t(`wizard.mode.${option}`)}</option>)}
            </select>
            {copyOnly && <span className="text-[11px] leading-relaxed text-text-muted">{t("wizard.mode.copyOnly")}</span>}
          </label>
          <p className={`rounded-xl px-3 py-3 text-xs leading-relaxed ${mode === "move" ? "bg-amber-500/10 text-text-primary" : "bg-panel/40 text-text-secondary"}`}>{t(`wizard.mode.${mode}Desc`)}</p>
          <p className="break-all text-[11px] leading-relaxed text-text-muted" data-testid="wizard-album-path-preview">{t("wizard.album.pathPreview")}：{mode === "reference" ? t("wizard.mode.reference") : importTargetPreview}</p>
          <details className="rounded-xl border border-edge bg-bg/40 p-3" data-testid="wizard-advanced">
            <summary className="cursor-pointer text-xs font-medium text-text-secondary">{t("wizard.advanced")}</summary>
            {albumChoice === "existing" && albumId !== null && (
              <input
                type="text"
                value={albumSubgroup}
                onChange={(e) => setAlbumSubgroup(e.target.value)}
                list="wizard-subgroup-options"
                placeholder={t("wizard.album.subgroupPlaceholder")}
                aria-label={t("wizard.album.subgroup")}
                className="ml-5 rounded-md border border-edge bg-bg px-2 py-1.5 text-xs text-text-primary outline-none transition-colors placeholder:text-text-muted/60 focus:border-accent"
                data-testid="wizard-album-subgroup"
              />
            )}
            {albumChoice === "existing" && albumId !== null && (
              <datalist id="wizard-subgroup-options">
                {subgroupSuggestions(albumSubgroupNames).map((name) => (
                  <option key={name} value={name} />
                ))}
              </datalist>
            )}

          {/* 导入位置（照片库属性，只读）：目标 = 所选照片库 root，纯时间布局；
              照片库登记管理在「存储」页（M5：设置页旧「库」tab 已退役） */}
          <div className="mt-5 flex flex-col gap-1.5">
            <div className="flex items-center justify-between gap-2">
              <span className="text-xs font-medium text-text-secondary">
                {t("wizard.location.title")}
              </span>
              <button
                type="button"
                onClick={() => navigate("/storage")}
                className="shrink-0 rounded bg-panel px-1.5 py-0.5 text-[11px] text-text-muted transition-colors hover:text-accent"
                title={t("wizard.location.badge")}
                data-testid="wizard-location-edit"
              >
                {t("wizard.location.badge")}
              </button>
            </div>
            <div
              className="flex flex-col gap-1.5 rounded-lg border border-edge bg-bg p-2.5"
              data-testid="wizard-location-card"
            >
              {mode === "reference" ? (
                <span className="text-xs leading-relaxed text-text-secondary">{t("wizard.mode.referenceDesc")}</span>
              ) : targetLibrary ? (
                <span className="break-all font-mono text-xs text-text-primary" title={targetRoot}>
                  {targetRoot}
                </span>
              ) : (
                <p className="text-[11px] leading-relaxed text-text-muted">
                  {t("wizard.location.noLibrary")}
                </p>
              )}
            </div>
          </div>


          <fieldset className="mt-4 flex flex-col gap-1">
            <legend className="mb-1 text-xs font-medium text-text-secondary">
              {t("wizard.duplicatePolicy")}
            </legend>
            {(["skip", "rename", "ask"] as const).map((option) => (
              <label key={option} className="flex cursor-pointer items-center gap-2 text-xs text-text-secondary">
                <input
                  type="radio"
                  name="wizard.duplicatePolicy"
                  value={option}
                  checked={duplicatePolicy === option}
                  onChange={() => setDuplicatePolicy(option)}
                  className="h-3 w-3 accent-[#F0A83C]"
                />
                {t(`onboarding.scheme.dup.${option}`)}
              </label>
            ))}
          </fieldset>

          <label className="mt-4 flex cursor-pointer items-center gap-2 text-xs text-text-secondary">
            <input
              type="checkbox"
              checked={skipImported}
              onChange={(e) => setSkipImported(e.target.checked)}
              className="h-3 w-3 accent-[#F0A83C]"
            />
            {t("wizard.skipImported")}
          </label>

          {/* 双目的地（M2）：默认关；移动模式互斥（后端拒 move+secondTarget） */}
          <div className="mt-4 flex flex-col gap-1.5">
            <label
              className={`flex items-center gap-2 text-xs ${
                mode !== "copy" ? "cursor-not-allowed text-text-muted" : "cursor-pointer text-text-secondary"
              }`}
              title={mode === "move" ? t("wizard.second.moveUnsupported") : undefined}
            >
              <input
                type="checkbox"
                checked={secondEnabled}
                disabled={mode !== "copy"}
                onChange={(e) => setSecondEnabled(e.target.checked)}
                className="h-3 w-3 accent-[#F0A83C] disabled:opacity-40"
                data-testid="wizard-second-toggle"
              />
              {t("wizard.second.label")}
            </label>
            <p className="pl-5 text-[11px] leading-relaxed text-text-muted">
              {mode !== "copy"
                ? t("wizard.second.moveUnsupported")
                : t("wizard.second.desc")}
            </p>
            {secondEnabled && mode === "copy" && (
              <div className="mt-1 flex flex-col gap-1.5" data-testid="wizard-second-panel">
    <div className="flex min-w-0 flex-wrap items-center gap-1.5">
                  <input
                    type="text"
                    value={secondRoot}
                    onChange={(e) => setSecondRoot(e.target.value)}
                    placeholder={t("wizard.second.rootPlaceholder")}
                    aria-label={t("wizard.second.root")}
                    className="min-w-0 flex-1 rounded-md border border-edge bg-bg px-2 py-1.5 font-mono text-[11px] text-text-primary outline-none transition-colors focus:border-accent"
                    data-testid="wizard-second-root"
                  />
                  <button
                    type="button"
                    onClick={() => void browseSecondRoot()}
                    className="shrink-0 rounded-md border border-edge px-2 py-1.5 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
                    data-testid="wizard-second-browse"
                  >
                    {t("wizard.browse")}
                  </button>
                </div>
                {secondRoot.trim() === "" && (
                  <p className="pl-0.5 text-[11px] text-yellow-300">{t("wizard.second.required")}</p>
                )}
                <p className="text-[11px] leading-relaxed text-text-muted">
                  {t("wizard.second.sameTemplate")}
                </p>
                {secondRoot.trim() !== "" && (
                  <p
                    className="break-all font-mono text-[11px] text-text-secondary"
                    data-testid="wizard-second-path-preview"
                    title={secondImportTargetPreview}
                  >
                    {secondImportTargetPreview}
                  </p>
                )}
              </div>
            )}
          </div>

          </details>
        </fieldset>
        {(albumError ?? startError) && <p role="alert" className="mt-4 whitespace-pre-wrap rounded-xl bg-red-500/10 p-3 text-xs leading-relaxed text-red-500">{albumError ?? startError}</p>}
  </>;
}
