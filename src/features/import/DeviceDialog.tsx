import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion } from "motion/react";

import { formatBytes } from "@/lib/format";
import { useImportStore } from "@/stores/importStore";
import {
  cameraCapture,
  cameraProbe,
  subscribeAppEvents,
  type CameraInfo,
  type FileKind,
} from "@/ipc/api";
import { deviceKindLabelKey, devicePresentationKind, type DevicePresentationKind } from "./devicePresentation";
import { canTriggerCapture, sameCameraDevice, tetheringChips } from "./tetheringCapabilities";

/**
 * 设备就绪弹窗（B 简洁风格）：deviceScanned 事件把设备推入 promptQueue，
 * 本组件逐个弹出居中卡片——设备名 + 类型徽标、按类型统计（照片/RAW
 * 分色数字）、总量与新增数、「开始导入」「忽略」。多设备同屏时排队逐个弹。
 *
 * 阶段 E-1 联拍区：相机设备额外显示能力 chips（文件传输/可触发拍摄/自动收片，
 * 进入弹窗懒探测一次并缓存到组件态；未探测显示「未探测」），有拍摄能力时
 * 行尾「拍摄」按钮（camera_capture → toast → refreshDevice 刷新快照）；
 * tetheringObjectAdded 事件命中当前设备时同样刷新（轻度：事件到达即刷新）。
 */

const KIND_BADGE: Record<FileKind, string> = {
  photo: "text-accent",
  raw: "text-sky-400",
  other: "text-text-muted",
};

/** 拍摄成功/失败 toast 的驻留时长 */
const TOAST_AUTO_DISMISS_MS = 4000;

type TetherToast = { kind: "ok" | "err"; text: string };

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
  const refreshDevice = useImportStore((s) => s.refreshDevice);

  const currentId = queue[0];
  const device = devices.find((d) => d.id === currentId) ?? null;
  const waitingCount = Math.max(0, queue.length - (device ? 1 : 0));

  // --- 联拍（阶段 E-1） ---------------------------------------------------------
  const isCamera = device !== null && devicePresentationKind(device) === "camera";
  /** 探测缓存（组件态）：deviceId → CameraInfo；null = 已请求未回或探测失败（未知） */
  const [probeCache, setProbeCache] = useState<Record<string, CameraInfo | null>>({});
  const cameraInfo = currentId !== undefined ? probeCache[currentId] ?? null : null;
  const [capturing, setCapturing] = useState(false);
  const [toast, setToast] = useState<TetherToast | null>(null);

  // 进入弹窗（设备行展开/进入）懒探测一次；结果缓存到组件态（ref 查重 + state 驱动渲染，
  // 避免「写占位 → 依赖变化 → cleanup 取消在途探测」的自取消环），后端在途时保持「未探测」
  const probeCacheRef = useRef<Record<string, CameraInfo | null>>({});
  useEffect(() => {
    if (!isCamera || currentId === undefined) return;
    const cache = probeCacheRef.current;
    if (currentId in cache) return;
    cache[currentId] = null; // 占位：已请求未回（未知）
    setProbeCache({ ...cache });
    void cameraProbe(currentId).then((info) => {
      probeCacheRef.current[currentId] = info;
      setProbeCache({ ...probeCacheRef.current });
    });
  }, [isCamera, currentId]);

  // 切换弹出的设备时清拍摄态与 toast
  useEffect(() => {
    setCapturing(false);
    setToast(null);
  }, [currentId]);

  // toast 驻留后自动消失
  useEffect(() => {
    if (toast === null) return;
    const timer = setTimeout(() => setToast(null), TOAST_AUTO_DISMISS_MS);
    return () => clearTimeout(timer);
  }, [toast]);

  // tetheringObjectAdded：当前弹窗设备收到事件即刷新快照（轻度，不做去重）
  const currentIdRef = useRef<string | null>(null);
  useEffect(() => {
    currentIdRef.current = currentId ?? null;
  }, [currentId]);
  useEffect(() => {
    let disposed = false;
    let dispose = () => {};
    void subscribeAppEvents((event) => {
      if (event.type !== "tetheringObjectAdded") return;
      const id = currentIdRef.current;
      if (id === null || !sameCameraDevice(event.pnpId, id)) return;
      void refreshDevice(id);
    }).then((unlisten) => {
      if (disposed) unlisten();
      else dispose = unlisten;
    });
    return () => {
      disposed = true;
      dispose();
    };
  }, [refreshDevice]);

  async function handleCapture(): Promise<void> {
    if (!device || capturing) return;
    setCapturing(true);
    const result = await cameraCapture(device.id);
    setCapturing(false);
    if (result.error === null && result.objectName !== null) {
      setToast({ kind: "ok", text: t("deviceDialog.tethering.captured", { name: result.objectName }) });
      void refreshDevice(device.id);
    } else {
      setToast({ kind: "err", text: result.error ?? t("deviceDialog.tethering.captureFailed") });
    }
  }

  const stats: { kind: FileKind; count: number }[] = device
    ? (["photo", "raw"] as FileKind[]).map((kind) => ({
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

            {/* 联拍区（阶段 E-1）：仅相机设备显示；读卡器/文件夹不涉及 */}
            {isCamera && (
              <div className="mt-4" data-testid="device-dialog-tethering">
                <div
                  className="flex flex-wrap items-center gap-1.5"
                  aria-label={t("deviceDialog.tethering.capabilities")}
                >
                  {cameraInfo === null ? (
                    <span
                      className="rounded bg-panel px-1.5 py-0.5 text-[11px] text-text-muted"
                      data-testid="device-dialog-cap-unprobed"
                    >
                      {t("deviceDialog.tethering.unprobed")}
                    </span>
                  ) : (
                    <>
                      {tetheringChips(cameraInfo.capabilities).map((chip) => (
                        <span
                          key={chip.id}
                          data-testid="device-dialog-cap"
                          data-capability={chip.id}
                          data-supported={chip.supported}
                          className={`rounded px-1.5 py-0.5 text-[11px] ${
                            chip.supported
                              ? "bg-accent/10 text-accent"
                              : "bg-panel text-text-muted opacity-70"
                          }`}
                        >
                          {t(`deviceDialog.tethering.${chip.id}`)}
                        </span>
                      ))}
                      {/* 行尾拍摄按钮：有触发拍摄能力才出现（能力全 false 时无拍摄入口） */}
                      {canTriggerCapture(cameraInfo.capabilities) && (
                        <button
                          type="button"
                          onClick={() => void handleCapture()}
                          disabled={capturing}
                          className="ml-auto flex shrink-0 items-center gap-1 rounded-md border border-accent/60 px-2.5 py-1 text-[11px] font-medium text-accent transition-colors hover:bg-accent/10 disabled:cursor-not-allowed disabled:opacity-50"
                          data-testid="device-dialog-capture"
                        >
                          {capturing && (
                            <span className="h-3 w-3 animate-spin rounded-full border border-accent border-t-transparent" />
                          )}
                          {capturing
                            ? t("deviceDialog.tethering.capturing")
                            : t("deviceDialog.tethering.capture")}
                        </button>
                      )}
                    </>
                  )}
                </div>
                {toast !== null && (
                  <p
                    role="status"
                    data-testid="device-dialog-toast"
                    className={`mt-2 text-xs ${toast.kind === "ok" ? "text-accent" : "text-red-400"}`}
                  >
                    {toast.text}
                  </p>
                )}
              </div>
            )}

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
