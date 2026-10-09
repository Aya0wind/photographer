import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { isTauri } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow, Window } from "@tauri-apps/api/window";
import { assetsByIds } from "@/ipc/api";
import { useSettingsStore } from "@/stores/settingsStore";
import AdvancedEditorOverlay from "./AdvancedEditorOverlay";
import { useAdvancedEditorStore, type EditorPhoto } from "../lib/advancedEditorStore";
import { EDITOR_OPEN, EDITOR_READY } from "../lib/editorWindow";

export default function AdvancedEditorWindowPage() {
  const { t } = useTranslation();
  const current = useAdvancedEditorStore((s) => s.current);
  const loaded = useSettingsStore((s) => s.loaded);
  const theme = useSettingsStore((s) => s.settings.appearance?.theme ?? "dark");
  useEffect(() => {
    if (isTauri()) void getCurrentWindow().setTheme(theme).catch((error: unknown) => console.warn("Could not update editor window theme", error));
  }, [theme]);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!loaded) return;
    let cancelled = false;
    let release: (() => void) | undefined;
    void (async () => {
      if (isTauri()) release = await listen<EditorPhoto>(EDITOR_OPEN, ({ payload }) => {
        if (!cancelled && payload && typeof payload.libraryId === "string" && Number.isSafeInteger(payload.asset?.id) && payload.asset.id > 0)
          useAdvancedEditorStore.getState().request(payload);
      });
      if (cancelled) { release?.(); return; }
      const params = new URLSearchParams(window.location.search);
      const libraryId = params.get("library");
      const id = Number(params.get("asset"));
      if (!libraryId || !Number.isSafeInteger(id) || id <= 0) throw new Error(t("advancedEditor.invalidPhoto"));
      const [asset] = await assetsByIds([id]);
      if (cancelled) return;
      if (!asset) throw new Error(t("advancedEditor.invalidPhoto"));
      // 库为资产静态归属：URL 里的库参数与资产实际所属库不一致 = 旧会话跨库
      if (asset.libraryId !== libraryId) throw new Error(t("advancedEditor.libraryChanged"));
      const album = params.get("album");
      const albumId = album ? Number(album) : undefined;
      useAdvancedEditorStore.getState().request({ asset, libraryId,
        ...(albumId && Number.isSafeInteger(albumId) ? { albumId } : {}), subgroup: params.get("subgroup") });
    })().catch((e: unknown) => { if (!cancelled) setError(String(e)); }).finally(() => {
      if (!cancelled && isTauri()) void emit(EDITOR_READY).catch(() => {});
    });
    return () => { cancelled = true; release?.(); };
  }, [loaded, t]);
  const close = useCallback(() => {
    if (!isTauri()) { window.history.back(); return; }
    void getCurrentWindow().destroy().then(() => Window.getByLabel("main")).then(async (main) => {
      if (main) { await main.show(); await main.setFocus(); }
    }).catch((error: unknown) => console.error("Could not close editor or restore library window", error));
  }, []);
  if (!current) return <div className="flex h-screen items-center justify-center bg-bg text-text-secondary">
    {error ? <div className="space-y-3"><p role="alert">{error}</p><button onClick={close}>{t("editor.close")}</button></div> : t("advancedEditor.loading")}
  </div>;
  return <AdvancedEditorOverlay key={`${current.libraryId}:${current.asset.id}`} asset={current.asset}
    libraryId={current.libraryId} originAlbumId={current.albumId} originSubgroup={current.subgroup} onClose={close} />;
}
