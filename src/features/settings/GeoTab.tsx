import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { subscribeAppEvents, type AppEvent } from "@/ipc/api";
import {
  mapGeoCancel,
  mapGeoDelete,
  mapGeoDownloadStart,
  mapGeoStatus,
  type GeoStatus,
} from "@/ipc/api/map";
import { SectionTitle, SettingRow } from "@/features/settings/pages/SettingsPage";

/**
 * 设置页「地图数据」tab：地理数据包的下载/取消/删除管理（对齐 AI 模型
 * 管理的交互形态——拍摄地图页不再提供下载入口，只跳转到这里）。
 * - 状态行：包是否就绪 + DataV 已装文件数（<250 = 上次中断可补全）
 * - 下载中：进度条（mapGeoProgress 事件驱动）+ 取消
 * - 失败：错误信息 + 重试
 * - 已就绪：删除（两步确认，清数据包与库内地区表）
 */

export default function GeoTab() {
  const { t } = useTranslation();
  const [status, setStatus] = useState<GeoStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const pullStatus = useCallback(() => {
    mapGeoStatus()
      .then(setStatus)
      .catch(() => setStatus(null));
  }, []);

  useEffect(() => {
    pullStatus();
    let disposed = false;
    let unsub: (() => void) | null = null;
    void subscribeAppEvents((event: AppEvent) => {
      if (disposed) return;
      if (event.type === "mapGeoProgress") {
        setStatus((prev) =>
          prev
            ? { ...prev, phase: event.stage, done: event.done, total: event.total, message: event.message }
            : prev,
        );
      }
    }).then((fn) => {
      if (disposed) fn();
      else unsub = fn;
    });
    return () => {
      disposed = true;
      unsub?.();
    };
  }, [pullStatus]);

  async function start(): Promise<void> {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await mapGeoDownloadStart();
      pullStatus();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function cancel(): Promise<void> {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await mapGeoCancel();
      pullStatus();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function remove(): Promise<void> {
    setConfirmDelete(false);
    setBusy(true);
    setError(null);
    try {
      await mapGeoDelete();
      pullStatus();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  if (status === null) {
    return (
      <>
        <SectionTitle>{t("settings.geo.title")}</SectionTitle>
        <div className="py-4 text-xs text-text-muted" data-testid="settings-geo-loading">
          {t("common.loading")}
        </div>
      </>
    );
  }

  const downloading = status.phase === "downloading" || status.phase === "loading";
  const pct = downloading && status.total > 0
    ? Math.min(100, Math.round((status.done / status.total) * 100))
    : 0;
  const failed = status.phase === "failed";
  // DataV 全量 ~375 文件；<250 视为可补全（与后端口径一致）
  const incomplete = status.installed && status.datavFiles < 250;

  return (
    <>
      <SectionTitle>{t("settings.geo.title")}</SectionTitle>
      <SettingRow
        label={t("settings.geo.dataPackage")}
        desc={t("settings.geo.dataPackageDesc")}
        testId="settings-geo-row"
      >
        <div className="flex items-center gap-2">
          {downloading ? (
            <button
              type="button"
              onClick={() => void cancel()}
              disabled={busy}
              className="rounded-md border border-edge px-2.5 py-1 text-[11px] text-text-secondary transition-colors hover:border-red-400 hover:text-red-400 disabled:opacity-40"
              data-testid="settings-geo-cancel"
            >
              {t("settings.geo.cancel")}
            </button>
          ) : confirmDelete ? (
            <>
              <span className="text-[11px] text-red-400">{t("settings.geo.deleteConfirm")}</span>
              <button
                type="button"
                onClick={() => setConfirmDelete(false)}
                className="rounded-md border border-edge px-2.5 py-1 text-[11px] text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
                data-testid="settings-geo-delete-no"
              >
                {t("common.cancel")}
              </button>
              <button
                type="button"
                onClick={() => void remove()}
                disabled={busy}
                className="rounded-md bg-red-500 px-2.5 py-1 text-[11px] font-medium text-white transition-colors hover:brightness-110 disabled:opacity-40"
                data-testid="settings-geo-delete-yes"
              >
                {t("settings.geo.deleteGo")}
              </button>
            </>
          ) : (
            <>
              {status.installed && (
                <button
                  type="button"
                  onClick={() => setConfirmDelete(true)}
                  className="rounded-md border border-edge px-2.5 py-1 text-[11px] text-text-secondary transition-colors hover:border-red-400 hover:text-red-400"
                  data-testid="settings-geo-delete"
                >
                  {t("settings.geo.delete")}
                </button>
              )}
              {(failed || incomplete || !status.installed) && (
                <button
                  type="button"
                  onClick={() => void start()}
                  disabled={busy}
                  className="rounded-md bg-accent px-3 py-1 text-[11px] font-medium text-black transition-colors hover:brightness-110 disabled:opacity-40"
                  data-testid="settings-geo-download"
                >
                  {failed
                    ? t("settings.geo.retry")
                    : incomplete
                      ? t("settings.geo.complete")
                      : t("settings.geo.download")}
                </button>
              )}
            </>
          )}
        </div>
      </SettingRow>

      {/* 状态与进度（行下方整宽说明区） */}
      <div className="border-b border-edge/40 py-2" data-testid="settings-geo-state" data-phase={status.phase}>
        {downloading ? (
          <div className="flex flex-col gap-1.5">
            <div className="flex items-center justify-between text-[11px] text-text-muted">
              <span>{status.phase === "downloading" ? t("settings.geo.downloading") : t("settings.geo.loading")}</span>
              <span className="font-mono tabular-nums" data-testid="settings-geo-pct">{pct}%</span>
            </div>
            <div className="h-1.5 w-full overflow-hidden rounded-full bg-panel">
              <div
                className="h-full rounded-full bg-accent transition-[width] duration-300"
                style={{ width: `${pct}%` }}
                data-testid="settings-geo-bar"
              />
            </div>
          </div>
        ) : failed ? (
          <div className="space-y-1">
            <div className="text-[11px] text-red-400" data-testid="settings-geo-failed">
              {t("settings.geo.failed")}
            </div>
            {status.message && (
              <div className="break-all text-[11px] leading-relaxed text-text-muted" data-testid="settings-geo-failed-message">
                {status.message}
              </div>
            )}
          </div>
        ) : (
          <div className="text-[11px] leading-relaxed text-text-muted" data-testid="settings-geo-summary">
            {status.installed
              ? t("settings.geo.summaryInstalled", {
                  files: status.datavFiles,
                  cache: status.cacheReady ? t("settings.geo.cacheReady") : t("settings.geo.cacheBuilding"),
                })
              : t("settings.geo.summaryMissing")}
            {incomplete && <span className="ml-1 text-amber-400">{t("settings.geo.incomplete")}</span>}
          </div>
        )}
        {error && (
          <div className="mt-1 text-[11px] text-red-400" role="alert" data-testid="settings-geo-error">
            {error}
          </div>
        )}
      </div>
    </>
  );
}
