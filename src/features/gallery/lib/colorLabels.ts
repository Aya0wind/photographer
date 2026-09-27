/**
 * 颜色标签（B1，LR 五色标）前端共享定义：
 * - 枚举与 IPC 契约（AssetDto.colorLabel / AssetFilters.colorLabel / asset_label_set）同域
 * - 色点样式表（Tailwind 4 调色板近似 LR 经典红黄绿蓝紫）
 * - 中文文案走 i18n：gallery.color.<label>
 */

import type { ColorLabelKind } from "@/ipc/api";

export type ColorLabel = ColorLabelKind;

export const COLOR_LABELS: readonly ColorLabel[] = ["red", "yellow", "green", "blue", "purple"];

/** 字符串 → ColorLabel（非法值/空 → null；后端脏数据防御） */
export function asColorLabel(value: string | null | undefined): ColorLabel | null {
  return COLOR_LABELS.includes(value as ColorLabel) ? (value as ColorLabel) : null;
}

/** 色点底色（瓦片角标/查看器色点/筛选按钮共用；LR 经典色近似） */
export const COLOR_DOT_CLASS: Record<ColorLabel, string> = {
  red: "bg-red-500",
  yellow: "bg-yellow-400",
  green: "bg-green-500",
  blue: "bg-blue-500",
  purple: "bg-purple-500",
};

/** 色点描边（未选中态在深浅背景上都可辨） */
export const COLOR_DOT_RING = "ring-1 ring-black/30";
