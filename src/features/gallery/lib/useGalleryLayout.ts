import { useState } from "react";

import type { GridLayout } from "@/features/gallery/components/AssetGrid";

/**
 * 画廊布局模式（四档循环）：square 方格 / tiles 瓦片 / justify 对齐行 / masonry 瀑布流。
 * union 归 AssetGrid 引擎侧拥有（LayoutSwitch 只管循环序与持久化，不渲染布局本身）。
 * localStorage `smartphoto.gallery.layout` 全局记忆；非法/缺失回落 justify——
 * justify 是引入本开关前画廊的固定布局，老用户升级后视觉不变。
 */

export type GalleryLayout = GridLayout;

export const GALLERY_LAYOUT_KEY = "smartphoto.gallery.layout";

/** 四档循环序（LayoutSwitch 点击按此推进；数组序即切换序） */
export const GALLERY_LAYOUT_ORDER: readonly GalleryLayout[] = [
  "square",
  "tiles",
  "justify",
  "masonry",
];

/** 非法/缺失存值时的默认档（保持既有画廊视觉） */
export const GALLERY_LAYOUT_DEFAULT: GalleryLayout = "justify";

export function loadGalleryLayout(): GalleryLayout {
  try {
    // 存值可能来自旧版本或手改，不在四档内则回落默认
    const stored = localStorage.getItem(GALLERY_LAYOUT_KEY);
    return stored !== null && (GALLERY_LAYOUT_ORDER as readonly string[]).includes(stored)
      ? (stored as GalleryLayout)
      : GALLERY_LAYOUT_DEFAULT;
  } catch {
    return GALLERY_LAYOUT_DEFAULT;
  }
}

export function saveGalleryLayout(layout: GalleryLayout): void {
  try {
    localStorage.setItem(GALLERY_LAYOUT_KEY, layout);
  } catch {
    // 存储不可用时仅内存态生效
  }
}

export function useGalleryLayout(): [GalleryLayout, (next: GalleryLayout) => void] {
  const [layout, setLayout] = useState<GalleryLayout>(loadGalleryLayout);
  const change = (next: GalleryLayout) => {
    setLayout(next);
    saveGalleryLayout(next);
  };
  return [layout, change];
}
