import { useMemo, useRef, useState } from "react";
import { useSearchParams } from "react-router";

import { assetsByIds, type AssetDto } from "@/ipc/api";
import type { AssetGroup } from "./assetGroups";
import { markAssetViewed } from "./viewMark";

/**
 * 查看器路由接线（画廊/搜索共用，跨日期浏览当前结果集）：
 * 以 /gallery?asset=<id>（或 /search?asset=<id>）的 URL searchParams 表达打开状态——
 * 组件不卸载（虚拟网格滚动位置天然保留）、可深链/可后退，Esc=移除参数返回。
 *
 * 为什么不用 /gallery/:assetId 子路由：画廊是 keyset 分页（按 afterId 顺序加载），
 * 无法直接跳到任意资产的位置；路由内状态让画廊页保持挂载，返回时滚动位置与
 * 已加载页完整保留。
 *
 * 浏览打点（M4.5）：openAsset/navigateTo 打开资产即 markAssetViewed（30s 去抖，
 * 后端 recent_viewed 数据源）；打点失败静默，不阻塞查看。
 */

export interface AssetViewerTarget {
  asset: AssetDto;
  group: AssetGroup;
  index: number;
}

export function useAssetViewer(groups: AssetGroup[], sourceAssets: AssetDto[] = []) {
  const [searchParams, setSearchParams] = useSearchParams();
  const [version, setVersion] = useState<{ anchorId: number; asset: AssetDto } | null>(null);
  const versionRequest = useRef(0);
  // 日期分组只服务网格展示。预览使用当前结果集的完整顺序，翻页可跨日期。
  const viewerGroup = useMemo<AssetGroup>(() => ({
    key: "__viewer__",
    date: null,
    assets: groups.flatMap((group) => group.assets),
  }), [groups]);

  const assetParam = searchParams.get("asset");
  const assetId =
    assetParam !== null && /^\d+$/.test(assetParam) ? Number(assetParam) : null;
  const activeAnchor = useRef(assetId);
  activeAnchor.current = assetId;

  let viewer: AssetViewerTarget | null = null;
  if (assetId !== null) {
    const original = viewerGroup.assets.find((a) => a.id === assetId)
      ?? sourceAssets.find((a) => a.id === assetId);
    let index = viewerGroup.assets.findIndex((a) => a.id === assetId);
    // 后一页加载 JPG 后，原先的 RAW 卡会被合并；已打开的预览仍属于该组。
    if (index < 0 && typeof original?.pairId === "number") {
      index = viewerGroup.assets.findIndex((a) => a.pairId === original.pairId);
    }
    if (index >= 0) viewer = {
      asset: version?.anchorId === assetId ? version.asset : original ?? viewerGroup.assets[index],
      group: viewerGroup,
      index,
    };
  }

  return {
    viewer,
    openAsset: (asset: AssetDto) => {
      ++versionRequest.current;
      setVersion(null);
      markAssetViewed(asset.id);
      setSearchParams(previous => {
        const next=new URLSearchParams(previous);
        next.set("asset",String(asset.id));
        return next;
      });
    },
    closeViewer: () => {
      ++versionRequest.current;
      setVersion(null);
      setSearchParams(previous => {
        const next=new URLSearchParams(previous);
        next.delete("asset");
        return next;
      });
    },
    /** 在当前结果集切换（可跨日期）；切图同样算一次浏览 */
    navigateTo: (index: number) => {
      if (!viewer) return;
      const next = viewer.group.assets[index];
      if (next) {
        ++versionRequest.current;
        setVersion(null);
        markAssetViewed(next.id);
        setSearchParams(previous => {
          const params=new URLSearchParams(previous);
          params.set("asset",String(next.id));
          return params;
        });
      }
    },
    /** 格式/版本切换只替换预览文件，不插入网格或改变胶片条的位置。 */
    selectVersion: async (id: number) => {
      if (!viewer || assetId === null) return;
      const anchorId = assetId;
      const request = ++versionRequest.current;
      try {
        const known = sourceAssets.find((a) => a.id === id)
          ?? viewerGroup.assets.find((a) => a.id === id);
        const selected = known ?? (await assetsByIds([id])).find((a) => a.id === id);
        if (!selected || activeAnchor.current !== anchorId || request !== versionRequest.current) return;
        markAssetViewed(selected.id);
        setVersion({ anchorId, asset: selected });
      } catch {
        // 已删除/不可访问的版本保持当前预览。
      }
    },
  };
}
