import type {
  EditRecipe,
  EditRecipeBrushStroke,
  EditRecipeCrop,
  EditRecipeOutput,
  EditRecipeTextLayer,
} from "@/ipc/api";
import { clamp01, clampCrop } from "./coords";

/**
 * 编辑配方状态机（撤销/重做栈前端本地维护）。
 *
 * 坐标基（与 IPC 契约一致）：
 * - 基准图 = 未旋转的原图（ctx.width/height，取预览 naturalWidth/Height）；
 * - rotateQuarter 先作用于基准图得到「旋转后图像」，crop 相对其归一化；
 * - 文字/笔迹相对「裁剪后画布」归一化（画布宽=1）。
 *
 * 变换规则（保证几何操作后坐标基严格成立）：
 * - rotate(±1)：crop 矩形随图像旋转重写（+90 CW: {x:1-y-h, y:x, w:h, h:w}）；
 *   图层点逐点变换（+90 CW: (a,b)→(1-b,a)，经旧画布→旋转图→新画布两跳）；
 *   sizeRel/widthRel 按画布宽度像素比缩放（字号/笔宽像素值不变）。
 *   文字保持水平（配方无文字旋转字段），锚点随点变换——先旋转后裁剪的主流程
 *   不受影响；旋转在文字之后发生属边缘场景，锚点语义仍确定。
 * - cropApply：图层点经「旧画布→旋转图→新画布」重基，裁剪掉的区域外坐标
 *   夹取回 [0,1]；sizeRel/widthRel 按 w 比例缩放。
 * - undo/redo 恢复几何/图层快照，但保留当前 output（输出设置不随几何撤销）。
 */

/** 基准图（未旋转）像素尺寸 */
export interface RecipeContext {
  width: number;
  height: number;
}

export interface RecipeHistory {
  past: EditRecipe[];
  present: EditRecipe;
  future: EditRecipe[];
}

export type TextLayerPatch = Partial<Omit<EditRecipeTextLayer, "id">>;

export type RecipeAction =
  | { type: "rotate"; delta: 1 | -1 }
  | { type: "cropApply"; crop: EditRecipeCrop | null }
  | { type: "textAdd"; layer: EditRecipeTextLayer }
  | { type: "textUpdate"; id: string; patch: TextLayerPatch }
  | { type: "textRemove"; id: string }
  | { type: "strokeAdd"; stroke: EditRecipeBrushStroke }
  | { type: "setOutput"; patch: Partial<EditRecipeOutput> }
  | { type: "commitFrom"; snapshot: EditRecipe }
  | { type: "undo" }
  | { type: "redo" }
  | { type: "reset" };

const FULL_CROP: EditRecipeCrop = { x: 0, y: 0, w: 1, h: 1 };

/** 新建配方默认值（quality 90 为 JPEG 常用档） */
export function defaultRecipe(): EditRecipe {
  return {
    version: 1,
    rotateQuarter: 0,
    crop: null,
    textLayers: [],
    brushStrokes: [],
    output: { longEdge: null, quality: 90 },
  };
}

/** 以已保存配方（或默认）初始化历史栈 */
export function initRecipeHistory(recipe: EditRecipe | null): RecipeHistory {
  return { past: [], present: recipe ?? defaultRecipe(), future: [] };
}

/** 旋转后图像尺寸（基准图经 quarter×90° 顺时针） */
export function rotatedSize(ctx: RecipeContext, quarter: number): { w: number; h: number } {
  const swap = ((quarter % 4) + 4) % 4 % 2 === 1;
  const w = ctx.width > 0 ? ctx.width : 1;
  const h = ctx.height > 0 ? ctx.height : 1;
  return swap ? { w: h, h: w } : { w, h };
}

function pushHistory(state: RecipeHistory, next: EditRecipe): RecipeHistory {
  if (next === state.present) return state;
  return { past: [...state.past, state.present], present: next, future: [] };
}

