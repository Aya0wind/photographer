import { convertFileSrc } from "@tauri-apps/api/core";

import { ipc } from "../index";

/** 地理数据管线状态（map_geo_status） */
export interface GeoStatus {
  /** 三根文件是否就绪（世界两国界包 + DataV 省级根） */
  installed: boolean;
  /** notInstalled / downloading / loading / ready / backfilling / failed */
  phase: string;
  done: number;
  total: number;
  message: string | null;
  /** 树缓存 JSON 可读（前端直读提速） */
  cacheReady: boolean;
  /** DataV 市县文件已装数（<250 = 上次中断可补全） */
  datavFiles: number;
}

/** 聚类气泡代表图样本 */
export interface ClusterSample {
  id: number;
  path: string;
  kind: string;
}

/** 分层聚合气泡（map_clusters） */
export interface MapCluster {
  regionId: number;
  name: string;
  lat: number;
  lon: number;
  count: number;
  samples: ClusterSample[];
}

/** 树缓存 JSON 行（regions-cache.json；扁平 id/parent 结构） */
export interface RegionCacheRow {
  id: number;
  parent: number | null;
  level: number;
  name: string;
  code: string;
  lat: number;
  lon: number;
}

export async function mapGeoStatus(): Promise<GeoStatus> {
  return ipc<GeoStatus>("map_geo_status");
}

/** 触发数据包下载（幂等；失败抛错给引导 UI 显示） */
export async function mapGeoDownloadStart(): Promise<void> {
  await ipc<void>("map_geo_download_start");
}

/**
 * 树缓存直读（convertFileSrc → fetch 本地文件）。未就绪/读失败回 null，
 * 调用方降级（层级名走 clusters 自带 name）。
 */
export async function mapRegionTree(): Promise<RegionCacheRow[] | null> {
  try {
    const url = await ipc<string | null>("map_geo_cache_url");
    if (!url) return null;
    const res = await fetch(convertFileSrc(url));
    if (!res.ok) return null;
    const rows = (await res.json()) as RegionCacheRow[];
    return Array.isArray(rows) ? rows : null;
  } catch {
    return null;
  }
}

/** 分层聚合：level 0 国 /1 省 /2 市 /3 县；parent 限定下钻范围 */
export async function mapClusters(
  level: number,
  parentRegionId: number | null,
): Promise<MapCluster[]> {
  return ipc<MapCluster[]>("map_clusters", {
    level,
    parentRegionId: parentRegionId ?? undefined,
  });
}
