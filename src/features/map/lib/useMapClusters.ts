import { useCallback, useEffect, useRef, useState } from "react";

import { mapClusters, type MapCluster } from "@/ipc/api/map";

import type { MapLevel } from "./hierarchy";

/**
 * 聚合气泡数据钩子：按 (level, parentId) 拉取；会话内缓存防来回切层重复
 * 请求；`refresh()` 重新拉当前层（「换一批」——后端 ORDER BY random()，
 * 同层样本即换）。regions-updated 事件由页面层调用 refreshAll 处理。
 */
export function useMapClusters(level: MapLevel, parentId: number | null) {
  const cacheRef = useRef(new Map<string, MapCluster[]>());
  const [clusters, setClusters] = useState<MapCluster[]>([]);
  const [loading, setLoading] = useState(false);
  const key = `${level}:${parentId ?? "-"}`;

  const load = useCallback(
    (bust: boolean) => {
      setLoading(true);
      mapClusters(level, parentId)
        .then((list) => {
          if (bust) cacheRef.current.delete(key);
          cacheRef.current.set(key, list);
          setClusters(list);
        })
        .catch(() => {
          if (bust) cacheRef.current.delete(key);
          setClusters(cacheRef.current.get(key) ?? []);
        })
        .finally(() => setLoading(false));
    },
    [level, parentId, key],
  );

  useEffect(() => {
    const cached = cacheRef.current.get(key);
    if (cached) {
      setClusters(cached);
      return;
    }
    load(false);
  }, [key, load]);

  /** 换一批：越过缓存重拉（随机样本变化） */
  const refresh = useCallback(() => load(true), [load]);

  /** 索引有新真值（回填推进/编辑联动）：全量失效重拉当前层 */
  const invalidateAll = useCallback(() => {
    cacheRef.current.clear();
    load(true);
  }, [load]);

  return { clusters, loading, refresh, invalidateAll };
}