/** +90° 顺时针：旧旋转图归一化点 (u,v) → 新旋转图 (1-v, u)；-90° 逆时针 → (v, 1-u) */
function rotatePointInImage(u: number, v: number, delta: 1 | -1): { u: number; v: number } {
  return delta === 1 ? { u: clamp01(1 - v), v: clamp01(u) } : { u: clamp01(v), v: clamp01(1 - u) };
}

/** 画布点 ↔ 旋转图归一化的双向换算 */
function canvasToImage(a: number, b: number, crop: EditRecipeCrop): { u: number; v: number } {
  return { u: crop.x + a * crop.w, v: crop.y + b * crop.h };
}
function imageToCanvas(u: number, v: number, crop: EditRecipeCrop): { a: number; b: number } {
  return { a: clamp01((u - crop.x) / crop.w), b: clamp01((v - crop.y) / crop.h) };
}

/** 旋转后画布像素宽度（字号/笔宽的换算基准） */
function canvasPixelWidth(ctx: RecipeContext, quarter: number, crop: EditRecipeCrop): number {
  return crop.w * rotatedSize(ctx, quarter).w;
}

/** 旋转 ±90°：变换 quarter + crop + 全部图层（点重基、尺寸按画布宽像素比缩放） */
function applyRotate(recipe: EditRecipe, delta: 1 | -1, ctx: RecipeContext): EditRecipe {
  const quarter = (((recipe.rotateQuarter + delta) % 4) + 4) % 4 as EditRecipe["rotateQuarter"];
  const oldCrop = recipe.crop ?? FULL_CROP;
  // crop 矩形整块旋转（角点变换后取包围盒；对 90° 步进即轴对齐交换）
  const tl = rotatePointInImage(oldCrop.x, oldCrop.y, delta);
  const br = rotatePointInImage(oldCrop.x + oldCrop.w, oldCrop.y + oldCrop.h, delta);
  const newCrop: EditRecipeCrop = {
    x: Math.min(tl.u, br.u),
    y: Math.min(tl.v, br.v),
    w: Math.abs(br.u - tl.u),
    h: Math.abs(br.v - tl.v),
  };
  const scale =
    canvasPixelWidth(ctx, recipe.rotateQuarter, oldCrop) /
    canvasPixelWidth(ctx, quarter, newCrop);
  const remap = (a: number, b: number): { a: number; b: number } => {
    const img = canvasToImage(a, b, oldCrop);
    const rotated = rotatePointInImage(img.u, img.v, delta);
    return imageToCanvas(rotated.u, rotated.v, newCrop);
  };
  return {
    ...recipe,
    rotateQuarter: quarter,
    crop: recipe.crop === null && newCrop.w >= 0.999 && newCrop.h >= 0.999 ? null : newCrop,
    textLayers: recipe.textLayers.map((layer) => {
      const p = remap(layer.x, layer.y);
      return { ...layer, x: p.a, y: p.b, sizeRel: layer.sizeRel * scale };
    }),
    brushStrokes: recipe.brushStrokes.map((stroke) => ({
      ...stroke,
      widthRel: stroke.widthRel * scale,
      points: stroke.points.map((pt) => {
        const p = remap(pt.x, pt.y);
        return { x: p.a, y: p.b };
      }),
    })),
  };
}

/** 应用新裁剪：图层从旧画布重基到新画布（画布外点夹取 [0,1]，尺寸按 w 比缩放） */
function rebaseLayersOnCrop(
  recipe: EditRecipe,
  next: EditRecipeCrop | null,
): EditRecipe {
  const oldCrop = recipe.crop ?? FULL_CROP;
  const newCrop = next === null ? FULL_CROP : clampCrop(next);
  const scale = oldCrop.w / newCrop.w;
  const remap = (a: number, b: number): { a: number; b: number } => {
    const img = canvasToImage(a, b, oldCrop);
    return imageToCanvas(img.u, img.v, newCrop);
  };
  return {
    ...recipe,
    crop: next === null ? null : newCrop,
    textLayers: recipe.textLayers.map((layer) => {
      const p = remap(layer.x, layer.y);
      return { ...layer, x: p.a, y: p.b, sizeRel: layer.sizeRel * scale };
    }),
    brushStrokes: recipe.brushStrokes.map((stroke) => ({
      ...stroke,
      widthRel: stroke.widthRel * scale,
      points: stroke.points.map((pt) => {
        const p = remap(pt.x, pt.y);
        return { x: p.a, y: p.b };
      }),
    })),
  };
}

