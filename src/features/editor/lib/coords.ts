import type { EditRecipeCrop } from "@/ipc/api";

/**
 * 编辑器坐标换算（纯函数，预览像素 ↔ 归一化）。
 *
 * 契约口径：文字/笔迹坐标相对「裁剪后画布」归一化（画布宽=1）。预览渲染时
 * 「画布」= 裁剪区域的显示像素盒（canvasDisp），所有换算都经由当前显示尺寸，
 * 保证预览像素与配方归一化坐标逐位一致。
 */

/** 显示像素盒（画布或旋转后整图） */
export interface Size {
  width: number;
  height: number;
}

export const MIN_CROP = 0.05;

/** 夹取到 [0,1] */
export function clamp01(v: number): number {
  if (Number.isNaN(v)) return 0;
  return Math.min(1, Math.max(0, v));
}

/** 预览像素 → 画布归一化（除以显示尺寸；非有限值回退 0） */
export function normFromPixel(px: number, py: number, box: Size): { x: number; y: number } {
  if (box.width <= 0 || box.height <= 0) return { x: 0, y: 0 };
  return { x: clamp01(px / box.width), y: clamp01(py / box.height) };
}

/** 画布归一化 → 预览像素（乘以显示尺寸） */
export function pixelFromNorm(x: number, y: number, box: Size): { x: number; y: number } {
  return { x: x * box.width, y: y * box.height };
}

/**
 * 裁剪矩形夹取：x/y/w/h 归一到 [0,1]，x+w / y+h 不越界，w/h 不小于 minRel。
 * 输入可能来自拖拽（反向/零尺寸），先取 min/max 扶正再夹取。
 */
export function clampCrop(
  rect: { x: number; y: number; w: number; h: number },
  minRel = MIN_CROP,
): EditRecipeCrop {
  const x0 = clamp01(Math.min(rect.x, rect.x + rect.w));
  const y0 = clamp01(Math.min(rect.y, rect.y + rect.h));
  const x1 = clamp01(Math.max(rect.x, rect.x + rect.w));
  const y1 = clamp01(Math.max(rect.y, rect.y + rect.h));
  let w = Math.max(minRel, x1 - x0);
  let h = Math.max(minRel, y1 - y0);
  // 撑满后可能越右/下界：优先保住 w/h 的下限，再回缩起点
  let x = x0;
  let y = y0;
  if (x + w > 1) x = Math.max(0, 1 - w);
  if (y + h > 1) y = Math.max(0, 1 - h);
  if (x + w > 1) w = 1 - x;
  if (y + h > 1) h = 1 - y;
  return { x, y, w, h };
}

/**
 * 预设比例的最大内接居中矩形（旋转后图像归一化坐标）。
 * @param ratio 目标宽高比（w/h）；null = 自由（90% 居中）
 * @param imageAspect 旋转后图像的宽高比（显示宽/显示高）
 */
export function fitCropRect(ratio: number | null, imageAspect: number): EditRecipeCrop {
  const safeAspect = imageAspect > 0 && Number.isFinite(imageAspect) ? imageAspect : 1;
  let w: number;
  let h: number;
  if (ratio === null) {
    w = 0.9;
    h = 0.9;
  } else {
    // 归一化坐标下 w/h 的真实像素比 = (w/h) × imageAspect，令其等于 ratio
    h = 0.9;
    w = (h * ratio) / safeAspect;
    if (w > 0.95) {
      w = 0.95;
      h = (w * safeAspect) / ratio;
    }
  }
  return { x: (1 - w) / 2, y: (1 - h) / 2, w, h };
}

/** 矩形面积（归一化）——裁剪空判定（w×h ≥ 0.999 视为全图） */
export function isFullCrop(crop: EditRecipeCrop | null): boolean {
  if (crop === null) return true;
  return crop.x <= 0.001 && crop.y <= 0.001 && crop.w >= 0.999 && crop.h >= 0.999;
}

/** 无损往返校验辅助：归一化 → 像素 → 归一化（应还原；浮点误差 < 1e-9 量级） */
export function roundTripNorm(x: number, y: number, box: Size): { x: number; y: number } {
  const px = pixelFromNorm(x, y, box);
  return normFromPixel(px.x, px.y, box);
}
