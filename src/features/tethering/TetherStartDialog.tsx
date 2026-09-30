import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  tetheringCameraList,
  tetheringStart,
  type CameraInfo,
} from "@/ipc/api";
import { canTriggerCapture, tetheringChips } from "@/features/import/tetheringCapabilities";

/**
 * 联机拍摄启动弹窗（相册详情页入口）：选择相机 → tethering_start（后端开
 * 独立拍摄窗口）。无可联拍相机/启动失败都在弹窗内提示，不惊动页面。
 */
export default function TetherStartDialog({
  albumId,
  albumName,
  onClose,
}: {
  albumId: number;
  albumName: string;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [cameras, setCameras] = useState<CameraInfo[] | null>(null);
  const [starting, setStarting] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setCameras(null);
    setCameras(await tetheringCameraList());
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  async function start(camera: CameraInfo): Promise<void> {
    setStarting(camera.pnpId);
    setError(null);
    const result = await Promise.resolve(tetheringStart(albumId, camera.pnpId)).catch(() => null);
    setStarting(null);
    if (result?.ok) {
      onClose(); // 拍摄窗口已由后端打开
    } else {
      setError(result?.error ?? t("albums.tetherStartFailed"));
    }
  }

  return (
    <div
      className="fixed inset-0 z-[70] flex items-center justify-center bg-black/60"
      onClick={onClose}
      role="dialog"
      data-testid="tether-start-dialog"
    >
      <div
        className="w-[440px] rounded-xl border border-edge bg-surface p-5"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="text-sm font-semibold text-text-primary">
          {t("albums.tetherDialogTitle", { name: albumName })}
        </h2>

        <div className="mt-3 flex items-center justify-between">
          <span className="text-xs text-text-secondary">
            {cameras === null ? t("albums.tetherScanning") : `${cameras.length}`}
          </span>
          <button
            type="button"
            onClick={() => void refresh()}
            disabled={cameras === null}
            className="rounded border border-edge px-2 py-0.5 text-[10px] text-text-secondary transition-colors hover:border-accent hover:text-accent disabled:opacity-40"
            data-testid="tether-start-refresh"
          >
            {t("albums.tetherRefresh")}
          </button>
        </div>

        <div className="mt-2 flex max-h-64 flex-col gap-2 overflow-y-auto" data-testid="tether-start-list">
          {cameras !== null && cameras.length === 0 && (
            <div>
              <p className="rounded-md border border-edge bg-bg p-3 text-[11px] leading-relaxed text-text-muted" data-testid="tether-start-empty">
                {t("albums.tetherNoCamera")}
              </p>
              <p className="mt-1.5 text-[11px] leading-relaxed text-text-muted" data-testid="tether-start-sony-hint">
                {t("albums.tetherSonyHint")}
              </p>
            </div>
          )}
          {(cameras ?? []).map((camera) => {
            const capable = canTriggerCapture(camera.capabilities);
            return (
              <button
                key={camera.pnpId}
                type="button"
                disabled={!capable || starting !== null}
                onClick={() => void start(camera)}
                title={capable ? undefined : t("albums.tetherCameraUnsupported")}
                className="flex flex-col gap-1 rounded-md border border-edge bg-bg p-2.5 text-left transition-colors hover:border-accent disabled:cursor-not-allowed disabled:opacity-40"
                data-testid={`tether-start-camera-${camera.pnpId}`}
              >
                <span className="flex items-center justify-between text-xs text-text-primary">
                  <span className="truncate">{camera.name}</span>
                  {starting === camera.pnpId && (
                    <span className="text-[10px] text-accent">{t("albums.tetherScanning")}</span>
                  )}
                </span>
                <span className="flex flex-wrap gap-1">
                  {tetheringChips(camera.capabilities).map((chip) => (
                    <span
                      key={chip.id}
                      className={`rounded-full px-1.5 py-0.5 text-[10px] ${chip.supported ? "bg-accent/15 text-accent" : "bg-panel text-text-muted"}`}
                    >
                      {t(`deviceDialog.tethering.${chip.id === "triggerCapture" ? "triggerCapture" : chip.id === "autoIngest" ? "autoIngest" : "fileTransfer"}`)}
                    </span>
                  ))}
                </span>
              </button>
            );
          })}
        </div>

        {error !== null && (
          <p className="mt-2 text-[11px] text-red-400" role="alert" data-testid="tether-start-error">
            {error}
          </p>
        )}

        <div className="mt-4 flex justify-end">
          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
            data-testid="tether-start-cancel"
          >
            {t("common.cancel")}
          </button>
        </div>
      </div>
    </div>
  );
}
