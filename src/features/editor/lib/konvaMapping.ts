import type {
  EditRecipeBrushStroke,
  EditRecipeCrop,
  EditRecipeTextLayer,
} from "@/ipc/api";
import type { Size } from "./coords";
import { clamp01 } from "./coords";

/**
 * recipe 归一化坐标 ↔ Konva 节点像素属性的换算（纯函数，不 import konva）。
 *
 * Konva 只做交互层；recipe JSON 是唯一真理源——所有节点状态在提交时序列化回
 * recipe，加载时由 recipe 反序列化。换算口径与契约逐位一致：
 * - 「画布」= 裁剪后画布的显示像素盒（宽 dispW、高 dispH）；
 * - Konva Text 的 (x, y) 即文本框左上角、align=left（契约锚点语义）；
 * - sizeRel ↔ fontSize：sizeRel = 字高/画布宽 → fontSize = sizeRel × dispW；
 * - widthRel ↔ strokeWidth：strokeWidth = widthRel × dispW；
 * - 裁剪 Rect 在「旋转后整图」显示盒（宽 fullW、高 fullH）内表达。
 */

/** 文字层 → Konva Text 节点属性（px） */
export function textLayerToNodeAttrs(
  layer: EditRecipeTextLayer,
  canvas: Size,
): { x: number; y: number; fontSize: number; text: string; fill: string } {
  return {
    x: layer.x * canvas.width,
    y: layer.y * canvas.height,
    fontSize: layer.sizeRel * canvas.width,
    text: layer.text,
    fill: layer.color,
  };
}

/** Konva Text 节点状态 → 文字层（归一化）。id 透传（新放置时由调用方生成） */
export function nodeAttrsToTextLayer(
  attrs: { x: number; y: number; fontSize: number; text: string; fill: string },
  canvas: Size,
  id: string,
): EditRecipeTextLayer {
  return {
    id,
    x: clamp01(canvas.width > 0 ? attrs.x / canvas.width : 0),
    y: clamp01(canvas.height > 0 ? attrs.y / canvas.height : 0),
    text: attrs.text,
    sizeRel: canvas.width > 0 ? attrs.fontSize / canvas.width : 0.05,
    color: attrs.fill,
  };
}

/**
 * Transformer 缩放收尾：把节点的 scale 烘焙进字号（等比缩放，角锚点）。
 * 返回应用后的节点状态（scale 复位为 1、fontSize 已吸收缩放）。
 */
export function bakeTextScale(
  attrs: { x: number; y: number; fontSize: number; scaleX: number },
): { x: number; y: number; fontSize: number; scaleX: 1 } {
  const scale = Number.isFinite(attrs.scaleX) && attrs.scaleX > 0 ? attrs.scaleX : 1;
  return { x: attrs.x, y: attrs.y, fontSize: attrs.fontSize * scale, scaleX: 1 };
}

/** 笔迹 → Konva Line 的扁平 points 数组 [x0,y0,x1,y1,...]（px） */
export function strokeToLinePoints(
  stroke: Pick<EditRecipeBrushStroke, "points">,
  canvas: Size,
): number[] {
  const out: number[] = [];
  for (const p of stroke.points) {
    out.push(p.x * canvas.width, p.y * canvas.height);
  }
  return out;
}

/** Konva Line 扁平 points → 归一化点列（画布基） */
export function linePointsToNorm(points: number[], canvas: Size): { x: number; y: number }[] {
  const out: { x: number; y: number }[] = [];
  for (let i = 0; i + 1 < points.length; i += 2) {
    out.push({
      x: clamp01(canvas.width > 0 ? points[i] / canvas.width : 0),
      y: clamp01(canvas.height > 0 ? points[i + 1] / canvas.height : 0),
    });
  }
  return out;
}

/** 画笔像素粗细 */
export function strokeWidthPx(widthRel: number, canvas: Size): number {
  return Math.max(1, widthRel * canvas.width);
}

/** 画笔提交：px 粗细 → widthRel */
export function pxToWidthRel(strokeWidth: number, canvas: Size): number {
  return canvas.width > 0 ? strokeWidth / canvas.width : 0.01;
}

/** 裁剪矩形 → Konva Rect 属性（旋转后整图显示盒内，px） */
export function cropToRectAttrs(
  crop: EditRecipeCrop,
  full: Size,
): { x: number; y: number; width: number; height: number } {
  return {
    x: crop.x * full.width,
    y: crop.y * full.height,
    width: crop.w * full.width,
    height: crop.h * full.height,
  };
}

/** Konva Rect 属性 → 裁剪矩形（归一化，宽松夹取交给调用方的 clampCrop） */
export function rectAttrsToCrop(
  attrs: { x: number; y: number; width: number; height: number },
  full: Size,
): EditRecipeCrop {
  return {
    x: clamp01(full.width > 0 ? attrs.x / full.width : 0),
    y: clamp01(full.height > 0 ? attrs.y / full.height : 0),
    w: Math.abs(attrs.width) / (full.width > 0 ? full.width : 1),
    h: Math.abs(attrs.height) / (full.height > 0 ? full.height : 1),
  };
}

// --- 往返映射（测试与回归锚点） ------------------------------------------------

/** 文字层 → 节点 → 文字层 往返（id 保留；归一化值容差 1e-9 由调用方断言） */
export function roundTripTextLayer(layer: EditRecipeTextLayer, canvas: Size): EditRecipeTextLayer {
  return nodeAttrsToTextLayer(textLayerToNodeAttrs(layer, canvas), canvas, layer.id);
}

/** 笔迹 → Line points → 笔迹 往返 */
export function roundTripStroke(
  stroke: Pick<EditRecipeBrushStroke, "points">,
  canvas: Size,
): { x: number; y: number }[] {
  return linePointsToNorm(strokeToLinePoints(stroke, canvas), canvas);
}

/** 裁剪 → Rect → 裁剪 往返 */
export function roundTripCrop(crop: EditRecipeCrop, full: Size): EditRecipeCrop {
  return rectAttrsToCrop(cropToRectAttrs(crop, full), full);
}
