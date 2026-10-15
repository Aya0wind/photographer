import { useState } from "react";

// --- 查看方式：每次打开默认列表，缩略图由用户主动选择 ---------------------------

export type WizardViewMode = "list" | "grid";

export const VIEW_MODE_STORAGE_KEY = "photographer.import.viewMode";

// --- 展示尺寸；旧三栏存储键仅保留导出，不再读取旧布局 ---------------------

export const PANEL_COLLAPSE_KEY = "photographer.import.panelCollapse";
export const COL_WIDTHS_KEY = "photographer.import.colWidths";
export const TILE_SIZE_KEY = "photographer.import.tileSize";

// --- 缩略图档位：标准 160（默认）/ 大 220，适配全宽挑选页面 -------

export type TileSizeKey = "standard" | "large";

export interface TileSizeSpec {
  /** 块宽（px），缩略区按 4:3 */
  width: number;
  thumbH: number;
  infoH: number;
  showSize: boolean;
}

export const TILE_SIZE_SPECS: Record<TileSizeKey, TileSizeSpec> = {
  standard: { width: 160, thumbH: 120, infoH: 40, showSize: true },
  large: { width: 220, thumbH: 165, infoH: 44, showSize: true },
};
export const TILE_SIZE_ORDER: readonly TileSizeKey[] = ["standard", "large"];
/** 档位图标：居中方块边长（12 viewBox 内） */
export const TILE_SIZE_ICON: Record<TileSizeKey, number> = { standard: 7, large: 12 };

function loadTileSize(): TileSizeKey {
  try {
    const value = localStorage.getItem(TILE_SIZE_KEY);
    return value === "large" ? value : "standard";
  } catch {
    return "standard";
  }
}

function saveTileSize(size: TileSizeKey): void {
  try {
    localStorage.setItem(TILE_SIZE_KEY, size);
  } catch {
    // 存储不可用时仅内存态生效
  }
}

export function useImportLayout() {
  const [viewMode, updateViewMode] = useState<WizardViewMode>("list");
  const [tileSize, updateTileSize] = useState<TileSizeKey>(loadTileSize);

  function setViewMode(mode: WizardViewMode): void {
    updateViewMode(mode);
  }
  function setTileSize(size: TileSizeKey): void {
    updateTileSize(size);
    saveTileSize(size);
  }
  return { viewMode, setViewMode, tileSize, setTileSize };
}
