import { useSearchParams } from "react-router";

import type { AssetDto } from "@/ipc/api";
import type { AssetGroup } from "./assetGroups";
import { markAssetViewed } from "./viewMark";

/**
 * 查看器路由接线（画廊/搜索共用）：
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

export function useAssetViewer(groups: AssetGroup[]) {
  const [searchParams, setSearchParams] = useSearchParams();

  const assetParam = searchParams.get("asset");
  const assetId =
    assetParam !== null && /^\d+$/.test(assetParam) ? Number(assetParam) : null;

  let viewer: AssetViewerTarget | null = null;
  if (assetId !== null) {
    for (const group of groups) {
      const index = group.assets.findIndex((a) => a.id === assetId);
      if (index >= 0) {
        viewer = { asset: group.assets[index], group, index };
        break;
      }
    }
  }

  return {
    viewer,
    openAsset: (asset: AssetDto) => {
      markAssetViewed(asset.id);
      setSearchParams({ asset: String(asset.id) });
    },
    closeViewer: () => {
      setSearchParams({});
    },
    /** 同组内切换（查看器左右键/胶片条点击）；切图同样算一次浏览 */
    navigateTo: (index: number) => {
      if (!viewer) return;
      const next = viewer.group.assets[index];
      if (next) {
        markAssetViewed(next.id);
        setSearchParams({ asset: String(next.id) });
      }
    },
  };
}
