/**
 * maplibre 画布封装（MapPage 专用，Phase 3/4）：
 * - 初始化 OpenFreeMap 暗色矢量底图（免费无 key；WebGL 本地渲染）
 * - zoom 防抖 250ms → onLevelChange（飞行途中不抖动切层）
 * - clusters 变化全量重建 marker（气泡 DOM 命令式构造，PhotoBubble 工厂）
 * - flyTarget 变化 → flyTo 丝滑飞行（1.2s，曲线 1.42）
 * - maplibre-gl 样式与全局 .no-motion 契约兼容（气泡动画纯 CSS）
 */

import { useEffect, useRef } from "react";
import * as maplibregl from "maplibre-gl";
import workerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?url";
import "maplibre-gl/dist/maplibre-gl.css";

import { convertFileSrc } from "@tauri-apps/api/core";

import { assetThumbGet } from "@/ipc/api/assets";
import type { MapCluster } from "@/ipc/api/map";

import { levelForZoom, type MapLevel } from "../lib/hierarchy";
import { createBubbleElement } from "./PhotoBubble";

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
  flyTarget: FlyTarget | null;
  /** 全局动画开关（settings.appearance.animations）：关闭时 flyTo 瞬时 */
  animationsOn: boolean;
}

const STYLE_URL = "https://tiles.openfreemap.org/styles/dark";

export default function MapCanvas({
  clusters,
  level,
  onLevelChange,
  onDrill,
  flyTarget,
  animationsOn,
}: MapCanvasProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const mapRef = useRef<maplibregl.Map | null>(null);
  const markersRef = useRef<maplibregl.Marker[]>([]);
  const levelRef = useRef(level);
  const onLevelChangeRef = useRef(onLevelChange);
  const onDrillRef = useRef(onDrill);
  levelRef.current = level;
  onLevelChangeRef.current = onLevelChange;
  onDrillRef.current = onDrill;

  // 初始化（一次）：底图 + zoom 防抖切层
  useEffect(() => {
    if (!containerRef.current || mapRef.current) return;
    const map = new maplibregl.Map({
      container: containerRef.current,
      style: STYLE_URL,
      center: [104, 35],
      zoom: 1.5,
      attributionControl: { compact: true },
    });
    mapRef.current = map;
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
      map.remove();
      mapRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 气泡同步：全清重建（随机样本本来就要换；400 内重建 ~10ms）
  useEffect(() => {
    const map = mapRef.current;
    if (!map) return;
    markersRef.current.forEach((m) => m.remove());
    markersRef.current = clusters.map((cluster, index) => {
      const element = createBubbleElement(cluster, index, () => onDrillRef.current(cluster));
      const marker = new maplibregl.Marker({ element })
        .setLngLat([cluster.lon, cluster.lat])
        .addTo(map);
      return marker;
    });
  }, [clusters]);

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

  return <div ref={containerRef} className="h-full w-full" data-testid="map-canvas" />;
}
