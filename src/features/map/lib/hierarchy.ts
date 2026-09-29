/**
 * 缩放层级 ↔ 行政层级映射（docs/plans/map-module.md）：
 * 全球视角看国家、国家视角看省、省看市、市看县（2026-09-29 定案：
 * 最深市县，无更深开关）。阈值经防抖（MapCanvas 250ms）后切换。
 */

export type MapLevel = 0 | 1 | 2 | 3;

export function levelForZoom(zoom: number): MapLevel {
  if (zoom < 3.5) return 0;
  if (zoom < 6.5) return 1;
  if (zoom < 9.5) return 2;
  return 3;
}

/** 点击下钻/回退时 flyTo 的目标 zoom（取该层可视区间的中部） */
export function zoomForLevel(level: MapLevel): number {
  if (level === 0) return 1.5;
  if (level === 1) return 4.8;
  if (level === 2) return 7.8;
  return 10.8;
}

export function levelLabelKey(level: MapLevel): string {
  if (level === 0) return "map.level.country";
  if (level === 1) return "map.level.province";
  if (level === 2) return "map.level.city";
  return "map.level.county";
}
