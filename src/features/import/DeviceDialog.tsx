import { useNavigate } from "react-router";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion } from "motion/react";

import { formatBytes } from "@/lib/format";
import { useImportStore } from "@/stores/importStore";
import type { FileKind } from "@/ipc/api";
import { deviceKindLabelKey, devicePresentationKind, type DevicePresentationKind } from "./devicePresentation";

/**
 * 设备就绪弹窗（B 简洁风格）：deviceScanned 事件把设备推入 promptQueue，
 * 本组件逐个弹出居中卡片——设备名 + 类型徽标、按类型统计（照片/RAW/视频
 * 分色数字）、总量与新增数、「开始导入」「忽略」。多设备同屏时排队逐个弹。
 */

const KIND_BADGE: Record<FileKind, string> = {
  photo: "text-accent",
  raw: "text-sky-400",
  video: "text-violet-400",
  other: "text-text-muted",
};

function DeviceGlyph({ kind }: { kind: DevicePresentationKind }) {
  const stroke = { fill: "none", stroke: "currentColor", strokeWidth: 1.4, strokeLinecap: "round", strokeLinejoin: "round" } as const;
  return (
    <svg viewBox="0 0 24 24" width="28" height="28" aria-hidden="true" className="text-text-secondary">
      {kind === "camera" ? (
        <g {...stroke}>
          <rect x="3.5" y="7" width="17" height="12" rx="2" />
          <path d="M9 7l1.2-2.4h3.6L15 7" />
          <circle cx="12" cy="13" r="3.2" />
        </g>
      ) : kind === "folder" ? (
        <g {...stroke}>
          <path d="M3.5 7.5c0-1.1.9-2 2-2h4l2 2.5h7c1.1 0 2 .9 2 2v8.5c0 1.1-.9 2-2 2h-13c-1.1 0-2-.9-2-2v-11z" />
        </g>
      ) : (
        <g {...stroke}>
          <rect x="5" y="4.5" width="14" height="15" rx="2" />
          <rect x="8.5" y="2.5" width="7" height="3.4" rx="1" />
          <path d="M8.5 13.5l2.2 2.2 4.4-4.4" />
        </g>
      )}
    </svg>
  );
}

export default function DeviceDialog() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const queue = useImportStore((s) => s.promptQueue);
  const devices = useImportStore((s) => s.devices);
  const ignoreDevice = useImportStore((s) => s.ignoreDevice);

  const currentId = queue[0];
  const device = devices.find((d) => d.id === currentId) ?? null;
  const waitingCount = Math.max(0, queue.length - (device ? 1 : 0));

  const stats: { kind: FileKind; count: number }[] = device
    ? (["photo", "raw", "video"] as FileKind[]).map((kind) => ({
        kind,
        count: device.filesByKind?.[kind] ?? 0,
      }))
    : [];

  return (
    <AnimatePresence>
      {device && (
        <motion.div
          key={device.id}
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.18, ease: "easeOut" }}
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
          role="dialog"
          aria-modal="true"
          aria-label={t("deviceDialog.title")}
        >
          <motion.div
            initial={{ opacity: 0, y: 12 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: 8 }}
            transition={{ duration: 0.18, ease: "easeOut" }}
            className="w-[380px] rounded-xl border border-edge bg-surface p-6 shadow-2xl"
            data-testid="device-dialog-card"
          >
            <div className="flex items-center gap-3">
              <DeviceGlyph kind={devicePresentationKind(device)} />
              <div className="min-w-0 flex-1">
                <h2 className="truncate text-base font-semibold text-text-primary" title={device.name}>
                  {device.name}
                </h2>
                <span className="mt-0.5 inline-block rounded bg-panel px-1.5 py-0.5 text-[11px] text-text-secondary">
                  {t(deviceKindLabelKey(device))}
                </span>
              </div>
            </div>

            <div className="mt-5 grid grid-cols-3 gap-2" data-testid="device-dialog-stats">
              {stats.map(({ kind, count }) => (
                <div key={kind} className="rounded-lg bg-bg px-3 py-2.5 text-center">
                  <div className={`text-lg font-semibold tabular-nums ${KIND_BADGE[kind]}`}>{count}</div>
                  <div className="mt-0.5 text-xs text-text-muted">{t(`deviceDialog.files.${kind}`)}</div>
                </div>
              ))}
            </div>

            <div className="mt-4 flex items-center justify-between text-xs text-text-secondary">
              <span>
                {t("deviceDialog.totalSize")}
                <span className="ml-1.5 font-mono text-text-primary">{formatBytes(device.bytesTotal)}</span>
              </span>
              <span className="rounded-md bg-accent/10 px-2 py-1 text-accent" data-testid="device-dialog-new">
                {t("deviceDialog.newFiles", { count: device.newFiles })}
              </span>
            </div>

            <div className="mt-6 flex gap-2.5">
              <button
                type="button"
                autoFocus
                onClick={() => {
                  ignoreDevice(device.id);
                  navigate(`/import?device=${encodeURIComponent(device.id)}`);
                }}
                className="flex-1 rounded-md bg-accent px-4 py-2 text-sm font-medium text-black transition-colors hover:brightness-110"
              >
                {t("deviceDialog.startImport")}
              </button>
              <button
                type="button"
                onClick={() => ignoreDevice(device.id)}
                className="rounded-md border border-edge px-4 py-2 text-sm text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
              >
                {t("deviceDialog.ignore")}
              </button>
            </div>

            {waitingCount > 0 && (
              <p className="mt-3 text-center text-xs text-text-muted">
                {t("deviceDialog.waiting", { count: waitingCount })}
              </p>
            )}
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
