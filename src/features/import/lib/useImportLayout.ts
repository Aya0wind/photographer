import { useState } from "react";

// --- 查看方式（缩略图默认 / 列表），localStorage 持久化 ---------------------------

export type WizardViewMode = "list" | "grid";

export const VIEW_MODE_STORAGE_KEY = "smartphoto.import.viewMode";

function loadViewMode(): WizardViewMode {
  try {
    return localStorage.getItem(VIEW_MODE_STORAGE_KEY) === "list" ? "list" : "grid";
  } catch {
    return "grid";
  }
}

function saveViewMode(mode: WizardViewMode): void {
  try {
    localStorage.setItem(VIEW_MODE_STORAGE_KEY, mode);
  } catch {
    // 存储不可用时仅内存态生效
  }
}

// --- 左栏分区折叠 + 三栏列宽 + tile 尺寸（localStorage 记忆） ---------------------

export const PANEL_COLLAPSE_KEY = "smartphoto.import.panelCollapse";
export const COL_WIDTHS_KEY = "smartphoto.import.colWidths";
export const TILE_SIZE_KEY = "smartphoto.import.tileSize";

const DEFAULT_LEFT_WIDTH = 270;
const DEFAULT_RIGHT_WIDTH = 320;
const LEFT_WIDTH_RANGE: [number, number] = [200, 400];
const RIGHT_WIDTH_RANGE: [number, number] = [260, 440];

function clampNumber(value: number, [min, max]: [number, number]): number {
  return Math.min(max, Math.max(min, value));
}

export type PanelSectionKey = "devices" | "fs" | "recent" | "source";
type PanelCollapseState = Record<PanelSectionKey, boolean>;

function loadPanelCollapse(): PanelCollapseState {
  try {
    const raw = localStorage.getItem(PANEL_COLLAPSE_KEY);
    const parsed = raw ? (JSON.parse(raw) as Record<string, unknown>) : {};
    return {
      devices: parsed.devices === true,
      fs: parsed.fs === true,
      recent: parsed.recent === true,
      source: parsed.source === true,
    };
  } catch {
    return { devices: false, fs: false, recent: false, source: false };
  }
}

function savePanelCollapse(state: PanelCollapseState): void {
  try {
    localStorage.setItem(PANEL_COLLAPSE_KEY, JSON.stringify(state));
  } catch {
    // 存储不可用时仅内存态生效
  }
}

function loadColWidths(): { left: number; right: number } {
  try {
    const raw = localStorage.getItem(COL_WIDTHS_KEY);
    const parsed = raw ? (JSON.parse(raw) as { left?: unknown; right?: unknown }) : {};
    const left =
      typeof parsed.left === "number" && Number.isFinite(parsed.left)
        ? parsed.left
        : DEFAULT_LEFT_WIDTH;
    const right =
      typeof parsed.right === "number" && Number.isFinite(parsed.right)
        ? parsed.right
        : DEFAULT_RIGHT_WIDTH;
    return { left: clampNumber(left, LEFT_WIDTH_RANGE), right: clampNumber(right, RIGHT_WIDTH_RANGE) };
  } catch {
    return { left: DEFAULT_LEFT_WIDTH, right: DEFAULT_RIGHT_WIDTH };
  }
}

function saveColWidths(left: number, right: number): void {
  try {
    localStorage.setItem(COL_WIDTHS_KEY, JSON.stringify({ left, right }));
  } catch {
    // 存储不可用时仅内存态生效
  }
}

// --- 缩略图档位：标准 120（默认）/ 大 150，与其他照片网格统一为两档 -------

export type TileSizeKey = "standard" | "large";

export interface TileSizeSpec {
  /** 块宽（px），缩略区按 4:3 */
  width: number;
  thumbH: number;
  infoH: number;
  showSize: boolean;
}

export const TILE_SIZE_SPECS: Record<TileSizeKey, TileSizeSpec> = {
  standard: { width: 120, thumbH: 90, infoH: 40, showSize: true },
  large: { width: 150, thumbH: 112, infoH: 44, showSize: true },
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
  const [viewMode, updateViewMode] = useState<WizardViewMode>(loadViewMode);
  const [panelCollapse, setPanelCollapse] = useState<PanelCollapseState>(loadPanelCollapse);
  const [colWidths, setColWidths] = useState(loadColWidths);
  const [tileSize, updateTileSize] = useState<TileSizeKey>(loadTileSize);

  function setViewMode(mode: WizardViewMode): void {
    updateViewMode(mode);
    saveViewMode(mode);
  }
  function setTileSize(size: TileSizeKey): void {
    updateTileSize(size);
    saveTileSize(size);
  }
  /** 左栏分区折叠切换（写 localStorage 记忆） */
  function togglePanelSection(key: PanelSectionKey): void {
    setPanelCollapse((prev) => {
      const next = { ...prev, [key]: !prev[key] };
      savePanelCollapse(next);
      return next;
    });
  }

  /** 拖动分隔条：left=左栏宽 +dx；right=右栏宽 -dx（向右拖右栏变窄、中列变宽） */
  function resizeColumn(side: "left" | "right", dx: number): void {
    setColWidths((prev) => {
      const next =
        side === "left"
          ? { ...prev, left: clampNumber(Math.round(prev.left + dx), LEFT_WIDTH_RANGE) }
          : { ...prev, right: clampNumber(Math.round(prev.right - dx), RIGHT_WIDTH_RANGE) };
      if (next.left !== prev.left || next.right !== prev.right) {
        saveColWidths(next.left, next.right);
      }
      return next;
    });
  }

  /** 双击分隔条恢复默认列宽 */
  function resetColumns(): void {
    setColWidths({ left: DEFAULT_LEFT_WIDTH, right: DEFAULT_RIGHT_WIDTH });
    saveColWidths(DEFAULT_LEFT_WIDTH, DEFAULT_RIGHT_WIDTH);
  }

  return { viewMode, setViewMode, panelCollapse, colWidths, tileSize, setTileSize,
    togglePanelSection, resizeColumn, resetColumns };
}
