import { assetViewMark } from "@/ipc/api";

/**
 * 浏览打点（M4.5 最近浏览）：查看器打开/切图时调用 asset_view_mark。
 * 前端去抖：同一资产 30s 内重复打开/切换不重复打点（fire-and-forget，失败静默）。
 */

export const VIEW_MARK_DEBOUNCE_MS = 30_000;

const lastMarkAt = new Map<number, number>();

/** 标记资产被浏览（30s 去抖；时间到后同资产再次打开会重新打点） */
export function markAssetViewed(assetId: number, now = Date.now()): void {
  const last = lastMarkAt.get(assetId);
  if (last !== undefined && now - last < VIEW_MARK_DEBOUNCE_MS) return;
  lastMarkAt.set(assetId, now);
  void assetViewMark(assetId);
}

/** 仅测试用：清空去抖表（模块级状态，用例间隔离） */
export function resetViewMarkForTests(): void {
  lastMarkAt.clear();
}
