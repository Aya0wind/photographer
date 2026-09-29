/**
 * 拍摄地图页（/map，组织组 · 器材统计之下）：
 * - 数据管线状态机：未安装 → 下载引导；下载/加载 → 进度；回填 → 地图 +
 *   顶部进度；就绪 → 地图；失败 → 重试
 * - 分层气泡：zoom 驱动切层（全局聚合），点击气泡 flyTo 下钻（parent 限定）
 * - 面包屑（全球 > 中国 > 京省…）+ 换一批（随机样本重拉）+ 回填中提示
 */

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { subscribeAppEvents } from "@/ipc/api/events";
import type { AppEvent } from "@/ipc/api/types";
import { mapGeoStatus, type GeoStatus, type MapCluster } from "@/ipc/api/map";
import { useMotionOn } from "@/lib/motion";

import MapCanvas, { type FlyTarget } from "../components/MapCanvas";
import { zoomForLevel, type MapLevel } from "../lib/hierarchy";
import { useMapClusters } from "../lib/useMapClusters";

interface Crumb {
  id: number;
  name: string;
  lat: number;
  lon: number;
}

export default function MapPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const motionOn = useMotionOn();
  const [status, setStatus] = useState<GeoStatus | null>(null);
  const [level, setLevel] = useState<MapLevel>(0);
  const [crumbs, setCrumbs] = useState<Crumb[]>([]); // 下钻链（空 = 全球）
  const [fly, setFly] = useState<FlyTarget | null>(null);

  const parentId = crumbs.length > 0 ? crumbs[crumbs.length - 1].id : null;
  const { clusters, loading, refresh, invalidateAll } = useMapClusters(level, parentId);

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
        // 进度事件驱动状态快进（比轮询 status 轻省）
        setStatus((prev) =>
          prev
            ? {
                ...prev,
                phase: event.stage,
                done: event.done,
                total: event.total,
                message: event.message,
              }
            : prev,
        );
      }
      if (event.type === "mapRegionsUpdated") {
        invalidateAll();
      }
    }).then((fn) => {
      if (disposed) fn();
      else unsub = fn;
    });
    return () => {
      disposed = true;
      unsub?.();
    };
  }, [pullStatus, invalidateAll]);

  // 下载/重试入口全部收进设置页「地图数据」（2026-09-29 用户定案）：
  // 本页只读展示管线状态 + 跳转。
  const gotoSettings = useCallback(() => navigate("/settings?tab=map"), [navigate]);

  // zoom 驱动切层：回到全局聚合（drill 链清空）
  const onLevelChange = useCallback((next: MapLevel) => {
    setLevel(next);
    setCrumbs([]);
  }, []);

  // 点击气泡：未到县 → 下钻一层（parent 限定 + flyTo）；县级 → 原地放大
  const onDrill = useCallback(
    (cluster: MapCluster) => {
      if (level < 3) {
        const next = (level + 1) as MapLevel;
        setLevel(next);
        setCrumbs((prev) => [
          ...prev,
          { id: cluster.regionId, name: cluster.name, lat: cluster.lat, lon: cluster.lon },
        ]);
        setFly({ lat: cluster.lat, lon: cluster.lon, zoom: zoomForLevel(next), nonce: Date.now() });
      } else {
        setFly({ lat: cluster.lat, lon: cluster.lon, zoom: 12.5, nonce: Date.now() });
      }
    },
    [level],
  );

  /** 面包屑回退：crumbs[i] 是 level i 的节点，回到它 = 显示其子层（level i+1） */
  const backTo = useCallback((index: number, crumb: Crumb) => {
    setCrumbs((prev) => prev.slice(0, index + 1));
    const next = Math.min(3, index + 1) as MapLevel;
    setLevel(next);
    setFly({ lat: crumb.lat, lon: crumb.lon, zoom: zoomForLevel(next), nonce: Date.now() });
  }, []);

  if (!status) {
    return (
      <div className="h-full" data-testid="map-page">
        <div className="flex h-full items-center justify-center text-xs text-text-muted" data-testid="map-loading">
          {t("common.loading")}
        </div>
      </div>
    );
  }

  if (!status.installed) {
    return (
      <div className="h-full" data-testid="map-page">
        <div className="flex h-full flex-col items-center justify-center gap-4 px-8">
          <div className="flex h-14 w-14 items-center justify-center rounded-2xl border border-edge bg-surface">
            <svg viewBox="0 0 24 24" width="28" height="28" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" className="text-accent" aria-hidden="true">
              <circle cx="12" cy="12" r="9" />
              <path d="M3 12h18M12 3c2.5 2.7 3.8 5.7 3.8 9s-1.3 6.3-3.8 9c-2.5-2.7-3.8-5.7-3.8-9s1.3-6.3 3.8-9z" />
            </svg>
          </div>
          <div className="text-sm font-semibold">{t("map.downloadTitle")}</div>
          <p className="max-w-md text-center text-xs leading-relaxed text-text-muted" data-testid="map-download-hint">
            {t("map.downloadHint")}
          </p>
          <button
            type="button"
            onClick={gotoSettings}
            className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-white transition-opacity hover:opacity-90"
            data-testid="map-download-start"
          >
            {t("map.gotoSettings")}
          </button>
        </div>
      </div>
    );
  }

  if (status.phase === "downloading" || status.phase === "loading") {
    const pct = status.total > 0 ? Math.min(100, Math.round((status.done / status.total) * 100)) : 0;
    return (
      <div className="h-full" data-testid="map-page">
        <div className="flex h-full flex-col items-center justify-center gap-3">
          <div className="animate-pulse text-sm font-semibold" data-testid="map-preparing">
            {status.phase === "downloading" ? t("map.downloading") : t("map.loading")}
          </div>
          <div className="h-1.5 w-64 overflow-hidden rounded-full bg-surface">
            <div className="h-full rounded-full bg-accent transition-[width] duration-300" style={{ width: `${pct}%` }} />
          </div>
          <div className="text-[11px] text-text-muted">{pct}%</div>
        </div>
      </div>
    );
  }

  if (status.phase === "failed") {
    return (
      <div className="h-full" data-testid="map-page">
        <div className="flex h-full flex-col items-center justify-center gap-3">
          <div className="text-sm text-red-400" data-testid="map-failed">
            {t("map.failed")}
          </div>
          {status.message && <div className="max-w-md text-center text-[11px] text-text-muted">{status.message}</div>}
          <button
            type="button"
            onClick={gotoSettings}
            className="rounded-md border border-edge px-4 py-1.5 text-xs text-text-muted transition-colors hover:border-accent hover:text-accent"
            data-testid="map-retry"
          >
            {t("map.gotoSettings")}
          </button>
        </div>
      </div>
    );
  }

  // 回填中/就绪：地图视图
  const backfilling = status.phase === "backfilling";
  const backfillPct = backfilling && status.total > 0 ? Math.min(100, Math.round((status.done / status.total) * 100)) : 0;

  return (
    <div className="flex h-full flex-col" data-testid="map-page">
      <div className="flex shrink-0 items-center gap-2 px-4 pt-3">
        <h1 className="text-sm font-semibold text-text-primary">{t("map.title")}</h1>
        {/* 面包屑：全球 > 国家 > 省 …（点击回退） */}
        <nav className="flex min-w-0 items-center gap-1 text-[11px] text-text-muted" data-testid="map-breadcrumb">
          <button type="button" className="rounded px-1 hover:text-accent" onClick={() => { setCrumbs([]); setLevel(0); setFly({ lat: 104, lon: 35, zoom: 1.5, nonce: Date.now() }); }}>
            {t("map.level.world")}
          </button>
          {crumbs.map((crumb, i) => (
            <span key={crumb.id} className="flex items-center gap-1">
              <span className="opacity-40">/</span>
              <button type="button" className="rounded px-1 truncate max-w-28 hover:text-accent" onClick={() => backTo(i, crumb)}>
                {crumb.name}
              </button>
            </span>
          ))}
        </nav>
        <div className="ml-auto flex items-center gap-2">
          {backfilling && (
            <span className="text-[11px] text-text-muted" data-testid="map-backfill-hint">
              {t("map.backfilling")} {backfillPct}%
            </span>
          )}
          {status.datavFiles < 250 && (
            <button
              type="button"
              onClick={gotoSettings}
              className="rounded-md border border-accent/40 px-2 py-1 text-[11px] text-accent transition-colors hover:border-accent"
              data-testid="map-complete-download"
            >
              {t("map.completeDownload")}
            </button>
          )}
          <button
            type="button"
            onClick={refresh}
            className="rounded-md border border-edge px-2 py-1 text-[11px] text-text-muted transition-colors hover:border-accent hover:text-accent"
            data-testid="map-refresh"
          >
            {t("map.refreshSamples")}
          </button>
        </div>
      </div>
      {backfilling && (
        <div className="mx-4 mt-2 h-0.5 shrink-0 overflow-hidden rounded-full bg-surface" data-testid="map-backfill-bar">
          <div className="h-full bg-accent transition-[width] duration-300" style={{ width: `${backfillPct}%` }} />
        </div>
      )}
      <div className="relative min-h-0 flex-1 overflow-hidden px-4 pb-4 pt-2">
        <div className="h-full w-full overflow-hidden rounded-xl border border-edge" style={{ background: "#0E0F12" }}>
          <MapCanvas
            clusters={clusters}
            level={level}
            onLevelChange={onLevelChange}
            onDrill={onDrill}
            flyTarget={fly}
            animationsOn={motionOn}
          />
        </div>
        {!loading && clusters.length === 0 && (
          <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
            <div className="rounded-lg border border-edge bg-panel/90 px-6 py-4 text-center text-xs text-text-muted backdrop-blur" data-testid="map-empty">
              {t("map.empty")}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
