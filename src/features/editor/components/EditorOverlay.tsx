import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { convertFileSrc } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import {
  albumList,
  assetMetadataGet,
  assetMetadataSave,
  type EditableMetadata,
  editRecipeDelete,
  editRecipeSave,
  exportRun,
  subscribeAppEvents,
  type AlbumDto,
  type AssetDto,
  type EditRecipe,
  type ExportOptions,
} from "@/ipc/api";
import { useAssetThumbUrl } from "@/features/gallery/lib/thumbPipeline";
import {
  defaultRecipe,
  initRecipeHistory,
  newLayerId,
  recipeEquals,
  recipeForPersist,
  recipeReducer,
  rotatedSize,
  type RecipeContext,
} from "../lib/recipe";
import { clampCrop, fitCropRect, isFullCrop, type Size } from "../lib/coords";
import {
  defaultExportDraft,
  parseLongEdge,
  validateExportDraft,
  type ExportOptionsDraft,
} from "../lib/exportOptions";
import EditorCanvas, { type EditorTool } from "./EditorCanvas";
import ExportAlbumPicker from "./ExportAlbumPicker";
import MetadataFields from "./MetadataFields";

/**
 * 全屏编辑浮层（阶段 D 前端）：
 * - 非破坏：所有编辑只改配方状态；「保存配方」写 DB（editRecipeSave）；
 *   右上「导出」= 系统目录选择器确定即导出（folder 模式）、「加入相册」=
 *   程序内相册选择对话框（album 模式入册生成派生资产）。导出参数（文件名/
 *   长边/质量）取「输出」面板当前值——没有导出设置弹窗步骤。
 * - 撤销/重做栈本地维护（recipeReducer）；输出设置不随几何撤销。
 * - 交互层是 EditorCanvas（react-konva）；本组件持有工具态/面板/弹窗/toast。
 * - 未保存改动关闭需确认；重置需确认后 editRecipeDelete。
 */

const THUMB_SIZE = 1280;
const RAW_EMBED_SIZE = 6000;
const RAW_FALLBACK_SIZE = 2048;

/** 画笔/文字共用色板（照片标注常用色） */
const PALETTE = ["#FFFFFF", "#111111", "#F0A83C", "#FF5252", "#4FC3F7", "#66BB6A"];

/** 长边输入 → 配方同步（非法中间态不写配方，导出校验兜底） */
function syncLongEdge(value: string): { longEdge: number | null } | null {
  const parsed = parseLongEdge(value);
  return Number.isNaN(parsed) ? null : { longEdge: parsed };
}

function safeConvert(path: string): string | null {
  try {
    return convertFileSrc(path) || null;
  } catch {
    return null;
  }
}

function errorMessage(err: unknown): string {
  if (err instanceof Error && err.message !== "") return err.message;
  if (typeof err === "string" && err !== "") return err;
  return "";
}

type EditorToast =
  | { kind: "save-ok" }
  | { kind: "save-error"; message: string }
  | { kind: "reset-ok" }
  | { kind: "reset-error"; message: string }
  | { kind: "export-running"; phase: string }
  | { kind: "export-ok-folder"; path: string }
  | { kind: "export-ok-album" }
  | { kind: "export-error"; message: string };

interface EditorOverlayProps {
  asset: AssetDto;
  initial: { recipe: EditRecipe | null; updatedAt: string | null };
  onClose: () => void;
  /** 配方保存/重置后回传查看器（刷新「已编辑」角标与详情行） */
  onSaved?: (recipe: EditRecipe | null) => void;
  onMetadataSaved?: (metadata: EditableMetadata) => void;
}