/** 配方状态机（不可变；pushHistory 语义 = past 收纳旧 present、清空 future） */
export function recipeReducer(
  state: RecipeHistory,
  action: RecipeAction,
  ctx: RecipeContext = { width: 0, height: 0 },
): RecipeHistory {
  const present = state.present;
  switch (action.type) {
    case "rotate":
      return pushHistory(state, applyRotate(present, action.delta, ctx));
    case "cropApply":
      return pushHistory(state, rebaseLayersOnCrop(present, action.crop));
    case "textAdd":
      return pushHistory(state, { ...present, textLayers: [...present.textLayers, action.layer] });
    case "textUpdate": {
      const clampSize = (v: number | undefined): number | undefined =>
        v !== undefined && Number.isFinite(v) && v > 0 ? v : undefined;
      return {
        ...state,
        present: {
          ...present,
          textLayers: present.textLayers.map((l) => {
            if (l.id !== action.id) return l;
            const patch = action.patch;
            return {
              ...l,
              ...patch,
              x: patch.x !== undefined ? clamp01(patch.x) : l.x,
              y: patch.y !== undefined ? clamp01(patch.y) : l.y,
              sizeRel: clampSize(patch.sizeRel) ?? l.sizeRel,
            };
          }),
        },
      };
    }
    case "textRemove":
      return pushHistory(state, {
        ...present,
        textLayers: present.textLayers.filter((l) => l.id !== action.id),
      });
    case "strokeAdd":
      return pushHistory(state, {
        ...present,
        brushStrokes: [...present.brushStrokes, action.stroke],
      });
    case "setOutput":
      return { ...state, present: { ...present, output: { ...present.output, ...action.patch } } };
    case "commitFrom":
      // 连续手势（文字拖动）收尾：把手势前快照压入 past（present 已是终态）
      return { past: [...state.past, action.snapshot], present, future: [] };
    case "undo": {
      if (state.past.length === 0) return state;
      const previous = state.past[state.past.length - 1];
      return {
        past: state.past.slice(0, -1),
        // 输出设置不随几何撤销：保留当前 output
        present: { ...previous, output: present.output },
        future: [present, ...state.future],
      };
    }
    case "redo": {
      if (state.future.length === 0) return state;
      const next = state.future[0];
      return {
        past: [...state.past, present],
        present: { ...next, output: present.output },
        future: state.future.slice(1),
      };
    }
    case "reset":
      return initRecipeHistory(null);
  }
}

/** 键序无关规范化：后端 serde_json 无 preserve_order，回传配方为字母键序；
 * 本地对象为 TS 声明序，JSON.stringify 直比会误判不等（保存后恒脏）。 */
function canonicalize(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonicalize);
  if (value !== null && typeof value === "object") {
    const src = value as Record<string, unknown>;
    return Object.fromEntries(
      Object.keys(src)
        .sort()
        .map((key) => [key, canonicalize(src[key])]),
    );
  }
  return value;
}

/** 配方等值比较（键序无关；用于脏检查） */
export function recipeEquals(a: EditRecipe, b: EditRecipe): boolean {
  return JSON.stringify(canonicalize(a)) === JSON.stringify(canonicalize(b));
}

/** 持久化/导出前清洗：剔除空文字层与零点笔迹（放置后未输入的占位层不入库） */
export function recipeForPersist(recipe: EditRecipe): EditRecipe {
  return {
    ...recipe,
    textLayers: recipe.textLayers.filter((l) => l.text.trim() !== ""),
    brushStrokes: recipe.brushStrokes.filter((s) => s.points.length > 0),
  };
}

/** 图层 id（无 crypto 依赖，组件单进程内足够） */
export function newLayerId(): string {
  return `l-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}
