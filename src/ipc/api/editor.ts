import { ipc } from "../index";
import {
  type EditAdjustments,
  type EditRecipe,
  type EditRecipeBrushStroke,
  type EditRecipeCrop,
  type EditRecipeState,
  type EditRecipeTextLayer,
  type EditableMetadata,
  type ExportOptions,
  type ExportRunResult,
  type ExportTask,
} from "./types";
import { INVOKE_UNAVAILABLE_PATTERN } from "./errors";

export async function assetMetadataGet(assetId: number): Promise<EditableMetadata> {
  return ipc<EditableMetadata>("asset_metadata_get", { assetId });
}
export async function assetMetadataSave(assetId: number, metadata: EditableMetadata): Promise<EditableMetadata> {
  return ipc<EditableMetadata>("asset_metadata_save", { assetId, metadata });
}

/** 脏数据容错：后端配方 JSON → EditRecipe 归一（形状异常返回 null，UI 回退默认配方） */
function normalizeEditRecipe(value: unknown): EditRecipe | null {
  if (value === null || typeof value !== "object") return null;
  const r = value as Record<string, unknown>;
  const numOf = (v: unknown): number => (typeof v === "number" && Number.isFinite(v) ? v : Number.NaN);
  const quarter = numOf(r.rotateQuarter);
  if (r.version !== 1 || ![0, 1, 2, 3].includes(quarter)) return null;
  const cropRaw = r.crop;
  let crop: EditRecipeCrop | null = null;
  if (cropRaw !== null && typeof cropRaw === "object") {
    const c = cropRaw as Record<string, unknown>;
    const x = numOf(c.x), y = numOf(c.y), w = numOf(c.w), h = numOf(c.h);
    if ([x, y, w, h].some((n) => Number.isNaN(n))) return null;
    crop = { x, y, w, h };
  }
  const textLayers: EditRecipeTextLayer[] = Array.isArray(r.textLayers)
    ? r.textLayers.filter(
        (l): l is EditRecipeTextLayer =>
          l !== null && typeof l === "object" &&
          typeof (l as EditRecipeTextLayer).id === "string" &&
          typeof (l as EditRecipeTextLayer).text === "string" &&
          typeof (l as EditRecipeTextLayer).color === "string" &&
          typeof (l as EditRecipeTextLayer).sizeRel === "number",
      )
    : [];
  const brushStrokes: EditRecipeBrushStroke[] = Array.isArray(r.brushStrokes)
    ? r.brushStrokes.filter(
        (s): s is EditRecipeBrushStroke =>
          s !== null && typeof s === "object" &&
          typeof (s as EditRecipeBrushStroke).id === "string" &&
          typeof (s as EditRecipeBrushStroke).color === "string" &&
          typeof (s as EditRecipeBrushStroke).widthRel === "number" &&
          Array.isArray((s as EditRecipeBrushStroke).points),
      )
    : [];
  const outputRaw = r.output;
  const output =
    outputRaw !== null && typeof outputRaw === "object"
      ? (outputRaw as Record<string, unknown>)
      : {};
  const longEdge = numOf(output.longEdge);
  const quality = numOf(output.quality);
  return {
    version: 1,
    ...(r.renderer === "photocraft" ? { renderer: "photocraft" as const } : {}),
    rotateQuarter: quarter as EditRecipe["rotateQuarter"],
    crop,
    textLayers,
    brushStrokes,
    ...(r.adjustments && typeof r.adjustments === "object" ? { adjustments: Object.fromEntries(["brightness", "contrast", "saturation"].map((key) => {
      const value = (r.adjustments as Record<string, unknown>)[key];
      return [key, typeof value === "number" && Number.isFinite(value) ? Math.max(-100, Math.min(100, value)) : 0];
    })) as unknown as EditAdjustments } : {}),
    output: {
      longEdge: Number.isNaN(longEdge) ? null : longEdge,
      quality: Number.isNaN(quality) ? 90 : Math.min(100, Math.max(1, Math.round(quality))),
    },
  };
}

export interface EditorPreviewSession {
  histogram: number[][];
  sensorRaw: boolean;
  bitDepth: string;
  warnings: string[];
  sessionId: string;
  sourceUrl: string;
  width: number;
  height: number;
}

export async function editPreviewOpen(assetId: number, libraryId: string): Promise<EditorPreviewSession> {
  return ipc<EditorPreviewSession>("edit_preview_open", { assetId: String(assetId), libraryId });
}

