import { useState } from "react";

/**
 * 画廊/搜索共享的缩略图方格尺寸（三档）：小 120 / 中 200（默认）/ 大 280。
 * localStorage `smartphoto.gallery.tileSize` 全局记忆——画廊与搜索切换后保持一致；
 * AssetGrid 以边长重算虚拟行模型（列数/行高）。
 */

export type GalleryTileSize = "small" | "medium" | "large";

export const GALLERY_TILE_SIZE_KEY = "smartphoto.gallery.tileSize";

/** 方格边长（px） */
export const GALLERY_TILE_PX: Record<GalleryTileSize, number> = {
  small: 120,
  medium: 200,
  large: 280,
};

/** justify 网格目标行高（px，M4.5 A4）：画廊专用三档（~160/220/280）；
 *  搜索等 square 视图继续用 GALLERY_TILE_PX */
export const GALLERY_JUSTIFY_ROW_PX: Record<GalleryTileSize, number> = {
  small: 160,
  medium: 220,
  large: 280,
};

export function loadGalleryTileSize(): GalleryTileSize {
  try {
    const value = localStorage.getItem(GALLERY_TILE_SIZE_KEY);
    return value === "small" || value === "large" ? value : "medium";
  } catch {
    return "medium";
  }
}

export function saveGalleryTileSize(size: GalleryTileSize): void {
  try {
    localStorage.setItem(GALLERY_TILE_SIZE_KEY, size);
  } catch {
    // 存储不可用时仅内存态生效
  }
}

export function useGalleryTileSize(): [GalleryTileSize, (next: GalleryTileSize) => void] {
  const [size, setSize] = useState<GalleryTileSize>(loadGalleryTileSize);
  const change = (next: GalleryTileSize) => {
    setSize(next);
    saveGalleryTileSize(next);
  };
  return [size, change];
}
