import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { getIntlLocale } from "@/i18n";
import type { DeviceSnapshot } from "@/ipc/api";
import type { RecentSource } from "@/stores/importStore";
import { type DevicePresentationKind, deviceKindLabelKey, devicePresentationKind } from "../devicePresentation";
import { FolderGlyph } from "../SourceFileViews";

/** 设备类型图标（读卡器=存储卡 / 相机 / 文件夹），16 viewBox stroke 手写 */
export function DeviceGlyph({
  kind,
  size = 14,
  className = "",
}: {
  kind: DevicePresentationKind;
  size?: number;
  className?: string;
}) {
  const paths: Record<DevicePresentationKind, ReactNode> = {
    reader: (
      <>
        <path d="M4.6 2.5h4.5L12.5 6v6.2c0 .8-.6 1.3-1.4 1.3H4.6c-.9 0-1.6-.7-1.6-1.5V4c0-.8.7-1.5 1.6-1.5z" />
        <path d="M9.1 2.5V6h3.4" />
      </>
    ),
    camera: (
      <>
        <rect x="2" y="4.6" width="12" height="8.4" rx="1.5" />
        <path d="M5.7 4.6l.9-1.7h2.8l.9 1.7" />
        <circle cx="8" cy="8.7" r="2.4" />
      </>
    ),
    folder: (
      <path d="M2 4.75C2 3.78 2.78 3 3.75 3h2.6l1.5 1.75h4.4c.97 0 1.75.78 1.75 1.75v5.75c0 .97-.78 1.75-1.75 1.75h-8.5C2.78 14 2 13.22 2 12.25v-7.5z" />
    ),
  };
  return (
    <svg
      viewBox="0 0 16 16"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth="1.3"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={`shrink-0 ${className}`}
      aria-hidden="true"
    >
      {paths[kind]}
    </svg>
  );
}

/** 设备列表行的文件数徽标：总文件数千分位；无文件时退回类型中文标签 */
function deviceBadge(d: DeviceSnapshot, t: (key: string) => string): string {
  if (d.mediaPresent === false) return t("wizard.noCard");
  if (d.scanStatus === "scanning") return t("wizard.deviceScanning");
  if (d.scanStatus === "failed") return t("wizard.deviceScanFailed");
  const total = Object.values(d.filesByKind).reduce((sum, n) => sum + n, 0);
  return total > 0 ? `${total.toLocaleString(getIntlLocale())} ${t("wizard.deviceFiles")}` : t(deviceKindLabelKey(d));
}

export default function ImportSourcePicker({ devices, recentSources, selectedId, scanningPath, nativeUnavailable, onDevice, onFolder, onRecent }: {
  devices: DeviceSnapshot[];
  recentSources: RecentSource[];
  selectedId: string | null;
  scanningPath: string | null;
  nativeUnavailable: boolean;
  onDevice: (id: string) => void;
  onFolder: () => void;
  onRecent: (entry: RecentSource) => void;
}) {
  const { t } = useTranslation();
  const recent = recentSources.filter((entry) => entry.kind === "folder" || devices.some((device) => device.id === entry.id && device.mediaPresent !== false));
  return (
    <section className="sp-scroll min-h-0 flex-1 overflow-y-auto px-6 py-8" data-testid="wizard-source-picker">
      <div className="mx-auto flex w-full max-w-4xl flex-col gap-7">
        <div><h2 className="text-2xl font-semibold tracking-tight text-text-primary">{t("wizard.flow.sourceTitle")}</h2><p className="mt-2 max-w-xl text-sm leading-relaxed text-text-secondary">{t("wizard.flow.sourceDesc")}</p></div>
        <button type="button" onClick={onFolder} disabled={scanningPath !== null} className="ui-glass group flex w-full items-center gap-4 rounded-2xl border p-5 text-left transition-colors hover:border-accent disabled:cursor-wait" data-testid="wizard-choose-folder">
          <span className="flex h-12 w-12 shrink-0 items-center justify-center rounded-2xl bg-accent/10 text-accent"><FolderGlyph size={25} /></span>
          <span className="min-w-0 flex-1"><span className="block text-sm font-semibold text-text-primary">{t("wizard.chooseFolder")}</span><span className="mt-1 block text-xs leading-relaxed text-text-muted">{t("wizard.flow.folderDesc")}</span></span>
          {scanningPath !== null ? <span className="h-5 w-5 shrink-0 animate-spin rounded-full border-2 border-edge border-t-accent" aria-hidden="true" /> : <span className="text-lg text-text-muted" aria-hidden="true">→</span>}
        </button>
        {scanningPath !== null && <p role="status" className="break-all rounded-xl bg-panel/50 px-4 py-3 text-xs leading-relaxed text-text-secondary">{t("wizard.flow.folderScanning", { path: scanningPath })}</p>}
        <div><h3 className="mb-3 text-xs font-semibold text-text-secondary">{t("wizard.section.devices")}</h3>
          {devices.length === 0 ? <div className="rounded-2xl border border-dashed border-edge px-5 py-7 text-sm leading-relaxed text-text-muted">{t(nativeUnavailable ? "wizard.noNativeDevice" : "wizard.noDevice")}</div> :
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3" data-testid="wizard-device-list">
              {devices.map((device) => <button key={device.id} type="button" disabled={device.mediaPresent === false} onClick={() => onDevice(device.id)}
                className={`ui-glass flex min-w-0 items-center gap-3 rounded-2xl border p-4 text-left transition-colors hover:border-accent disabled:cursor-not-allowed disabled:opacity-50 ${device.id === selectedId ? "border-accent" : "border-edge"}`}
                style={{ borderColor: device.id === selectedId ? "var(--color-accent)" : undefined }}
                data-testid="wizard-device-item" data-device-id={device.id} data-selected={device.id === selectedId}>
                <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-panel text-text-secondary"><DeviceGlyph kind={devicePresentationKind(device)} size={22} /></span>
                <span className="min-w-0 flex-1"><span className="block truncate text-sm font-medium text-text-primary">{device.name}</span><span className="mt-1 block text-xs text-text-muted">{deviceBadge(device, t)}</span></span>
                <span className="text-text-muted" aria-hidden="true">→</span>
              </button>)}
            </div>}
        </div>
        {recent.length > 0 && <div data-testid="wizard-recent"><h3 className="mb-3 text-xs font-semibold text-text-secondary">{t("wizard.recent.title")}</h3><div className="flex flex-col gap-1">
          {recent.map((entry) => <button key={entry.id} type="button" onClick={() => onRecent(entry)} className="flex min-w-0 items-center gap-3 rounded-xl px-3 py-3 text-left text-xs text-text-secondary hover:bg-panel/70" data-testid="wizard-recent-item">
            <DeviceGlyph kind={entry.kind === "folder" ? "folder" : devicePresentationKind(entry)} size={16} /><span className="min-w-0 flex-1 truncate">{entry.name}</span><span className="hidden max-w-[50%] truncate text-[11px] text-text-muted md:block" title={entry.id}>{entry.id.replace(/^FOLDER:/, "")}</span><span aria-hidden="true">→</span>
          </button>)}
        </div></div>}
      </div>
    </section>
  );
}