/** 返回二进制 JPEG；调用方拥有并负责释放 object URL。 */
export async function editPreviewRender(sessionId: string, recipe: EditRecipe, interactive = false): Promise<string> {
  const bytes = await ipc<ArrayBuffer | number[]>("edit_preview_render", { sessionId, recipe, interactive });
  return URL.createObjectURL(new Blob([new Uint8Array(bytes)], { type: "image/jpeg" }));
}

export async function editPreviewClose(sessionId: string): Promise<void> {
  await ipc<void>("edit_preview_close", { sessionId });
}

export async function editProjectSave(assetId: number, libraryId: string, recipe: EditRecipe): Promise<void> {
  await ipc<void>("edit_project_save", { assetId: String(assetId), libraryId, recipe });
}

export async function editProjectOpen(assetId: number, libraryId: string): Promise<EditRecipe | null> {
  return ipc<EditRecipe | null>("edit_project_open", { assetId: String(assetId), libraryId });
}

export async function advancedExport(assetId: number, libraryId: string, recipe: EditRecipe, sessionId: string, albumId?: number, subgroup?: string | null): Promise<ExportTask> {
  return ipc<ExportTask>("edit_advanced_export", { assetId: String(assetId), libraryId, recipe,
    albumId: albumId === undefined ? null : String(albumId), subgroup: subgroup ?? null, sessionId });
}

export async function advancedExportFolder(photo: { assetId: number; libraryId: string; sessionId: string }, recipe: EditRecipe, options: ExportOptions): Promise<ExportTask> {
  return ipc<ExportTask>("edit_advanced_export_folder", { request: { ...photo, assetId: String(photo.assetId), recipe, options } });
}

export async function editExportStatus(assetId: number, libraryId: string, jobId: number): Promise<ExportTask> {
  return ipc<ExportTask>("edit_export_status", { assetId: String(assetId), libraryId, jobId });
}

/** edit_recipe_get 结果归一 */
function normalizeEditRecipeState(value: unknown): EditRecipeState {
  if (value === null || typeof value !== "object") return { recipe: null, updatedAt: null };
  const r = value as Record<string, unknown>;
  return {
    recipe: normalizeEditRecipe(r.recipe),
    updatedAt: typeof r.updatedAt === "string" ? r.updatedAt : null,
  };
}

/** 读取资产的编辑配方（无配方/命令失败回退 { recipe: null }——UI 显示「无」态）。
 *  后端契约 asset_id 为字符串（edit.rs parse_asset_id），此处统一转换。 */
export async function editRecipeGet(assetId: number): Promise<EditRecipeState> {
  try {
    return normalizeEditRecipeState(await ipc<unknown>("edit_recipe_get", { assetId: String(assetId) }));
  } catch {
    return { recipe: null, updatedAt: null };
  }
}

/** 保存编辑配方（非破坏，仅写本应用数据库）。不 catch：失败文案由调用方 toast */
export async function editRecipeSave(assetId: number, recipe: EditRecipe): Promise<EditRecipeState> {
  return normalizeEditRecipeState(
    await ipc<unknown>("edit_recipe_save", { assetId: String(assetId), recipe }),
  );
}

/** 删除编辑配方（编辑器「重置」）。不 catch：失败文案透传给调用方 */
export async function editRecipeDelete(assetId: number): Promise<void> {
  await ipc<void>("edit_recipe_delete", { assetId: String(assetId) });
}

/** 启动导出后台任务（export_run；进度/完成经 exportTaskProgress/exportTaskFinished 事件）。
 *  业务错误（如目录不可写）透传原始 Err 文案；invoke 不可用 error=null。 */
export async function exportRun(
  assetId: number,
  recipe: EditRecipe,
  options: ExportOptions,
): Promise<ExportRunResult> {
  try {
    const task = await ipc<ExportTask>("export_run", {
      assetId: String(assetId),
      recipe,
      options,
    });
    if (
      task === null || typeof task !== "object" ||
      typeof task.id !== "number" || !Number.isFinite(task.id)
    ) {
      return { ok: false, error: null };
    }
    return { ok: true, task };
  } catch (err) {
    const message = err instanceof Error ? err.message : typeof err === "string" ? err : "";
    if (!message || INVOKE_UNAVAILABLE_PATTERN.test(message)) {
      return { ok: false, error: null };
    }
    return { ok: false, error: message };
  }
}

export function editPreviewPick(sessionId: string, recipe: EditRecipe, x: number, y: number, picker: string) {
  return ipc<{ sample?: [number, number, number]; points?: [number, number][]; red?: [number, number][]; green?: [number, number][]; blue?: [number, number][] }>("edit_preview_pick", { sessionId, recipe, x, y, picker });
}
