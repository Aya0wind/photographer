import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { convertFileSrc } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import {
  albumList,
  albumSubgroups,
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
  type ExportOptionsDraft,
} from "../lib/exportOptions";
import EditorCanvas, { type EditorTool } from "./EditorCanvas";
import ExportDialog from "./ExportDialog";

/**
 * 全屏编辑浮层（阶段 D 前端）：
 * - 非破坏：所有编辑只改配方状态；「保存配方」写 DB（editRecipeSave），「导出」
 *   经 exportRun 后台任务生成新 JPEG（album 模式入册并生成派生资产）。
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
}

export default function EditorOverlay({ asset, initial, onClose, onSaved }: EditorOverlayProps) {
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
  const dirty = !recipeEquals(recipeForPersist(present), baseline);

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
      if (next === "crop") {
        const rot = rotatedSize(ctxRef.current, present.rotateQuarter);
        setCropDraft(present.crop ?? fitCropRect(null, rot.w / rot.h));
        setCropRatio(null);
      } else {
        setCropDraft(null);
        setCropRatio(null);
      }
      if (next !== "text") setSelectedTextId(null);
      setTool(next);
    },
    [present.crop, present.rotateQuarter],
  );

  function applyCropDraft(): void {
    if (cropDraft === null) return;
    const clamped = clampCrop(cropDraft);
    dispatch({ type: "cropApply", crop: isFullCrop(clamped) ? null : clamped });
    setTool("view");
    setCropDraft(null);
    setCropRatio(null);
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
  const [exportOpen, setExportOpen] = useState(false);
  const [albums, setAlbums] = useState<AlbumDto[]>([]);
  const [subgroups, setSubgroups] = useState<string[]>([]);
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
    const albumId = Number(exportDraft.albumId);
    if (exportDraft.mode !== "album" || !Number.isInteger(albumId) || albumId <= 0) {
      setSubgroups([]);
      return;
    }
    let cancelled = false;
    void albumSubgroups(albumId).then((list) => {
      if (!cancelled) setSubgroups(list.map((g) => g.name));
    });
    return () => {
      cancelled = true;
    };
  }, [exportDraft.mode, exportDraft.albumId]);

  async function pickOutputDir(): Promise<void> {
    try {
      const dir = await openDialog({ directory: true });
      if (typeof dir === "string" && dir !== "") {
        setExportDraft((d) => ({ ...d, outputDir: dir }));
      }
    } catch {
      // 用户取消/对话框不可用：静默
    }
  }

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
      const state = await editRecipeSave(asset.id, recipeForPersist(present));
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
    setExportOpen(false);
    const result = await exportRun(asset.id, recipeForPersist(present), options);
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
        if (exportOpen) setExportOpen(false);
        else if (confirm !== null) setConfirm(null);
        else requestClose();
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

  const toolButton = (id: EditorTool, label: string, testId: string) => (
    <button
      type="button"
      onClick={() => selectTool(id)}
      aria-pressed={tool === id}
      title={label}
      className={`flex h-9 w-9 items-center justify-center rounded-md border text-xs transition-colors ${
        tool === id
          ? "border-accent bg-accent/10 text-accent"
          : "border-transparent text-text-secondary hover:bg-panel hover:text-text-primary"
      }`}
      data-testid={testId}
      data-active={tool === id}
    >
      {label.slice(0, 1)}
    </button>
  );

  return (
    <div
      className="fixed inset-0 z-[60] flex flex-col bg-black/95"
      role="dialog"
      aria-modal="true"
      aria-label={t("editor.title")}
      data-testid="editor-overlay"
    >
      {/* 顶栏 */}
      <div className="flex h-12 shrink-0 items-center gap-3 border-b border-edge px-4">
        <h2 className="flex min-w-0 items-baseline gap-2 text-sm font-semibold text-text-primary">
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
        <div className="ml-auto flex items-center gap-2">
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
            className="rounded-md border border-edge px-3 py-1.5 text-xs font-medium text-text-secondary transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="editor-save"
          >
            {t("editor.save")}
          </button>
          <button
            type="button"
            onClick={() => setExportOpen(true)}
            disabled={missing}
            title={missing ? t("editor.missingSource") : undefined}
            className="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
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

      <div className="flex min-h-0 flex-1">
        {/* 工具栏 */}
        <div className="flex w-14 shrink-0 flex-col items-center gap-1.5 border-r border-edge py-3" data-testid="editor-toolrail">
          {toolButton("view", t("editor.tool.view"), "editor-tool-view")}
          {toolButton("crop", t("editor.tool.crop"), "editor-tool-crop")}
          {toolButton("text", t("editor.tool.text"), "editor-tool-text")}
          {toolButton("brush", t("editor.tool.brush"), "editor-tool-brush")}
          <span className="my-1 h-px w-8 bg-edge" aria-hidden="true" />
          <button
            type="button"
            onClick={() => dispatch({ type: "rotate", delta: -1 })}
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
            onClick={() => dispatch({ type: "rotate", delta: 1 })}
            title={t("editor.rotateCw")}
            className="flex h-9 w-9 items-center justify-center rounded-md text-text-secondary transition-colors hover:bg-panel hover:text-text-primary"
            data-testid="editor-rotate-cw"
          >
            <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M12.5 6.5a5 5 0 1 0-1.2 5.4" />
              <path d="M12.8 3.2v3.3H9.5" />
            </svg>
          </button>
        </div>

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
        </div>

        {/* 右侧栏：工具选项 + 输出设置 */}
        <aside className="sp-scroll flex w-80 shrink-0 flex-col gap-4 overflow-y-auto border-l border-edge bg-surface p-3" data-testid="editor-sidebar">
          {/* 工具选项 */}
          {tool === "crop" && cropDraft !== null && (
            <section data-testid="editor-crop-options">
              <h3 className="mb-2 text-[10px] font-semibold uppercase tracking-wider text-text-muted">
                {t("editor.crop.presets")}
              </h3>
              <div className="mb-2 flex flex-wrap gap-1" data-testid="editor-crop-presets">
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
              <div className="flex gap-2">
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

          {/* 输出设置 */}
          <section className="border-t border-edge/60 pt-3" data-testid="editor-output-panel">
            <h3 className="mb-2 text-[10px] font-semibold uppercase tracking-wider text-text-muted">
              {t("editor.output.title")}
            </h3>
            <div className="mb-2 flex gap-2" role="radiogroup" aria-label={t("editor.output.mode")}>
              <label className="flex flex-1 cursor-pointer items-center gap-1.5 rounded-md border border-edge px-2 py-1.5 text-[11px] transition-colors has-[:checked]:border-accent has-[:checked]:text-accent">
                <input
                  type="radio"
                  name="export-mode"
                  checked={exportDraft.mode === "folder"}
                  onChange={() => setExportDraft((d) => ({ ...d, mode: "folder" }))}
                  className="h-3 w-3 accent-[#F0A83C]"
                  data-testid="editor-output-mode-folder"
                />
                {t("editor.output.mode.folder")}
              </label>
              <label className="flex flex-1 cursor-pointer items-center gap-1.5 rounded-md border border-edge px-2 py-1.5 text-[11px] transition-colors has-[:checked]:border-accent has-[:checked]:text-accent">
                <input
                  type="radio"
                  name="export-mode"
                  checked={exportDraft.mode === "album"}
                  onChange={() => setExportDraft((d) => ({ ...d, mode: "album" }))}
                  className="h-3 w-3 accent-[#F0A83C]"
                  data-testid="editor-output-mode-album"
                />
                {t("editor.output.mode.album")}
              </label>
            </div>

            {exportDraft.mode === "folder" ? (
              <div className="space-y-2">
                <div>
                  <span className="mb-1 block text-[11px] text-text-muted">{t("editor.output.dir")}</span>
                  <div className="flex gap-1.5">
                    <input
                      type="text"
                      readOnly
                      value={exportDraft.outputDir}
                      placeholder={t("editor.output.dirPlaceholder")}
                      className="min-w-0 flex-1 truncate rounded-md border border-edge bg-bg px-2 py-1.5 text-[11px] text-text-primary outline-none placeholder:text-text-muted/60"
                      data-testid="editor-output-dir"
                    />
                    <button
                      type="button"
                      onClick={() => void pickOutputDir()}
                      className="shrink-0 rounded-md border border-edge px-2 py-1.5 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
                      data-testid="editor-dir-pick"
                    >
                      {t("editor.output.dirPick")}
                    </button>
                  </div>
                </div>
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
              </div>
            ) : (
              <div className="space-y-2">
                <label className="block">
                  <span className="mb-1 block text-[11px] text-text-muted">{t("editor.output.album")}</span>
                  <select
                    value={exportDraft.albumId}
                    onChange={(e) => setExportDraft((d) => ({ ...d, albumId: e.target.value }))}
                    className="w-full rounded-md border border-edge bg-bg px-2 py-1.5 text-[11px] text-text-primary outline-none focus:border-accent"
                    data-testid="editor-output-album"
                  >
                    <option value="">{t("editor.output.albumPlaceholder")}</option>
                    {albums.map((album) => (
                      <option key={album.id} value={String(album.id)}>
                        {album.name}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="block">
                  <span className="mb-1 block text-[11px] text-text-muted">{t("editor.output.subgroup")}</span>
                  <input
                    type="text"
                    list="editor-subgroup-options"
                    value={exportDraft.subgroup}
                    onChange={(e) => setExportDraft((d) => ({ ...d, subgroup: e.target.value }))}
                    placeholder={t("editor.output.subgroupPlaceholder")}
                    className="w-full rounded-md border border-edge bg-bg px-2 py-1.5 text-[11px] text-text-primary outline-none placeholder:text-text-muted/60 focus:border-accent"
                    data-testid="editor-output-subgroup"
                  />
                  <datalist id="editor-subgroup-options">
                    {subgroups.map((name) => (
                      <option key={name} value={name} />
                    ))}
                  </datalist>
                </label>
              </div>
            )}

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

            <label className="mt-2 flex cursor-pointer items-center gap-2 text-[11px] text-text-secondary">
              <input
                type="checkbox"
                checked={exportDraft.removeGps}
                onChange={(e) => setExportDraft((d) => ({ ...d, removeGps: e.target.checked }))}
                className="h-3.5 w-3.5 accent-[#F0A83C]"
                data-testid="editor-removegps"
              />
              {t("editor.output.removeGps")}
            </label>

            <div className="mt-2 grid grid-cols-2 gap-2">
              <label className="block">
                <span className="mb-1 block text-[11px] text-text-muted">{t("editor.output.copyright")}</span>
                <input
                  type="text"
                  value={exportDraft.copyright}
                  onChange={(e) => setExportDraft((d) => ({ ...d, copyright: e.target.value }))}
                  className="w-full rounded-md border border-edge bg-bg px-2 py-1.5 text-[11px] text-text-primary outline-none focus:border-accent"
                  data-testid="editor-copyright"
                />
              </label>
              <label className="block">
                <span className="mb-1 block text-[11px] text-text-muted">{t("editor.output.author")}</span>
                <input
                  type="text"
                  value={exportDraft.author}
                  onChange={(e) => setExportDraft((d) => ({ ...d, author: e.target.value }))}
                  className="w-full rounded-md border border-edge bg-bg px-2 py-1.5 text-[11px] text-text-primary outline-none focus:border-accent"
                  data-testid="editor-author"
                />
              </label>
            </div>
            <label className="mt-2 block">
              <span className="mb-1 block text-[11px] text-text-muted">{t("editor.output.keywords")}</span>
              <input
                type="text"
                value={exportDraft.keywords}
                onChange={(e) => setExportDraft((d) => ({ ...d, keywords: e.target.value }))}
                placeholder={t("editor.output.keywordsPlaceholder")}
                className="w-full rounded-md border border-edge bg-bg px-2 py-1.5 text-[11px] text-text-primary outline-none placeholder:text-text-muted/60 focus:border-accent"
                data-testid="editor-keywords"
              />
            </label>
          </section>
        </aside>
      </div>

      {/* 导出确认弹窗 */}
      {exportOpen && (
        <ExportDialog
          asset={asset}
          recipe={present}
          draft={exportDraft}
          albums={albums}
          running={toast !== null && toast.kind === "export-running"}
          onCancel={() => setExportOpen(false)}
          onConfirm={(options) => void runExport(options)}
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
