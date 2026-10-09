import { create } from "zustand";
import type { AssetDto } from "@/ipc/api";

export const ASSET_DRAG_TYPE = "application/x-photographer-asset";
export interface EditorOrigin { albumId?: number; subgroup?: string | null; }
export interface EditorPhoto extends EditorOrigin { asset: AssetDto; libraryId: string; }
interface EditorState {
  current: EditorPhoto | null;
  pending: EditorPhoto | null;
  request: (photo: EditorPhoto) => void;
  accept: () => void;
  cancel: () => void;
  close: () => void;
}
export const useAdvancedEditorStore = create<EditorState>((set, get) => ({
  current: null, pending: null,
  request: (photo) => {
    const current = get().current;
    if (current?.asset.id === photo.asset.id && current.libraryId === photo.libraryId) return;
    set(current ? { pending: photo } : { current: photo, pending: null });
  },
  accept: () => { const pending = get().pending; if (pending) set({ current: pending, pending: null }); },
  cancel: () => set({ pending: null }),
  close: () => set({ current: null, pending: null }),
}));

export function photoContext(asset: AssetDto, origin?: EditorOrigin): EditorPhoto | null {
  // 大一统定案（2026-10-09）：库是资产静态归属，不再有「活动库」；
  // 编辑上下文的库 id 直接取资产自身的 libraryId。
  const libraryId = asset.libraryId;
  if (!libraryId) return null;
  const match = window.location.pathname.match(/^\/albums\/(\d+)(?:\/|$)/);
  return { asset, libraryId, ...(match ? { albumId: Number(match[1]) } : {}), ...origin };
}

export function openAdvancedEditor(asset: AssetDto, origin?: EditorOrigin) {
  const photo = photoContext(asset, origin);
  if (photo) void import("./editorWindow").then(({ showEditorWindow }) => showEditorWindow(photo)).catch((error: unknown) => window.alert(String(error)));
}
