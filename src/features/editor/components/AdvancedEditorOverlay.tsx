import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { CurvePicker } from "../lib/curves";
import { useEffect, useMemo, useReducer, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { open } from "@tauri-apps/plugin-dialog";
import {
  editPreviewPick, albumList, assetAlbums, editProjectOpen, editProjectSave, advancedExportFolder, advancedExport, editExportStatus, subscribeAppEvents,
  type AlbumDto, type AssetDto, type EditRecipe, type ExportOptions,
} from "@/ipc/api";
import ActionPopover from "@/shared/components/ActionPopover";
import EditorCanvas, { type EditorTool } from "./EditorCanvas";
import ExportAlbumPicker from "./ExportAlbumPicker";
import AdvancedToolPanel from "./AdvancedToolPanel";
import { advancedRecipe, advancedReducer } from "../lib/advancedRecipe";
import { newLayerId, recipeEquals, recipeForPersist, type RecipeContext } from "../lib/recipe";
import { clampCrop, isFullCrop, type Size } from "../lib/coords";
import { useEditorPreview } from "../lib/useEditorPreview";
import { ASSET_DRAG_TYPE, useAdvancedEditorStore, type EditorPhoto } from "../lib/advancedEditorStore";

const TOOLS: { id: EditorTool; icon: string; label: string }[] = [
  { id: "view", icon: "M4 8h16v12H4zM8 4h8", label: "editor.tool.view" },
  { id: "crop", icon: "M6 3v15h15M3 6h15v15", label: "editor.tool.cropRotate" },
  { id: "adjust", icon: "M4 7h16M4 12h16M4 17h16M8 4v6M16 9v6M10 14v6", label: "editor.tool.adjust" },
  { id: "text", icon: "M5 5h14M12 5v14M8 19h8", label: "editor.tool.text" },
  { id: "brush", icon: "m5 15 10-11 5 5-11 10H4zM13 6l5 5", label: "editor.tool.brush" },
  { id: "output", icon: "M12 3v12m-4-4 4 4 4-4M4 16v5h16v-5", label: "editor.tool.output" },
];
const BUTTON = "rounded-xl border border-edge px-3 py-2 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent disabled:opacity-40";

export default function AdvancedEditorOverlay({ asset, libraryId, originAlbumId, originSubgroup, onClose }: { asset: AssetDto; libraryId: string; originAlbumId?: number; originSubgroup?: string | null; onClose: () => void }) {
  const { t } = useTranslation();
  // 大一统:无「活动库切换」,编辑期间资产归属不可变;仅防御资产数据异常
  const libraryChanged = asset.libraryId != null && asset.libraryId !== libraryId;
  const context = useRef<RecipeContext>({ width: 0, height: 0 });
  const [history, dispatch] = useReducer(
    (state: Parameters<typeof advancedReducer>[0], action: Parameters<typeof advancedReducer>[1]) => advancedReducer(state, action, context.current),
    undefined, () => ({ past: [], present: advancedRecipe(), future: [] }),
  );
  const recipe = history.present;
  const [tool, setTool] = useState<EditorTool>("adjust");
  const preview = useEditorPreview(asset.id, tool === "crop" ? { ...recipe, crop: null, textLayers: [], brushStrokes: [] } : recipe, libraryId);
  const sourceSize = useMemo<Size | null>(() => preview.session ? { width: preview.session.width, height: preview.session.height } : null, [preview.session]);
  context.current = sourceSize ?? { width: 0, height: 0 };
  const [zoom, setZoom] = useState(1);
  const [zoomVisible, setZoomVisible] = useState(true);
  const zoomControls = useRef<HTMLDivElement>(null);
  const zoomHover = useRef(false);
  const zoomTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  function wakeZoom() {
    setZoomVisible(true);
    if (zoomTimer.current) clearTimeout(zoomTimer.current);
    zoomTimer.current = setTimeout(() => {
      if (!zoomHover.current && !zoomControls.current?.contains(document.activeElement)) setZoomVisible(false);
    }, 3000);
  }
  useEffect(() => {
    wakeZoom();
    return () => { if (zoomTimer.current) clearTimeout(zoomTimer.current); };
  }, []);
  const [compare, setCompare] = useState(false);
  const [picker, setPicker] = useState<CurvePicker | null>(null);
  const [sample, setSample] = useState<[number, number, number] | null>(null);
  const [samplingError, setSamplingError] = useState<string | null>(null);
  const pickSequence = useRef(0);
  useEffect(() => { pickSequence.current++; }, [recipe, picker]);
  async function samplePhoto(x: number, y: number) {
    const sessionId = preview.session?.sessionId;
    if (!sessionId || !picker) return;
    const sequence = ++pickSequence.current;
    try {
      const result = await editPreviewPick(sessionId, recipe, x, y, picker);
      if (sequence !== pickSequence.current) return;
      setSamplingError(null);
      if (result.sample) setSample(result.sample);
      else dispatch({ type: "advanced", patch: { curves: result.points ?? [], channelCurves: {
        ...recipe.advanced?.channelCurves, red: result.red ?? [], green: result.green ?? [], blue: result.blue ?? [],
      } } });
    } catch (error) { if (sequence === pickSequence.current) setSamplingError(String(error)); }
  }
  const [cropDraft, setCropDraft] = useState<EditRecipe["crop"]>(null);
  const [cropRatio, setCropRatio] = useState<number | null>(null);
  const [selectedText, setSelectedText] = useState<string | null>(null);
  const [color, setColor] = useState("#FFFFFF");
  const [brushWidth, setBrushWidth] = useState(0.005);
  const [projectLoading, setProjectLoading] = useState(true);
  const [projectError, setProjectError] = useState<string | null>(null);
  const [projectRetry, setProjectRetry] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const pendingPhoto = useAdvancedEditorStore((s) => s.pending);
  const returnAfterExport = useRef(false);
  const [baseline, setBaseline] = useState(() => advancedRecipe());
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<"close" | "reset" | "switch" | null>(null);
  const [albumPicker, setAlbumPicker] = useState(false);
  const [albums, setAlbums] = useState<AlbumDto[]>([]);
  const [sourceAlbums, setSourceAlbums] = useState<AlbumDto[]>([]);
  const albumCloseAfter = useRef(false);
  const [exporting, setExporting] = useState(false);
  const job = useRef<number | null>(null);
  const gesture = useRef<EditRecipe | null>(null);
  const mounted = useRef(true);
  const effective = useMemo(() => tool === "crop" && cropDraft ? advancedReducer(history, {
    type: "cropApply", crop: isFullCrop(clampCrop(cropDraft)) ? null : clampCrop(cropDraft),
  }, context.current).present : recipe, [tool, cropDraft, history, recipe, sourceSize]);
  const dirty = !recipeEquals(recipeForPersist(effective), recipeForPersist(baseline));
  const unavailable = libraryChanged || !preview.session || preview.error !== null || projectLoading || projectError !== null;

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  useEffect(() => {
    let cancelled = false;
    void albumList().then((value) => { if (!cancelled) setAlbums(value); }).catch(() => {});
    void assetAlbums(asset.id).then((value) => { if (!cancelled) setSourceAlbums(value); });
    return () => { cancelled = true; };
  }, [asset.id, libraryId]);
  useEffect(() => {
    let cancelled = false;
    let off: (() => void) | null = null;
    void subscribeAppEvents((event) => {
      if (event.type === "exportTaskFinished" && event.jobId === job.current) {
        job.current = null;
        setExporting(false);
        setNotice(event.ok ? t("advancedEditor.exported") : event.error ?? t("editor.exportFailed"));
        if (event.ok && returnAfterExport.current) closeEditor();
        returnAfterExport.current = false;
      }
    }).then((value) => { if (cancelled) value(); else off = value; }).catch(() => {});
    return () => { cancelled = true; off?.(); };
  }, [t, onClose]);

  useEffect(() => {
    if (!exporting || libraryChanged) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      const id = job.current;
      if (id === null || cancelled) return;
      try {
        const task = await editExportStatus(asset.id, libraryId, id);
        if (cancelled || job.current !== id) return;
        if (task.status === "done" || task.status === "error") {
          job.current = null; setExporting(false);
          setNotice(task.status === "done" ? t("advancedEditor.exported") : task.error ?? t("editor.exportFailed"));
          if (task.status === "done" && returnAfterExport.current) closeEditor();
          returnAfterExport.current = false;
          return;
        }
      } catch { /* 事件通道仍可完成任务；暂时不可读时重试。 */ }
      if (!cancelled) timer = setTimeout(() => void poll(), 1500);
    };
    void poll();
    return () => { cancelled = true; clearTimeout(timer); };
  }, [exporting, libraryChanged, asset.id, libraryId, onClose, t]);

  useEffect(() => {
    let cancelled = false;
    setProjectLoading(true); setProjectError(null);
    void editProjectOpen(asset.id, libraryId).then((saved) => {
      if (cancelled) return;
      const loaded = saved ?? advancedRecipe();
      dispatch({ type: "loadProject", recipe: loaded }); setBaseline(loaded);
    }).catch((e: unknown) => { if (!cancelled) setProjectError(String(e)); })
      .finally(() => { if (!cancelled) setProjectLoading(false); });
    return () => { cancelled = true; };
  }, [asset.id, libraryId, projectRetry]);
  useEffect(() => {
    if (!pendingPhoto || busy || exporting || projectLoading) return;
    if (dirty) setConfirm("switch");
    else useAdvancedEditorStore.getState().accept();
  }, [pendingPhoto, dirty, busy, exporting, projectLoading]);
  useEffect(() => { rootRef.current?.focus(); }, []);
  const requestCloseRef = useRef<() => void>(() => {});
  requestCloseRef.current = requestClose;
  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let release: (() => void) | undefined;
    void getCurrentWindow().onCloseRequested((event) => {
      event.preventDefault(); requestCloseRef.current();
    }).then((off) => { if (disposed) off(); else release = off; });
    return () => { disposed = true; release?.(); };
  }, []);
  useEffect(() => {
    const release = () => setCompare(false);
    window.addEventListener("blur", release);
    window.addEventListener("pointerup", release);
    window.addEventListener("pointercancel", release);
    window.addEventListener("keyup", release);
    return () => { window.removeEventListener("blur", release); window.removeEventListener("pointerup", release); window.removeEventListener("pointercancel", release); window.removeEventListener("keyup", release); };
  }, []);

  function beginGesture() { gesture.current ??= recipe; }
  function endGesture() {
    if (gesture.current && !recipeEquals(gesture.current, recipe)) dispatch({ type: "commitFrom", snapshot: gesture.current });
    gesture.current = null;
  }
  function chooseTool(next: EditorTool) {
    endGesture();
    setCompare(false); setPicker(null);
    if (tool === "crop" && next !== "crop") setCropDraft(null);
    if (next === "crop") { setCropDraft(recipe.crop ?? { x: 0, y: 0, w: 1, h: 1 }); setCropRatio(null); }
    setTool(next);
  }
  function closeEditor() {
    busyRef.current = true; setBusy(true);
    void preview.close().finally(onClose);
  }
  function requestClose() { if (!busyRef.current && !exporting) dirty ? setConfirm("close") : closeEditor(); }

  async function saveProject(): Promise<boolean> {
    if (busyRef.current || unavailable) return false;
    busyRef.current = true; setBusy(true);
    const snapshot = recipeForPersist(effective);
    try {
      await editProjectSave(asset.id, libraryId, snapshot);
      if (!mounted.current) return false;
      if (tool === "crop" && cropDraft) { dispatch({ type: "cropApply", crop: snapshot.crop }); setCropDraft(null); setTool("view"); }
      setBaseline(snapshot); setNotice(t("advancedEditor.saved"));
      return true;
    } catch (e: unknown) { if (mounted.current) setNotice(String(e)); return false; }
    finally { busyRef.current = false; if (mounted.current) setBusy(false); }
  }
  async function returnToLibrary(albumId = originAlbumId ?? (sourceAlbums.length === 1 ? sourceAlbums[0].id : undefined), subgroup: string | null = null, closeAfter = false) {
    if (exporting || busyRef.current || unavailable) return;
    if (albumId === undefined && sourceAlbums.length > 1) {
      albumCloseAfter.current = closeAfter; setAlbumPicker(true); return;
    }
    const snapshot = recipeForPersist(effective);
    if (!await saveProject() || !mounted.current) return;
    busyRef.current = true; setBusy(true);
    try {
      const task = await advancedExport(asset.id, libraryId, snapshot, preview.session!.sessionId, albumId, subgroup);
      if (!mounted.current) return;
      job.current = task.id; returnAfterExport.current = closeAfter;
      setExporting(true); setNotice(t("advancedEditor.exporting"));
    } catch (e: unknown) { if (mounted.current) setNotice(String(e)); }
    finally { busyRef.current = false; if (mounted.current) setBusy(false); }
  }
  async function startExport(target: Pick<ExportOptions, "mode" | "folder" | "album">) {
    if (busyRef.current || exporting || unavailable) return;
    busyRef.current = true; setBusy(true);
    try {
      const result = await advancedExportFolder({ assetId: asset.id, libraryId, sessionId: preview.session!.sessionId }, recipeForPersist(effective), {
        ...target, longEdge: recipe.output.longEdge, quality: recipe.output.quality,
        removeGps: false, copyright: "", author: "", keywords: [],
      });
      if (!mounted.current) return;
      job.current = result.id; setExporting(true); setNotice(t("advancedEditor.exporting"));
    } catch (e: unknown) { if (mounted.current) setNotice(String(e)); }
    finally { busyRef.current = false; if (mounted.current) setBusy(false); }
  }
  async function exportFolder() {
    if (busyRef.current || exporting) return;
    try {
      const path = await open({ directory: true, multiple: false });
      if (typeof path === "string" && mounted.current) await startExport({ mode: "folder",
        folder: { outputDir: path, fileName: `${asset.name.replace(/\.[^.]+$/, "")}_advanced.jpg` } });
    } catch (e: unknown) { if (mounted.current) setNotice(String(e)); }
  }

  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return;
      const input = event.target instanceof HTMLElement && (event.target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(event.target.tagName));
      if (input) return;
      if (event.key === "\\" && !event.ctrlKey && !event.metaKey && !unavailable) { event.preventDefault(); setCompare(true); return; }
      if (event.key === "Escape") {
        event.preventDefault(); event.stopImmediatePropagation();
        if (picker) setPicker(null);
        else if (confirm) { setConfirm(null); useAdvancedEditorStore.getState().cancel(); }
        else if (albumPicker) setAlbumPicker(false);
        else requestClose();
      } else if (!busyRef.current && !exporting && !unavailable && (event.metaKey || event.ctrlKey)) {
        if (event.key.toLowerCase() === "s") { event.preventDefault(); void saveProject(); }
        if (event.key.toLowerCase() === "z") { event.preventDefault(); endGesture(); dispatch({ type: event.shiftKey ? "redo" : "undo" }); }
      }
    };
    window.addEventListener("keydown", key, true);
    return () => window.removeEventListener("keydown", key, true);
  });

  return <div ref={rootRef} tabIndex={-1} role="dialog" aria-modal="true" aria-label={t("advancedEditor.title")} className="fixed inset-0 flex flex-col bg-bg outline-none" data-testid="advanced-editor-overlay"
    onDragOver={(e) => { if (e.dataTransfer.types.includes(ASSET_DRAG_TYPE)) { e.preventDefault(); e.dataTransfer.dropEffect = "copy"; } }}
    onDrop={(e) => {
      e.preventDefault();
      if (busy || exporting || projectLoading) return;
      try {
        const raw = e.dataTransfer.getData(ASSET_DRAG_TYPE);
        if (!raw || raw.length > 32768) return;
        const photo = JSON.parse(raw) as EditorPhoto;
        if (photo.libraryId === libraryId && Number.isSafeInteger(photo.asset?.id) && photo.asset.id > 0) useAdvancedEditorStore.getState().request(photo);
      } catch { setNotice(t("advancedEditor.dropFailed")); }
    }}>
    <header className="ui-glass flex min-h-14 shrink-0 flex-wrap items-center gap-2 border-b border-edge px-4 py-2">
      <div className="min-w-0"><h2 className="text-sm font-semibold text-text-primary">{t("advancedEditor.title")} {dirty && <span className="text-accent">•</span>}</h2><p className="truncate text-[11px] text-text-muted">{asset.name}</p></div>
      <div className="ml-auto flex flex-wrap items-center justify-end gap-2">
        <button className={BUTTON} disabled={!history.past.length || busy || exporting} onClick={() => { endGesture(); dispatch({ type: "undo" }); }} title={t("editor.undo")} aria-label={t("editor.undo")}>↶</button>
        <button className={BUTTON} disabled={!history.future.length || busy || exporting} onClick={() => dispatch({ type: "redo" })} title={t("editor.redo")} aria-label={t("editor.redo")}>↷</button>
        <button className={BUTTON} disabled={busy || unavailable} aria-pressed={compare} title={t("advancedEditor.compareHold")} onPointerDown={(event) => { if (event.button !== 0) return; event.currentTarget.setPointerCapture(event.pointerId); setCompare(true); }} onPointerUp={() => setCompare(false)} onPointerCancel={() => setCompare(false)} onLostPointerCapture={() => setCompare(false)} onBlur={() => setCompare(false)} onKeyDown={(event) => { if (event.key === " " || event.key === "Enter") { event.preventDefault(); setCompare(true); } }} onKeyUp={() => setCompare(false)}>{t("advancedEditor.compare")}</button>
        <button className={BUTTON} disabled={busy || unavailable} onClick={() => void saveProject()}>{t("advancedEditor.saveChanges")}</button>
        <button className="ui-primary rounded-xl px-4 py-2 text-xs disabled:opacity-40" disabled={busy || exporting || unavailable} onClick={() => void returnToLibrary(undefined, originSubgroup ?? null, true)}>{t("advancedEditor.saveReturn")}</button>

        <ActionPopover label={t("ui.more")} closeOnAction>
          <button className={BUTTON} disabled={busy || exporting || unavailable} onClick={() => void exportFolder()}>{t("editor.export")}</button>
          <button className={BUTTON} disabled={busy || exporting || unavailable} onClick={() => { albumCloseAfter.current = false; setAlbumPicker(true); }}>{t("editor.exportAlbum")}</button>
          <button className={BUTTON} disabled={busy || exporting} onClick={() => setConfirm("reset")}>{t("editor.reset")}</button>
        </ActionPopover>
        <button className={BUTTON} disabled={busy || exporting} onClick={requestClose}>{t("editor.close")}</button>
      </div>
    </header>
    <div className="flex min-h-0 flex-1 gap-2 p-2">
      <nav role="tablist" aria-orientation="vertical" aria-label={t("editor.tools")} className="ui-glass flex w-14 shrink-0 flex-col items-center gap-1 rounded-2xl py-2">
        {TOOLS.map(({ id, label, icon }) => <button key={id} role="tab" aria-selected={tool === id} aria-label={t(label)} title={t(label)} disabled={busy || exporting || unavailable} onClick={() => chooseTool(id)} className={`flex h-11 w-11 items-center justify-center rounded-xl transition-colors ${tool === id ? "bg-accent/15 text-accent" : "text-text-secondary hover:bg-panel"}`}><svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={icon} /></svg></button>)}
      </nav>
      <main onPointerMove={wakeZoom} className="relative flex min-h-0 min-w-0 flex-1 overflow-hidden rounded-2xl bg-[#202020]" data-theme="dark">
        {libraryChanged ? <div role="alert" className="m-auto max-w-sm p-5 text-center text-sm text-text-secondary">{t("advancedEditor.libraryChanged")}</div> : projectError ? <div role="alert" className="m-auto max-w-sm space-y-3 p-5 text-center text-sm text-text-secondary"><p>{projectError}</p><button className={BUTTON} onClick={() => setProjectRetry((v) => v + 1)}>{t("editor.reload")}</button></div> : preview.error ? <div role="alert" className="m-auto max-w-sm space-y-3 p-5 text-center text-sm text-text-secondary"><p>{preview.error}</p><button className={BUTTON} onClick={preview.reload}>{t("editor.reload")}</button></div> : <EditorCanvas
          sampleMode={picker !== null && !compare} onSample={samplePhoto}
          src={preview.session?.sourceUrl ?? null} adjustedSrc={preview.url} showOriginal={compare}
          sourceSize={sourceSize} nativeGeometry nativeAnnotations backendAdjustments fallbackSize={sourceSize} zoom={zoom}
          onZoom={(factor) => setZoom((v) => Math.max(0.25, Math.min(4, v * factor)))}
          recipe={compare ? { ...recipe, textLayers: [], brushStrokes: [] } : recipe}
          tool={compare || busy || exporting || unavailable ? "view" : tool} cropRatio={cropRatio} cropDraft={cropDraft} onCropDraftChange={setCropDraft}
          brushOptions={{ color, widthRel: brushWidth }} selectedTextId={selectedText} onSelectText={setSelectedText}
          onPlaceText={(pos) => { const id = newLayerId(); dispatch({ type: "textAdd", layer: { id, ...pos, text: "", sizeRel: 0.05, color } }); setSelectedText(id); }}
          onTextChange={(id, patch) => dispatch({ type: "textUpdateLive", id, patch })}
          onStrokeCommit={(points) => dispatch({ type: "strokeAdd", stroke: { id: newLayerId(), points, color, widthRel: brushWidth } })}
          onGestureStart={beginGesture} onGestureEnd={endGesture} onImageReady={() => {}} onImageError={preview.reload} />}
        {(preview.loading || projectLoading) && <div className="pointer-events-none absolute inset-0 flex items-center justify-center" role="status" aria-label={t("advancedEditor.loading")}><span className="h-8 w-8 animate-spin rounded-full border-2 border-white/20 border-t-accent" /></div>}
        <div ref={zoomControls} onPointerEnter={() => { zoomHover.current = true; wakeZoom(); }} onPointerLeave={() => { zoomHover.current = false; wakeZoom(); }} onFocusCapture={wakeZoom} onBlurCapture={wakeZoom} aria-hidden={!zoomVisible} inert={!zoomVisible} className={`absolute bottom-4 left-1/2 z-10 flex -translate-x-1/2 items-center gap-1 rounded-2xl border border-white/10 bg-black/60 p-1.5 shadow-xl backdrop-blur-xl transition-opacity ${zoomVisible ? "opacity-100" : "pointer-events-none opacity-0"}`}>
          <button className="h-8 w-8 rounded-lg text-text-secondary hover:bg-panel" aria-label={t("editor.zoomOut")} onClick={() => setZoom((v) => Math.max(0.25, v / 1.25))}>−</button>
          <button className="h-8 min-w-16 rounded-lg text-xs tabular-nums text-text-secondary hover:bg-panel" title={t("editor.zoomFit")} onClick={() => setZoom(1)}>{Math.round(zoom * 100)}%</button>
          <button className="h-8 w-8 rounded-lg text-text-secondary hover:bg-panel" aria-label={t("editor.zoomIn")} onClick={() => setZoom((v) => Math.min(4, v * 1.25))}>+</button>
          {preview.pending && <span role="status" aria-label={t("advancedEditor.previewUpdating")} className="mx-2 h-4 w-4 animate-spin rounded-full border border-white/20 border-t-accent" />}
        </div>
      </main>
      <aside className="ui-glass sp-scroll w-72 shrink-0 overflow-y-auto rounded-2xl p-4">
        {samplingError && <p role="alert" className="mb-2 text-xs text-amber-500">{samplingError}</p>}
        {preview.session && <p className="mb-2 text-[11px] text-text-muted">{preview.session.sensorRaw ? "RAW · " : ""}{preview.session.bitDepth}</p>}
        {preview.session?.warnings.map((warning) => <p key={warning} role="status" className="mb-2 text-xs text-amber-500">{warning}</p>)}
        <fieldset disabled={busy || exporting || compare || unavailable} className="space-y-3">
          <h3 className="text-sm font-semibold text-text-primary">{t(TOOLS.find((v) => v.id === tool)?.label ?? "advancedEditor.title")}</h3>
          <AdvancedToolPanel sampling={{ picker, setPicker, sample, histogram: preview.session?.histogram ?? [] }} tool={tool} recipe={recipe} dispatch={dispatch} beginGesture={beginGesture} endGesture={endGesture}
            context={context} cropDraft={cropDraft} setCropDraft={setCropDraft} setTool={setTool} cropRatio={cropRatio} setCropRatio={setCropRatio}
            color={color} setColor={setColor} selectedText={selectedText} setSelectedText={setSelectedText}
            brushWidth={brushWidth} setBrushWidth={setBrushWidth} exporting={exporting} exportFolder={exportFolder} />
        </fieldset>
      </aside>
    </div>
    <footer className="flex h-8 shrink-0 items-center gap-3 px-4 text-[11px] text-text-muted"><span>{sourceSize ? `${sourceSize.width} × ${sourceSize.height}` : t("advancedEditor.loading")}</span><span className="truncate" role="status">{notice ?? t("advancedEditor.projectHint")}</span></footer>
    {albumPicker && <ExportAlbumPicker albums={albums} onCancel={() => setAlbumPicker(false)} onConfirm={(albumId, subgroup) => { setAlbumPicker(false); void returnToLibrary(albumId, subgroup, albumCloseAfter.current); }} />}
    {confirm && <div className="fixed inset-0 z-[90] flex items-center justify-center bg-black/50"><div role="alertdialog" aria-modal="true" aria-labelledby="advanced-confirm-title" className="ui-glass w-96 space-y-4 rounded-2xl p-5"><h3 id="advanced-confirm-title" className="text-sm font-semibold text-text-primary">{t(confirm === "reset" ? "editor.resetConfirmTitle" : "editor.unsavedTitle")}</h3><p className="text-xs text-text-secondary">{t(confirm === "reset" ? "editor.resetConfirmDesc" : "advancedEditor.unsaved")}</p><div className="flex justify-end gap-2"><button className={BUTTON} onClick={() => { setConfirm(null); useAdvancedEditorStore.getState().cancel(); }}>{t("common.cancel")}</button>{confirm !== "reset" && <button className="ui-primary rounded-xl px-4 py-2 text-xs" disabled={busy || unavailable} onClick={() => { const action = confirm; void saveProject().then((ok) => { if (!ok || !mounted.current) return; setConfirm(null); if (action === "close") closeEditor(); else useAdvancedEditorStore.getState().accept(); }); }}>{t("advancedEditor.saveContinue")}</button>}<button className={BUTTON} disabled={busy} onClick={() => { const action = confirm; setConfirm(null); if (action === "close") closeEditor(); else if (action === "switch") useAdvancedEditorStore.getState().accept(); else { dispatch({ type: "reset" }); setProjectError(null); gesture.current = null; setCropDraft(null); setSelectedText(null); setTool("adjust"); } }}>{t(confirm === "reset" ? "editor.resetConfirmGo" : "editor.unsavedDiscard")}</button></div></div></div>}
  </div>;
}
