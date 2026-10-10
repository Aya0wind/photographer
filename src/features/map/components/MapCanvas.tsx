/**
 * maplibre 画布封装（MapPage 专用，Phase 3/4）：
 * - 初始化内置行政区离线底图（无网络依赖；WebGL 本地渲染）
 * - zoom 防抖 250ms → onLevelChange（飞行途中不抖动切层）
 * - clusters 变化全量重建 marker（气泡 DOM 命令式构造，PhotoBubble 工厂）
 * - flyTarget 变化 → flyTo 丝滑飞行（1.2s，曲线 1.42）
 * - maplibre-gl 样式与全局 .no-motion 契约兼容（气泡动画纯 CSS）
 */

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import * as maplibregl from "maplibre-gl";
import workerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?url";
import "maplibre-gl/dist/maplibre-gl.css";

import { convertFileSrc } from "@tauri-apps/api/core";

import { assetThumbGet } from "@/ipc/api/assets";
import type { MapCluster } from "@/ipc/api/map";

import { levelForZoom, type MapLevel } from "../lib/hierarchy";
import { createBubbleElement } from "./PhotoBubble";
import { attachOfflineBasemap, offlineMapStyle } from "../lib/offlineBasemap";

// maplibre v6 的 worker 是独立文件，URL 由 import.meta.url 动态拼接——
// vite 预打包探测不到（deps 目录里没有 worker 文件 → 404 → Worker failed to load →
// 样式永不完成 → 黑底图）。?url 让 dev/build 都拿到真实资源地址（2026-09-29 真机黑图修复）。
maplibregl.setWorkerUrl(workerUrl);

export interface FlyTarget {
  lat: number;
  lon: number;
  zoom: number;
  /** 递增数：同目标重飞也触发 effect */
  nonce: number;
}

interface MapCanvasProps {
  clusters: MapCluster[];
  level: MapLevel;
  onLevelChange: (level: MapLevel) => void;
  onDrill: (cluster: MapCluster) => void;
  /** 气泡「N 张 →」角标 / 样图点击 → 打开该地区照片子页（MapPage 导航） */
  onOpenPhotos?: (regionId: number) => void;
  flyTarget: FlyTarget | null;
  /** 全局动画开关（settings.appearance.animations）：关闭时 flyTo 瞬时 */
  animationsOn: boolean;
  /** 索引就绪后再加载详细行政区；准备期间仍显示本地世界轮廓。 */
  basemapReady?: boolean;
}

