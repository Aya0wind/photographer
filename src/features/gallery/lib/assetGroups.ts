import type { AssetDto } from "@/ipc/api";

/**
 * 画廊/搜索共用的日期分组（M3）：
 * capturedAt（ISO 8601）→ 本地日期组 "YYYY-MM-DD"；EXIF 缺失（capturedAt=null）
 * 归「未知日期」组且组序最前（比最早日期还前）。组序不信任 assets_page 的到达顺序，
 * 分组后统一排序：未知组 → 日期降序（ISO 日期字符串字典序即时间序）。
 */

/** 未知日期组键（EXIF 缺失资产的归组；组头/chips 中显示为「未知日期」） */
export const UNKNOWN_GROUP_KEY = "__unknown__";

export interface AssetGroup {
  /** 组键："YYYY-MM-DD" 或 UNKNOWN_GROUP_KEY */
  key: string;
  /** ISO 日期（截取前 10 位）；未知组为 null */
  date: string | null;
  assets: AssetDto[];
}

/** capturedAt → 组键（null/空串/短于日期 → 未知组） */
export function groupKeyOfDate(date: string | null | undefined): string {
  if (date === null || date === undefined || date.length < 10) return UNKNOWN_GROUP_KEY;
  return date.slice(0, 10);
}

/** 按拍摄日期分组（保持资产在组内的到达顺序；组间排序见文件头注释） */
export function groupAssetsByDate(assets: AssetDto[]): AssetGroup[] {
  const map = new Map<string, AssetGroup>();
  const keys: string[] = [];
  for (const asset of assets) {
    const key = groupKeyOfDate(asset.capturedAt);
    let group = map.get(key);
    if (!group) {
      group = { key, date: key === UNKNOWN_GROUP_KEY ? null : key, assets: [] };
      map.set(key, group);
      keys.push(key);
    }
    group.assets.push(asset);
  }
  keys.sort((a, b) => {
    if (a === UNKNOWN_GROUP_KEY) return b === UNKNOWN_GROUP_KEY ? 0 : -1;
    if (b === UNKNOWN_GROUP_KEY) return 1;
    return a < b ? 1 : a > b ? -1 : 0;
  });
  return keys.map((key) => map.get(key) as AssetGroup);
}

/** 组头日期文案："2026-09-18" → "2026年9月18日"（未知组的「未知日期」由调用方以 i18n 呈现） */
export function formatDateLabel(date: string): string {
  const [y, m, d] = date.slice(0, 10).split("-");
  if (!y || !m || !d) return date;
  return `${Number(y)}年${Number(m)}月${Number(d)}日`;
}

/** chips 条短文案："2026-09-18" → "09-18"（年份在 title 提示中完整给出） */
export function formatDateChip(date: string): string {
  return date.slice(5, 10);
}