export default function EditorOverlay({ asset, initial, onClose, onSaved, onMetadataSaved }: EditorOverlayProps) {
  const { t } = useTranslation();

  // --- 配方状态机（撤销栈） -----------------------------------------------------------
  const [baseSize, setBaseSize] = useState<Size | null>(null);
  const ctxRef = useRef<RecipeContext>({ width: 0, height: 0 });
  ctxRef.current = { width: baseSize?.width ?? 0, height: baseSize?.height ?? 0 };
  const [history, dispatch] = useReducer(
    (state: Parameters<typeof recipeReducer>[0], action: Parameters<typeof recipeReducer>[1]) =>
      recipeReducer(state, action, ctxRef.current),
    initial.recipe,
    initRecipeHistory,
  );
  const present = history.present;

  const [savedRecipe, setSavedRecipe] = useState<EditRecipe | null>(initial.recipe);
  const baseline = useMemo(
    () => recipeForPersist(savedRecipe ?? defaultRecipe()),
    [savedRecipe],
  );
  const recipeDirty = !recipeEquals(recipeForPersist(present), baseline);

  const [metadata, setMetadata] = useState<EditableMetadata | null>(null);
  const [savedMetadata, setSavedMetadata] = useState<EditableMetadata | null>(null);
  const [metadataError, setMetadataError] = useState<string | null>(null);
  const [metadataRetry, setMetadataRetry] = useState(0);
  useEffect(() => {
    let cancelled = false;
    setMetadataError(null);
    void assetMetadataGet(asset.id).then((value) => {
      if (!cancelled && value) { setMetadata(value); setSavedMetadata(value); }
      else if (!cancelled) setMetadataError("editor.metadata.readFailed");
    }).catch((error: unknown) => { if (!cancelled) setMetadataError(errorMessage(error) || "editor.metadata.readFailed"); });
    return () => { cancelled = true; };
  }, [asset.id, metadataRetry]);
  const metadataDirty = metadata !== null && JSON.stringify(metadata) !== JSON.stringify(savedMetadata);
  const dirty = recipeDirty || metadataDirty;
  const [zoom, setZoom] = useState(1);
  // --- 图源（复用查看器分级回退链） ---------------------------------------------------
  const originalUrl = useMemo(
    () => (asset.kind === "photo" ? safeConvert(asset.path) : null),
    [asset.kind, asset.path],
  );
  const [photoStage, setPhotoStage] = useState<"original" | "thumb">("original");
  useEffect(() => {
    setPhotoStage("original");
    setBaseSize(null);
  }, [asset.id, originalUrl]);
  const thumb = useAssetThumbUrl(asset.id, THUMB_SIZE, photoStage === "thumb" || asset.kind === "raw", "high");
  const rawEmbed = useAssetThumbUrl(asset.id, RAW_EMBED_SIZE, asset.kind === "raw", "high");
  const rawFull = useAssetThumbUrl(
    asset.id,
    RAW_FALLBACK_SIZE,
    asset.kind === "raw" && rawEmbed.status === "failed",
    "high",
  );
  const src =
    asset.kind === "photo"
      ? photoStage === "original" && originalUrl !== null
        ? originalUrl
        : thumb.url
      : (rawEmbed.url ?? rawFull.url ?? thumb.url);
  const failed =
    asset.kind === "photo"
      ? photoStage === "thumb" && thumb.status === "failed"
      : rawEmbed.status === "failed" && rawFull.status === "failed" && thumb.status === "failed";
  // 源文件被移动/删除（管线终态）：即便有历史缓存缩略图，编辑/导出都作用于源文件，
  // 缺源即无法编辑——画布区替换为缺失文案，保存/导出禁用。
  const missing = thumb.status === "missing";

  const fallbackSize = useMemo<Size | null>(
    () =>
      asset.width != null && asset.height != null && asset.width > 0 && asset.height > 0
        ? { width: asset.width, height: asset.height }
        : null,
    [asset.width, asset.height],
  );
  /** 等值短路：相同尺寸不落新引用（避免 EditorCanvas 的 ready effect 反复触发） */
  const handleImageReady = useCallback((size: Size) => {
    setBaseSize((prev) =>
      prev !== null && prev.width === size.width && prev.height === size.height ? prev : size,
    );
  }, []);

  // --- 工具与选项 ---------------------------------------------------------------------
  const [tool, setTool] = useState<EditorTool>("view");
  const [cropRatio, setCropRatio] = useState<number | null>(null);
  const [cropDraft, setCropDraft] = useState<Parameters<typeof clampCrop>[0] | null>(null);
  const [selectedTextId, setSelectedTextId] = useState<string | null>(null);
  /** 新放置文字层的默认样式（选中层的样式在选项面板逐层调整） */
  const [textOptions] = useState({ color: PALETTE[0], sizeRel: 0.06 });
  const [brushOptions, setBrushOptions] = useState({ color: "#FF5252", widthRel: 0.008 });

  const fullAspect = useMemo(() => {
    const rot = rotatedSize(ctxRef.current, present.rotateQuarter);
    return rot.w / rot.h;
  }, [baseSize, present.rotateQuarter]);

  const selectTool = useCallback(
    (next: EditorTool) => {
      if (next === tool) return;
      if (tool === "crop" && cropDraft && next !== "crop") {
        const crop = clampCrop(cropDraft);
        dispatch({ type: "cropApply", crop: isFullCrop(crop) ? null : crop });
      }
      if (next === "crop") {
        setCropDraft(present.crop ?? { x: 0, y: 0, w: 1, h: 1 });
        setCropRatio(null);
      } else {
        setCropDraft(null);
        setCropRatio(null);
      }
      if (next !== "text") setSelectedTextId(null);
      setTool(next);
      setZoom(1);
    },
    [present.crop, present.rotateQuarter, tool, cropDraft],
  );

  function applyCropDraft(): void {
    if (cropDraft === null) return;
    const clamped = clampCrop(cropDraft);
    dispatch({ type: "cropApply", crop: isFullCrop(clamped) ? null : clamped });
    setTool("view");
    setCropDraft(null);
    setCropRatio(null);
  }

  function rotateImage(delta: 1 | -1): void {
    let next = history;
    if (tool === "crop" && cropDraft) {
      const crop = clampCrop(cropDraft);
      const action = { type: "cropApply" as const, crop: isFullCrop(crop) ? null : crop };
      next = recipeReducer(next, action, ctxRef.current);
      dispatch(action);
    }
    next = recipeReducer(next, { type: "rotate", delta }, ctxRef.current);
    dispatch({ type: "rotate", delta });
    if (tool === "crop") { setCropDraft(next.present.crop ?? { x: 0, y: 0, w: 1, h: 1 }); setCropRatio(null); }
  }

  // --- 图层提交回调（Konva 交互层 → recipe 真理源） ------------------------------------
  const gestureSnapshotRef = useRef<EditRecipe | null>(null);
  function handleGestureStart(): void {
    gestureSnapshotRef.current = present;
  }
  function handleGestureEnd(): void {
    const snapshot = gestureSnapshotRef.current;
    gestureSnapshotRef.current = null;
    if (snapshot !== null && !recipeEquals(snapshot, present)) {
      dispatch({ type: "commitFrom", snapshot });
    }
  }
  const handlePlaceText = useCallback(
    (pos: { x: number; y: number }) => {
      const id = newLayerId();
      dispatch({
        type: "textAdd",
        layer: { id, x: pos.x, y: pos.y, text: "", sizeRel: textOptions.sizeRel, color: textOptions.color },
      });
      setSelectedTextId(id);
    },
    [textOptions.color, textOptions.sizeRel],
  );
  const handleTextChange = useCallback((id: string, patch: { x?: number; y?: number; text?: string; sizeRel?: number; color?: string }) => {
    dispatch({ type: "textUpdate", id, patch });
  }, []);
  const handleStrokeCommit = useCallback(
    (points: { x: number; y: number }[]) => {
      dispatch({
        type: "strokeAdd",
        stroke: { id: newLayerId(), color: brushOptions.color, widthRel: brushOptions.widthRel, points },
      });
    },
    [brushOptions.color, brushOptions.widthRel],
  );

  // --- 导出选项草稿 / 相册数据 ---------------------------------------------------------
  const [exportDraft, setExportDraft] = useState<ExportOptionsDraft>(() => ({
    ...defaultExportDraft(asset),
    longEdge: initial.recipe?.output.longEdge != null ? String(initial.recipe.output.longEdge) : "",
  }));
  const [albumPickerOpen, setAlbumPickerOpen] = useState(false);
  const [albums, setAlbums] = useState<AlbumDto[]>([]);
  useEffect(() => {
    let cancelled = false;
    void albumList().then((list) => {
      if (!cancelled) setAlbums(list);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (!savedMetadata) return;
    setExportDraft((draft) => ({ ...draft, author: savedMetadata.author, copyright: savedMetadata.copyright, keywords: savedMetadata.keywords.join(", ") }));
  }, [savedMetadata]);

  // --- toast / 后台导出事件（exportTaskProgress/Finished，按 jobId 关联） ----------------
  const [toast, setToast] = useState<EditorToast | null>(null);
  const activeJobRef = useRef<number | null>(null);
  useEffect(() => {
    let off: (() => void) | null = null;
    let cancelled = false;
    void subscribeAppEvents((event) => {
      if (event.type === "exportTaskProgress" && event.jobId === activeJobRef.current) {
        setToast({ kind: "export-running", phase: event.phase });
      } else if (event.type === "exportTaskFinished" && event.jobId === activeJobRef.current) {
        activeJobRef.current = null;
        if (event.ok) {
          setToast(
            event.outputPath
              ? { kind: "export-ok-folder", path: event.outputPath }
              : { kind: "export-ok-album" },
          );
        } else {
          setToast({ kind: "export-error", message: event.error ?? t("editor.exportFailed") });
        }
      }
    }).then((unlisten) => {
      if (cancelled) unlisten();
      else off = unlisten;
    });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [t]);

  const toastAutoMs =
    toast === null
      ? null
      : toast.kind === "save-ok" || toast.kind === "reset-ok"
        ? 2500
        : toast.kind.startsWith("export-ok") || toast.kind.endsWith("error")
          ? 6000
          : null;
  useEffect(() => {
    if (toastAutoMs === null) return;
    const timer = window.setTimeout(() => setToast(null), toastAutoMs);
    return () => window.clearTimeout(timer);
  }, [toast, toastAutoMs]);

  // --- 保存 / 重置 / 导出 ----------------------------------------------------------------
  const [saving, setSaving] = useState(false);
  async function save(): Promise<void> {
    if (saving) return;
    setSaving(true);
    try {
      if (metadataDirty && metadata) {
        const saved = await assetMetadataSave(asset.id, metadata);
        setMetadata(saved); setSavedMetadata(saved);
        onMetadataSaved?.(saved);
      }
      const toSave = tool === "crop" && cropDraft ? recipeReducer(history, { type: "cropApply", crop: isFullCrop(clampCrop(cropDraft)) ? null : clampCrop(cropDraft) }, ctxRef.current).present : present;
      const state = await editRecipeSave(asset.id, recipeForPersist(toSave));
      if (tool === "crop" && cropDraft) { applyCropDraft(); }
      setSavedRecipe(state.recipe);
      onSaved?.(state.recipe);
      setToast({ kind: "save-ok" });
    } catch (err) {
      const message = errorMessage(err);
      setToast({ kind: "save-error", message: message === "" ? t("editor.backendUnavailable") : message });
    } finally {
      setSaving(false);
    }
  }

  async function reset(): Promise<void> {
    setConfirm(null);
    try {
      await editRecipeDelete(asset.id);
      setToast({ kind: "reset-ok" });
    } catch (err) {
      // 删除失败（如后端未连接）也复位本地状态——用户意图明确；文案如实提示
      const message = errorMessage(err);
      setToast({ kind: "reset-error", message: message === "" ? t("editor.backendUnavailable") : message });
    }
    dispatch({ type: "reset" });
    setSavedRecipe(null);
    setSelectedTextId(null);
    setTool("view");
    onSaved?.(null);
  }

  async function runExport(options: ExportOptions): Promise<void> {
    const recipe = tool === "crop" && cropDraft ? recipeReducer(history, { type: "cropApply", crop: isFullCrop(clampCrop(cropDraft)) ? null : clampCrop(cropDraft) }, ctxRef.current).present : present;
    const result = await exportRun(asset.id, recipeForPersist(recipe), options);
    if (!result.ok) {
      setToast({
        kind: "export-error",
        message: result.error ?? t("editor.exportStartFailed"),
      });
      return;
    }
    activeJobRef.current = result.task.id;
    setToast({ kind: "export-running", phase: "render" });
  }

  /** 导出（右上按钮）：系统目录选择器 → 确定即导出；取消静默。
   *  文件名/长边/质量取「输出」面板当前值（2026-09-29 导出重做定案）。 */
  async function exportToFolder(): Promise<void> {
    if (missing) return;
    let dir: string | null = null;
    try {
      dir = await openDialog({ directory: true });
    } catch {
      return;
    }
    if (typeof dir !== "string" || dir === "") return;
    const validation = validateExportDraft(
      { ...exportDraft, mode: "folder", outputDir: dir },
      present.output.quality,
    );
    if (!validation.ok) {
      setToast({ kind: "export-error", message: validation.errors.map((code) => t(`editor.error.${code}`)).join("；") });
      return;
    }
    await runExport(validation.options);
  }

  /** 加入相册（右上按钮 → 程序内相册选择对话框）：成片以派生资产入册。
   *  文件名后端自动生成 {stem}_edit.jpg；子组取对话框输入。 */
  function exportToAlbum(albumId: number, subgroup: string | null): void {
    const validation = validateExportDraft(
      { ...exportDraft, mode: "album", albumId: String(albumId), subgroup: subgroup ?? "" },
      present.output.quality,
    );
    if (!validation.ok) {
      setToast({ kind: "export-error", message: validation.errors.map((code) => t(`editor.error.${code}`)).join("；") });
      return;
    }
    void runExport(validation.options);
  }

  // --- 关闭确认 / 快捷键 ------------------------------------------------------------------
  const [confirm, setConfirm] = useState<"close" | "reset" | null>(null);
  function requestClose(): void {
    if (tool === "crop") {
      setTool("view");
      setCropDraft(null);
      setCropRatio(null);
      return;
    }
    if (dirty) setConfirm("close");
    else onClose();
  }

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target;
      const inInput =
        target instanceof HTMLElement &&
        (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable);
      if (e.key === "Escape") {
        if (inInput) return; // 输入框内的 Esc 交给输入框（textarea 取消编辑等）
        e.preventDefault();
        if (albumPickerOpen) setAlbumPickerOpen(false);
        else if (confirm !== null) setConfirm(null);
        else requestClose();
        return;
      }
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        if (!missing) void save();
        return;
      }
      if (inInput) return;
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && (e.key === "z" || e.key === "Z")) {
        e.preventDefault();
        dispatch({ type: "undo" });
      } else if (
        (e.ctrlKey || e.metaKey) && (e.key === "y" || e.key === "Y" ||
          ((e.key === "z" || e.key === "Z") && e.shiftKey))
      ) {
        e.preventDefault();
        dispatch({ type: "redo" });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const selectedText = present.textLayers.find((l) => l.id === selectedTextId) ?? null;
  const canUndo = history.past.length > 0;
  const canRedo = history.future.length > 0;

  const TOOL_ICONS: Record<EditorTool, React.ReactNode> = {
    view: <><rect x="3" y="4" width="18" height="16" rx="2" /><path d="m4 17 5-5 4 4 3-3 4 4" /></>,
    crop: <><path d="M6 3v15h15M3 6h15v15" /></>,
    adjust: <><circle cx="12" cy="12" r="4" /><path d="M12 2v2m0 16v2M2 12h2m16 0h2M5 5l2 2m10 10 2 2M5 19l2-2M17 7l2-2" /></>,
    filters: <><circle cx="9" cy="9" r="5" /><circle cx="15" cy="15" r="5" /></>,
    text: <><path d="M5 5h14M12 5v14M8 19h8" /></>,
    brush: <><path d="m5 15 10-11 5 5-11 10H4zM13 6l5 5" /></>,
    metadata: <><circle cx="12" cy="12" r="9" /><path d="M12 11v6M12 7h.01" /></>,
    output: <><path d="M12 3v12m-4-4 4 4 4-4M4 16v5h16v-5" /></>,
  };
  const toolButton = (id: EditorTool, label: string, testId: string) => (
    <button type="button" role="tab" aria-selected={tool === id} aria-label={label} onClick={() => selectTool(id)} title={label}
      className={`flex h-14 min-w-14 flex-col items-center justify-center gap-1 border-b-2 px-3 text-[11px] transition-colors ${tool === id ? "border-accent text-accent" : "border-transparent text-text-secondary hover:bg-panel/40 hover:text-text-primary"}`}
      data-testid={testId} data-active={tool === id}>
      <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{TOOL_ICONS[id]}</svg>{label}
    </button>
  );

  return (
    <div
      className="fixed inset-0 z-[60] flex flex-col bg-[#202020]"
      role="dialog"
      aria-modal="true"
      aria-label={t("editor.title")}
      data-testid="editor-overlay"
    >
      {/* 顶栏 */}
      <div className="relative flex h-12 shrink-0 items-center gap-3 border-b border-edge px-4" data-testid="editor-titlebar">
        <div className="absolute inset-0" data-tauri-drag-region data-testid="editor-window-drag-region" />
        <h2 className="pointer-events-none relative flex min-w-0 items-baseline gap-2 text-sm font-semibold text-text-primary">
          <span>{t("editor.title")}</span>
          <span className="truncate font-mono text-xs font-normal text-text-muted" title={asset.name} data-testid="editor-asset-name">
            {asset.name}
          </span>
          {savedRecipe !== null && (
            <span className="shrink-0 rounded bg-accent/15 px-1.5 py-0.5 text-[10px] font-normal text-accent" data-testid="editor-edited-badge">
              {t("editor.editedBadge")}
            </span>
          )}
          {dirty && (
            <span className="shrink-0 text-[10px] font-normal text-text-muted" data-testid="editor-dirty">
              {t("editor.dirtyMark")}
            </span>
          )}
        </h2>
        <div className="relative ml-auto flex items-center gap-2">
          <button
            type="button"
            onClick={() => dispatch({ type: "undo" })}
            disabled={!canUndo}
            title={t("editor.undo")}
            className="rounded-md border border-edge px-2.5 py-1 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-30"
            data-testid="editor-undo"
          >
            {t("editor.undo")}
          </button>
          <button
            type="button"
            onClick={() => dispatch({ type: "redo" })}
            disabled={!canRedo}
            title={t("editor.redo")}
            className="rounded-md border border-edge px-2.5 py-1 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-30"
            data-testid="editor-redo"
          >
            {t("editor.redo")}
          </button>
          <button
            type="button"
            onClick={() => setConfirm("reset")}
            title={t("editor.reset")}
            className="rounded-md border border-edge px-2.5 py-1 text-xs text-text-secondary transition-colors hover:border-red-400 hover:text-red-400"
            data-testid="editor-reset"
          >
            {t("editor.reset")}
          </button>
          <span className="mx-1 h-5 w-px bg-edge" aria-hidden="true" />
          <button
            type="button"
            onClick={() => void save()}
            disabled={saving || missing}
            title={missing ? t("editor.missingSource") : undefined}
            className="h-9 rounded-md bg-accent px-4 text-xs font-semibold text-black shadow-sm transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="editor-save"
          >
            {t("editor.saveChanges")}</button>
          <button
            type="button"
            onClick={() => setAlbumPickerOpen(true)}
            disabled={missing}
            title={missing ? t("editor.missingSource") : undefined}
            className="h-9 rounded-md border border-edge bg-panel/40 px-4 text-xs font-medium text-text-secondary transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="editor-export-album"
          >
            {t("editor.exportAlbum")}
          </button>
          <button
            type="button"
            onClick={() => void exportToFolder()}
            disabled={missing}
            title={missing ? t("editor.missingSource") : undefined}
            className="h-9 rounded-md border border-edge bg-panel/40 px-4 text-xs font-medium text-text-secondary transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="editor-export"
          >
            {t("editor.export")}
          </button>
          <button
            type="button"
            onClick={requestClose}
            aria-label={t("editor.close")}
            className="rounded-md border border-edge px-2.5 py-1 text-xs text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
            data-testid="editor-close"
          >
            {t("editor.close")}
          </button>
        </div>
      </div>

      <div className="relative grid min-h-16 shrink-0 grid-cols-[1fr_auto_1fr] items-center gap-2 border-b border-edge/40 px-4">
        <div className="mr-auto flex shrink-0 items-center gap-1 rounded-lg border border-edge/60 bg-black/10 p-1">
          <button type="button" aria-label={t("editor.zoomOut")} onClick={() => setZoom((v) => Math.max(0.25, v / 1.25))} className="h-8 w-8 rounded text-lg text-text-secondary hover:bg-panel" data-testid="editor-zoom-out">−</button>
          <button type="button" onClick={() => setZoom(1)} className="h-8 min-w-16 rounded px-2 text-xs tabular-nums text-text-secondary hover:bg-panel" title={t("editor.zoomFit")} data-testid="editor-zoom-fit">{Math.round(zoom * 100)}%</button>
          <button type="button" aria-label={t("editor.zoomIn")} onClick={() => setZoom((v) => Math.min(4, v * 1.25))} className="h-8 w-8 rounded text-lg text-text-secondary hover:bg-panel" data-testid="editor-zoom-in">+</button>
        </div>
        <div role="tablist" aria-label={t("editor.tools")} className="flex items-center justify-center" data-testid="editor-toolrail">
          {toolButton("view", t("editor.tool.view"), "editor-tool-view")}
          {toolButton("crop", t("editor.tool.cropRotate"), "editor-tool-crop")}
          {toolButton("adjust", t("editor.tool.adjust"), "editor-tool-adjust")}
          {toolButton("filters", t("editor.tool.filters"), "editor-tool-filters")}
          {toolButton("text", t("editor.tool.text"), "editor-tool-text")}
          {toolButton("brush", t("editor.tool.brush"), "editor-tool-brush")}
          {toolButton("metadata", t("editor.tool.metadata"), "editor-tool-metadata")}
          {toolButton("output", t("editor.tool.output"), "editor-tool-output")}
        </div>
      </div>
      <div className="flex min-h-0 flex-1">
        {/* 画布区 */}
        <div className="relative flex min-h-0 min-w-0 flex-1 flex-col">
          {failed ? (
            <div className="flex flex-1 flex-col items-center justify-center gap-2 text-text-muted" data-testid="editor-load-failed">
              <span className="text-xs">{t("editor.loadFailed")}</span>
            </div>
          ) : missing ? (
            <div className="flex flex-1 flex-col items-center justify-center gap-2 text-text-muted" data-testid="editor-load-missing">
              <svg viewBox="0 0 24 24" width="40" height="40" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round" className="text-amber-400" aria-hidden="true">
                <rect x="3.5" y="4.5" width="17" height="15" rx="2" />
                <path d="M4.5 17l4.5-4.5 3.5 3.5 3-3 4 4" />
                <path d="M14.5 5.5l4 4M18.5 5.5l-4 4" />
              </svg>
              <span className="text-xs text-amber-300">{t("editor.missingSource")}</span>
            </div>
          ) : (
            <EditorCanvas
              src={src}
              zoom={zoom}
              onZoom={(factor) => setZoom((value) => Math.min(4, Math.max(0.25, value * factor)))}
              fallbackSize={fallbackSize}
              recipe={present}
              tool={tool}
              brushOptions={brushOptions}
              cropRatio={cropRatio}
              cropDraft={cropDraft}
              onCropDraftChange={setCropDraft}
              selectedTextId={selectedTextId}
              onSelectText={setSelectedTextId}
              onPlaceText={handlePlaceText}
              onTextChange={handleTextChange}
              onStrokeCommit={handleStrokeCommit}
              onGestureStart={handleGestureStart}
              onGestureEnd={handleGestureEnd}
              onImageReady={handleImageReady}
              onImageError={() => {
                if (asset.kind === "photo" && photoStage === "original") setPhotoStage("thumb");
              }}
            />
          )}
          {src === null && !failed && !missing && (
            <div className="pointer-events-none absolute bottom-4 left-1/2 -translate-x-1/2" data-testid="editor-loading">
              <div className="h-8 w-8 animate-spin rounded-full border-2 border-edge border-t-accent" />
            </div>
          )}
          {tool === "crop" && cropDraft !== null && (
            <section className="mx-auto w-full max-w-2xl shrink-0 px-5 pb-5 pt-3 text-center" data-testid="editor-crop-options">
              <div className="mb-2 flex items-center justify-center gap-4">
          <button
            type="button"
            onClick={() => rotateImage(-1)}
            title={t("editor.rotateCcw")}
            className="flex h-9 w-9 items-center justify-center rounded-md text-text-secondary transition-colors hover:bg-panel hover:text-text-primary"
            data-testid="editor-rotate-ccw"
          >
            <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M3.5 6.5a5 5 0 1 1 1.2 5.4" />
              <path d="M3.2 3.2v3.3h3.3" />
            </svg>
          </button>
          <button
            type="button"
            onClick={() => rotateImage(1)}
            title={t("editor.rotateCw")}
            className="flex h-9 w-9 items-center justify-center rounded-md text-text-secondary transition-colors hover:bg-panel hover:text-text-primary"
            data-testid="editor-rotate-cw"
          >
            <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M12.5 6.5a5 5 0 1 0-1.2 5.4" />
              <path d="M12.8 3.2v3.3H9.5" />
            </svg>
          </button>
<span className="min-w-12 text-sm tabular-nums text-text-secondary">{present.rotateQuarter * 90}°</span></div>
              <h3 className="mb-2 text-[10px] font-semibold uppercase tracking-wider text-text-muted">
                {t("editor.crop.presets")}
              </h3>
              <div className="mb-2 flex flex-wrap justify-center gap-2" data-testid="editor-crop-presets">
                {([
                  ["free", null],
                  ["1:1", 1],
                  ["4:3", 4 / 3],
                  ["3:4", 3 / 4],
                  ["16:9", 16 / 9],
                  ["9:16", 9 / 16],
                ] as const).map(([key, ratio]) => (
                  <button
                    key={key}
                    type="button"
                    onClick={() => {
                      setCropRatio(ratio);
                      if (ratio !== null) setCropDraft(fitCropRect(ratio, fullAspect));
                    }}
                    aria-pressed={cropRatio === ratio || (key === "free" && cropRatio === null)}
                    className={`rounded-md border px-2 py-1 text-[11px] transition-colors ${
                      cropRatio === ratio || (key === "free" && cropRatio === null)
                        ? "border-accent bg-accent/10 text-accent"
                        : "border-edge text-text-secondary hover:border-text-muted hover:text-text-primary"
                    }`}
                    data-testid={`editor-crop-preset-${key.replace(":", "-")}`}
                  >
                    {key === "free" ? t("editor.crop.free") : key}
                  </button>
                ))}
              </div>
              <p className="mb-2 text-[11px] leading-relaxed text-text-muted">{t("editor.crop.hint")}</p>
              <div className="mx-auto flex max-w-xs gap-2">
                <button
                  type="button"
                  onClick={applyCropDraft}
                  className="flex-1 rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110"
                  data-testid="editor-crop-apply"
                >
                  {t("editor.crop.apply")}
                </button>
                <button
                  type="button"
                  onClick={() => {
                    setTool("view");
                    setCropDraft(null);
                    setCropRatio(null);
                  }}
                  className="flex-1 rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
                  data-testid="editor-crop-cancel"
                >
                  {t("editor.crop.cancel")}
                </button>
              </div>
              {present.crop !== null && (
                <button
                  type="button"
                  onClick={() => setCropDraft({ x: 0, y: 0, w: 1, h: 1 })}
                  className="mt-2 w-full rounded-md border border-edge px-3 py-1.5 text-[11px] text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
                  data-testid="editor-crop-clear"
                >
                  {t("editor.crop.fullFrame")}
                </button>
              )}
            </section>
          )}

        </div>

        {/* 右侧栏：工具选项 + 输出设置 */}
        {["text", "brush", "adjust", "filters", "metadata", "output"].includes(tool) && <aside className="sp-scroll flex w-80 shrink-0 flex-col gap-4 overflow-y-auto border-l border-edge bg-surface p-3" data-testid="editor-sidebar">
          {tool === "metadata" && (metadata ? <MetadataFields value={metadata} onChange={(value) => {
            setMetadata(value);
            setExportDraft((draft) => ({ ...draft, author: value.author, copyright: value.copyright, keywords: value.keywords.join(", "), removeGps: value.gpsLat === null && value.gpsLon === null }));
          }} /> : metadataError ? <div role="alert" className="space-y-3 text-sm text-red-400"><p>{t(metadataError, { defaultValue: metadataError })}</p><button type="button" className="rounded-md border border-edge px-3 py-2 text-text-primary" onClick={() => setMetadataRetry((v) => v + 1)}>{t("editor.reload")}</button></div> : <div className="flex items-center gap-3 text-sm text-text-muted"><span className="h-5 w-5 animate-spin rounded-full border-2 border-edge border-t-accent" />{t("editor.metadata.loading")}</div>)}
          {tool === "adjust" && <section className="space-y-6"><h3 className="text-sm font-semibold text-text-primary">{t("editor.adjust.title")}</h3>{([['brightness', t("editor.adjust.brightness")], ['contrast', t("editor.adjust.contrast")], ['saturation', t("editor.adjust.saturation")]] as const).map(([key, label]) => <label key={key} className="block"><span className="mb-3 flex justify-between text-xs text-text-secondary">{label}<span className="tabular-nums">{present.adjustments?.[key] ?? 0}</span></span><input type="range" min="-100" max="100" value={present.adjustments?.[key] ?? 0} onPointerDown={handleGestureStart} onPointerUp={handleGestureEnd} onChange={(e) => dispatch({ type: "adjust", patch: { [key]: Number(e.target.value) }, record: gestureSnapshotRef.current === null })} className="w-full accent-[#F0A83C]" data-testid={`editor-adjust-${key}`} /></label>)}<button type="button" className="rounded-md border border-edge px-3 py-2 text-xs text-text-secondary" onClick={() => dispatch({ type: "adjust", patch: { brightness: 0, contrast: 0, saturation: 0 } })}>{t("editor.adjust.reset")}</button></section>}
          {tool === "filters" && <section className="space-y-4"><h3 className="text-sm font-semibold text-text-primary">{t("editor.tool.filters")}</h3><div className="grid grid-cols-2 gap-2">{([
            [t("editor.filter.original"), 0, 0, 0], [t("editor.filter.vivid"), 5, 15, 25], [t("editor.filter.soft"), 8, -15, -10], [t("editor.filter.monochrome"), 0, 10, -100],
          ] as const).map(([name, brightness, contrast, saturation]) => <button key={name} type="button" onClick={() => dispatch({ type: "adjust", patch: { brightness, contrast, saturation } })} aria-pressed={(present.adjustments?.brightness ?? 0) === brightness && (present.adjustments?.contrast ?? 0) === contrast && (present.adjustments?.saturation ?? 0) === saturation} className="h-20 rounded-lg border border-edge bg-panel text-sm text-text-secondary hover:border-accent aria-pressed:border-accent aria-pressed:bg-accent/10 aria-pressed:text-accent">{name}</button>)}</div></section>}
          {/* 工具选项 */}
          {tool === "text" && (
            <section data-testid="editor-text-options">
              <h3 className="mb-2 text-[10px] font-semibold uppercase tracking-wider text-text-muted">
                {t("editor.tool.text")}
              </h3>
              {selectedText === null ? (
                <p className="text-[11px] leading-relaxed text-text-muted">{t("editor.text.hint")}</p>
              ) : (
                <div className="space-y-2">
                  <label className="block">
                    <span className="mb-1 block text-[11px] text-text-muted">{t("editor.text.content")}</span>
                    <textarea
                      key={selectedText.id}
                      value={selectedText.text}
                      onChange={(e) => handleTextChange(selectedText.id, { text: e.target.value })}
                      rows={3}
                      placeholder={t("editor.text.placeholder")}
                      className="w-full resize-none rounded-md border border-edge bg-bg px-2 py-1.5 text-xs text-text-primary outline-none transition-colors placeholder:text-text-muted/60 focus:border-accent"
                      data-testid="editor-text-input"
                    />
                  </label>
                  <label className="block">
                    <span className="mb-1 flex items-baseline justify-between text-[11px] text-text-muted">
                      {t("editor.text.size")}
                      <span className="font-mono tabular-nums">{Math.round(selectedText.sizeRel * 1000) / 1000}</span>
                    </span>
                    <input
                      type="range"
                      min={0.02}
                      max={0.25}
                      step={0.005}
                      value={selectedText.sizeRel}
                      onChange={(e) => handleTextChange(selectedText.id, { sizeRel: Number(e.target.value) })}
                      className="w-full accent-[#F0A83C]"
                      data-testid="editor-text-size"
                    />
                  </label>
                  <div>
                    <span className="mb-1 block text-[11px] text-text-muted">{t("editor.text.color")}</span>
                    <div className="flex items-center gap-1.5">
                      {PALETTE.map((color) => (
                        <button
                          key={color}
                          type="button"
                          aria-label={color}
                          onClick={() => handleTextChange(selectedText.id, { color })}
                          className={`h-5 w-5 rounded-full border transition-transform hover:scale-110 ${
                            selectedText.color === color ? "border-accent" : "border-edge"
                          }`}
                          style={{ backgroundColor: color }}
                          data-testid="editor-text-color"
                          data-color={color}
                        />
                      ))}
                      <input
                        type="color"
                        value={selectedText.color}
                        onChange={(e) => handleTextChange(selectedText.id, { color: e.target.value })}
                        className="h-5 w-6 cursor-pointer rounded border border-edge bg-transparent"
                        aria-label={t("editor.text.customColor")}
                        data-testid="editor-text-custom-color"
                      />
                    </div>
                  </div>
                  <button
                    type="button"
                    onClick={() => {
                      dispatch({ type: "textRemove", id: selectedText.id });
                      setSelectedTextId(null);
                    }}
                    className="w-full rounded-md border border-edge px-3 py-1.5 text-[11px] text-text-secondary transition-colors hover:border-red-400 hover:text-red-400"
                    data-testid="editor-text-delete"
                  >
                    {t("editor.text.delete")}
                  </button>
                </div>
              )}
            </section>
          )}

          {tool === "brush" && (
            <section data-testid="editor-brush-options">
              <h3 className="mb-2 text-[10px] font-semibold uppercase tracking-wider text-text-muted">
                {t("editor.tool.brush")}
              </h3>
              <p className="mb-2 text-[11px] leading-relaxed text-text-muted">{t("editor.brush.hint")}</p>
              <div className="mb-2">
                <span className="mb-1 block text-[11px] text-text-muted">{t("editor.brush.color")}</span>
                <div className="flex items-center gap-1.5">
                  {PALETTE.map((color) => (
                    <button
                      key={color}
                      type="button"
                      aria-label={color}
                      onClick={() => setBrushOptions((o) => ({ ...o, color }))}
                      className={`h-5 w-5 rounded-full border transition-transform hover:scale-110 ${
                        brushOptions.color === color ? "border-accent" : "border-edge"
                      }`}
                      style={{ backgroundColor: color }}
                      data-testid="editor-brush-color"
                      data-color={color}
                    />
                  ))}
                </div>
              </div>
              <label className="block">
                <span className="mb-1 flex items-baseline justify-between text-[11px] text-text-muted">
                  {t("editor.brush.width")}
                  <span className="font-mono tabular-nums">{Math.round(brushOptions.widthRel * 1000) / 1000}</span>
                </span>
                <input
                  type="range"
                  min={0.002}
                  max={0.04}
                  step={0.001}
                  value={brushOptions.widthRel}
                  onChange={(e) => setBrushOptions((o) => ({ ...o, widthRel: Number(e.target.value) }))}
                  className="w-full accent-[#F0A83C]"
                  data-testid="editor-brush-width"
                />
              </label>
            </section>
          )}

          {/* 输出参数：文件名/长边/质量——“导出”“加入相册”按钮在顶栏；
              目录经系统选择器、相册经程序内对话框，均无导出设置弹窗步骤 */}
          {tool === "output" && <section className="space-y-2" data-testid="editor-output-panel">
            <h3 className="mb-2 text-[10px] font-semibold uppercase tracking-wider text-text-muted">
              {t("editor.output.title")}
            </h3>
            <label className="block">
              <span className="mb-1 block text-[11px] text-text-muted">{t("editor.output.fileName")}</span>
              <input
                type="text"
                value={exportDraft.fileName}
                onChange={(e) => setExportDraft((d) => ({ ...d, fileName: e.target.value }))}
                className="w-full rounded-md border border-edge bg-bg px-2 py-1.5 text-[11px] text-text-primary outline-none transition-colors focus:border-accent"
                data-testid="editor-output-filename"
              />
            </label>

            <div className="mt-2 flex gap-2">
              <label className="flex-1">
                <span className="mb-1 block text-[11px] text-text-muted">{t("editor.output.longEdge")}</span>
                <input
                  type="text"
                  inputMode="numeric"
                  value={exportDraft.longEdge}
                  onChange={(e) => {
                    const value = e.target.value;
                    setExportDraft((d) => ({ ...d, longEdge: value }));
                    const patch = syncLongEdge(value);
                    if (patch !== null) dispatch({ type: "setOutput", patch });
                  }}
                  placeholder={t("editor.output.longEdgePlaceholder")}
                  className="w-full rounded-md border border-edge bg-bg px-2 py-1.5 text-[11px] text-text-primary outline-none placeholder:text-text-muted/60 focus:border-accent"
                  data-testid="editor-longedge"
                />
              </label>
              <label className="flex-1">
                <span className="mb-1 flex items-baseline justify-between text-[11px] text-text-muted">
                  {t("editor.output.quality")}
                  <span className="font-mono tabular-nums" data-testid="editor-quality-value">{present.output.quality}</span>
                </span>
                <input
                  type="range"
                  min={1}
                  max={100}
                  step={1}
                  value={present.output.quality}
                  onChange={(e) => dispatch({ type: "setOutput", patch: { quality: Number(e.target.value) } })}
                  className="mt-3 w-full accent-[#F0A83C]"
                  data-testid="editor-quality"
                />
              </label>
            </div>

          </section>}
        </aside>}
      </div>

      {/* 加入相册导出目标（程序内对话框）：确认即 album 模式导出 */}
      {albumPickerOpen && (
        <ExportAlbumPicker
          albums={albums}
          onCancel={() => setAlbumPickerOpen(false)}
          onConfirm={(albumId, subgroup) => {
            setAlbumPickerOpen(false);
            exportToAlbum(albumId, subgroup);
          }}
        />
      )}

      {/* 确认弹窗（关闭有未保存 / 重置） */}
      {confirm !== null && (
        <div className="fixed inset-0 z-[90] flex items-center justify-center bg-black/50" data-testid="editor-confirm-overlay">
          <div
            role="alertdialog"
            aria-modal="true"
            className="w-[360px] overflow-hidden rounded-xl border border-edge bg-surface p-4 shadow-2xl"
            data-testid="editor-confirm-dialog"
          >
            <h3 className="mb-1.5 text-sm font-semibold text-text-primary">
              {confirm === "close" ? t("editor.unsavedTitle") : t("editor.resetConfirmTitle")}
            </h3>
            <p className="mb-3 text-xs leading-relaxed text-text-secondary">
              {confirm === "close" ? t("editor.unsavedDesc") : t("editor.resetConfirmDesc")}
            </p>
            <div className="flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setConfirm(null)}
                className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
                data-testid="editor-confirm-cancel"
              >
                {confirm === "close" ? t("editor.unsavedKeep") : t("common.cancel")}
              </button>
              <button
                type="button"
                onClick={() => {
                  if (confirm === "close") onClose();
                  else void reset();
                }}
                className={`rounded-md px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 ${
                  confirm === "reset" ? "bg-red-500" : "bg-accent"
                }`}
                data-testid="editor-confirm-ok"
              >
                {confirm === "close" ? t("editor.unsavedDiscard") : t("editor.resetConfirmGo")}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* toast（保存/导出进度与结果） */}
      {toast !== null && (
        <div
          className="fixed bottom-6 left-1/2 z-[95] -translate-x-1/2"
          role="status"
          data-testid="editor-toast"
          data-kind={toast.kind}
        >
          <div className="flex max-w-[560px] items-center gap-3 rounded-full border border-edge bg-surface px-4 py-2 text-xs text-text-secondary shadow-xl">
            {toast.kind === "export-running" && (
              <span className="flex items-center gap-2">
                <span className="h-3.5 w-3.5 animate-spin rounded-full border-2 border-edge border-t-accent" aria-hidden="true" />
                <span data-testid="editor-toast-phase">
                  {t(`editor.exportPhase.${toast.phase}`, { defaultValue: toast.phase })}
                </span>
              </span>
            )}
            <span className="min-w-0 truncate">
              {toast.kind === "save-ok"
                ? t("editor.saveSaved")
                : toast.kind === "save-error"
                  ? `${t("editor.saveFailed")}：${toast.message}`
                  : toast.kind === "reset-ok"
                    ? t("editor.resetDone")
                    : toast.kind === "reset-error"
                      ? `${t("editor.resetFailed")}：${toast.message}`
                      : toast.kind === "export-running"
                        ? t("editor.exportRunning")
                        : toast.kind === "export-ok-folder"
                          ? t("editor.exportDoneFolder", { path: toast.path })
                          : toast.kind === "export-ok-album"
                            ? t("editor.exportDoneAlbum")
                            : `${t("editor.exportFailed")}：${toast.message}`}
            </span>
            {toast.kind !== "export-running" && (
              <button
                type="button"
                onClick={() => setToast(null)}
                aria-label={t("common.close")}
                className="shrink-0 text-text-muted transition-colors hover:text-text-primary"
                data-testid="editor-toast-close"
              >
                ×
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
