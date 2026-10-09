import { isTauri } from "@tauri-apps/api/core";
import { emitTo, listen } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { EditorPhoto } from "./advancedEditorStore";

export const EDITOR_WINDOW = "advanced-editor";
export const EDITOR_OPEN = "advanced-editor-open";
export const EDITOR_READY = "advanced-editor-ready";
let requests: Promise<void> = Promise.resolve();

async function createWindow(photo: EditorPhoto): Promise<void> {
  let release: (() => void) | undefined;
  let errorRelease: (() => void) | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let resolveReady!: () => void;
  let rejectReady!: (error: unknown) => void;
  const ready = new Promise<void>((resolve, reject) => { resolveReady = resolve; rejectReady = reject; });
  release = await listen(EDITOR_READY, resolveReady);
  const params = new URLSearchParams({ library: photo.libraryId, asset: String(photo.asset.id) });
  if (photo.albumId !== undefined) params.set("album", String(photo.albumId));
  if (photo.subgroup != null) params.set("subgroup", photo.subgroup);
  const editor = new WebviewWindow(EDITOR_WINDOW, { url: `/advanced-editor?${params}`,
    title: "Photo Hub — Advanced Editor", width: 1440, height: 960, minWidth: 1000, minHeight: 680,
    decorations: true, resizable: true, focus: true, dragDropEnabled: false,
    theme: document.documentElement.dataset.theme === "light" ? "light" : "dark" });
  try {
    errorRelease = await editor.once("tauri://error", (event) => rejectReady(event.payload));
    timer = setTimeout(() => rejectReady(new Error("Advanced editor window did not become ready")), 20000);
    await ready;
  } catch (error) {
    await editor.destroy().catch(() => {});
    throw error;
  } finally { clearTimeout(timer); release?.(); errorRelease?.(); }
}

async function openWindow(photo: EditorPhoto): Promise<void> {
  if (!isTauri()) { window.location.assign(`/advanced-editor?${new URLSearchParams({ library: photo.libraryId, asset: String(photo.asset.id) })}`); return; }
  let editor = await WebviewWindow.getByLabel(EDITOR_WINDOW);
  if (!editor) {
    await createWindow(photo);
    editor = await WebviewWindow.getByLabel(EDITOR_WINDOW);
  }
  if (!editor) throw new Error("Advanced editor window is unavailable");
  await emitTo(EDITOR_WINDOW, EDITOR_OPEN, photo);
  await editor.show();
  await editor.setFocus();
}

/** 序列化创建/切图请求，避免快速双击创建两个同名窗口。 */
export function showEditorWindow(photo: EditorPhoto): Promise<void> {
  const next = requests.catch(() => {}).then(() => openWindow(photo));
  requests = next;
  return next;
}