export default function MapCanvas({
  clusters,
  level,
  onLevelChange,
  onDrill,
  onOpenPhotos,
  flyTarget,
  animationsOn,
  basemapReady = true,
}: MapCanvasProps) {
  const { t } = useTranslation();
  const [basemapFailed, setBasemapFailed] = useState(false);
  const reloadBasemapRef = useRef<(() => void) | null>(null);
  const basemapControllerRef = useRef<ReturnType<typeof attachOfflineBasemap> | null>(null);
  const basemapReadyRef = useRef(basemapReady);
  basemapReadyRef.current = basemapReady;
  const containerRef = useRef<HTMLDivElement | null>(null);
  const mapRef = useRef<maplibregl.Map | null>(null);
  const markersRef = useRef<maplibregl.Marker[]>([]);
  const levelRef = useRef(level);
  const onLevelChangeRef = useRef(onLevelChange);
  const onDrillRef = useRef(onDrill);
  const onOpenPhotosRef = useRef(onOpenPhotos);
  levelRef.current = level;
  onLevelChangeRef.current = onLevelChange;
  onDrillRef.current = onDrill;
  onOpenPhotosRef.current = onOpenPhotos;

  // 初始化（一次）：底图 + zoom 防抖切层
  useEffect(() => {
    if (!containerRef.current || mapRef.current) return;
    const map = new maplibregl.Map({
      container: containerRef.current,
      style: offlineMapStyle(),
      localIdeographFontFamily: "sans-serif",
      center: [104, 35],
      zoom: 1.5,
      attributionControl: { compact: true },
    });
    mapRef.current = map;
    const basemap = attachOfflineBasemap(map, setBasemapFailed, basemapReadyRef.current);
    basemapControllerRef.current = basemap;
    reloadBasemapRef.current = basemap.reload;
    if (import.meta.env.DEV) {
      // DEV 探针：CDP 验收用（真机黑帧排查），生产不打包
      (window as unknown as { __map?: maplibregl.Map }).__map = map;
      map.on("error", (e) => console.error("[maplibre]", e.error?.message ?? e.error ?? e));
    }
    map.addControl(new maplibregl.NavigationControl({ showCompass: false }), "bottom-right");
    let timer: ReturnType<typeof setTimeout> | null = null;
    map.on("zoomend", () => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => {
        const next = levelForZoom(map.getZoom());
        if (next !== levelRef.current) onLevelChangeRef.current(next);
      }, 250);
    });
    return () => {
      if (timer) clearTimeout(timer);
      markersRef.current.forEach((m) => m.remove());
      markersRef.current = [];
      basemap.dispose();
      basemapControllerRef.current = null;
      reloadBasemapRef.current = null;
      map.remove();
      mapRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    basemapControllerRef.current?.setReady(basemapReady);
  }, [basemapReady]);

  // 气泡同步：全清重建（随机样本本来就要换；400 内重建 ~10ms）
  useEffect(() => {
    const map = mapRef.current;
    if (!map) return;
    markersRef.current.forEach((m) => m.remove());
    markersRef.current = clusters.map((cluster, index) => {
      const element = createBubbleElement(cluster, index, () => onDrillRef.current(cluster), {
        onOpenPhotos: (regionId) => onOpenPhotosRef.current?.(regionId),
        photosLabel: t("map.badge.photos", { count: cluster.count }),
      });
      const marker = new maplibregl.Marker({ element })
        .setLngLat([cluster.lon, cluster.lat])
        .addTo(map);
      return marker;
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [clusters, t]);

  // 代表图懒加载（marker 挂载后异步补 src；失败保持占位底色）
  useEffect(() => {
    const elements = [...document.querySelectorAll<HTMLImageElement>(".map-bubble-photo[data-asset]")];
    for (const img of elements) {
      const assetId = Number(img.dataset.asset);
      if (!Number.isFinite(assetId) || img.dataset.loaded === "1") continue;
      img.dataset.loaded = "1";
      void assetThumbGet(assetId, 256)
        .then((r) => {
          if (r.status === "ready") {
            img.src = convertFileSrc(r.path);
            img.classList.add("map-bubble-loaded");
          }
        })
        .catch(() => {
          /* 占位底色即可 */
        });
    }
  }, [clusters]);

  // 飞行指令（下钻/回退）；动画关闭时瞬时（.no-motion 契约）
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !flyTarget) return;
    // 非法坐标防御：宁可不动也不要让 maplibre 抛 Invalid LngLat 炸掉路由
    if (!Number.isFinite(flyTarget.lat) || !Number.isFinite(flyTarget.lon)) return;
    map.flyTo({
      center: [flyTarget.lon, flyTarget.lat],
      zoom: flyTarget.zoom,
      duration: animationsOn ? 1200 : 0,
      curve: 1.42,
      essential: true,
    });
  }, [flyTarget, animationsOn]);

  return (
    <div className="relative h-full w-full">
      <div ref={containerRef} className="h-full w-full" data-testid="map-canvas" />
      {basemapFailed && (
        <div className="ui-glass absolute bottom-4 left-4 flex items-center gap-3 rounded-xl border px-3 py-2 text-xs text-text-secondary" role="alert">
          <span>{t("map.failed")}</span>
          <button type="button" className="text-accent" onClick={() => reloadBasemapRef.current?.()}>{t("map.retry")}</button>
        </div>
      )}
    </div>
  );
}
