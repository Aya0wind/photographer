import { useState } from "react";

/**
 * 画廊/搜索共享的缩略图方格尺寸（两档）：小 120 / 大 200（默认）。
 * 历史三档中的 280 已删——网格缩略图请求恒为 240→后端 256 一档，
 * 三档只是布局尺寸、最大档无更大图可加载纯属摆设。
 * localStorage `photographer.gallery.tileSize` 全局记忆——画廊与搜索切换后保持一致；
 * AssetGrid 以边长重算虚拟行模型（列数/行高）。旧存值 large 归并到大档。
 */

export type GalleryTileSize = "small" | "medium";

export const GALLERY_TILE_SIZE_KEY = "photographer.gallery.tileSize";

/** 方格边长（px） */
export const GALLERY_TILE_PX: Record<GalleryTileSize, number> = {
  small: 120,
  medium: 200,
};

/** justify 网格目标行高（px，M4.5 A4）：画廊专用两档（~160/220）；
 *  搜索等 square 视图继续用 GALLERY_TILE_PX */
export const GALLERY_JUSTIFY_ROW_PX: Record<GalleryTileSize, number> = {
  small: 160,
  medium: 220,
};

export function loadGalleryTileSize(): GalleryTileSize {
  try {
    // 旧三档的 large 归并到当前最大档（medium）
    return localStorage.getItem(GALLERY_TILE_SIZE_KEY) === "small" ? "small" : "medium";
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
