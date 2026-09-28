import { describe, expect, it } from "vitest";

import { clamp01, clampCrop, fitCropRect, isFullCrop, normFromPixel, pixelFromNorm, roundTripNorm } from "./coords";

describe("坐标换算：预览像素 ↔ 归一化", () => {
  const box = { width: 1200, height: 800 };

  it("像素 → 归一化 → 像素 无损往返", () => {
    const cases: [number, number][] = [
      [0, 0],
      [600, 400],
      [1200, 800],
      [333, 777],
    ];
    for (const [px, py] of cases) {
      const norm = normFromPixel(px, py, box);
      const back = pixelFromNorm(norm.x, norm.y, box);
      expect(back.x).toBeCloseTo(px, 6);
      expect(back.y).toBeCloseTo(py, 6);
    }
  });

  it("归一化 → 像素 → 归一化 无损往返（roundTripNorm）", () => {
    for (const [x, y] of [[0, 0], [0.5, 0.25], [1, 1], [0.123456, 0.654321]] as const) {
      const back = roundTripNorm(x, y, box);
      expect(back.x).toBeCloseTo(x, 9);
      expect(back.y).toBeCloseTo(y, 9);
    }
  });

  it("越界像素夹取到 [0,1]", () => {
    expect(normFromPixel(-50, -50, box)).toEqual({ x: 0, y: 0 });
    expect(normFromPixel(9999, 9999, box)).toEqual({ x: 1, y: 1 });
  });

  it("零尺寸盒回退 0（不产生 NaN）", () => {
    expect(normFromPixel(100, 100, { width: 0, height: 0 })).toEqual({ x: 0, y: 0 });
    expect(clamp01(Number.NaN)).toBe(0);
  });
});

describe("clampCrop 裁剪夹取", () => {
  it("正常矩形原样保留", () => {
    const rect = clampCrop({ x: 0.1, y: 0.2, w: 0.5, h: 0.4 });
    expect(rect.x).toBeCloseTo(0.1, 9);
    expect(rect.y).toBeCloseTo(0.2, 9);
    expect(rect.w).toBeCloseTo(0.5, 9);
    expect(rect.h).toBeCloseTo(0.4, 9);
  });

  it("反向拖拽（负宽高）扶正", () => {
    const rect = clampCrop({ x: 0.6, y: 0.6, w: -0.3, h: -0.2 });
    expect(rect.x).toBeCloseTo(0.3);
    expect(rect.y).toBeCloseTo(0.4);
    expect(rect.w).toBeCloseTo(0.3);
    expect(rect.h).toBeCloseTo(0.2);
  });

  it("越界夹回画面内并保住最小尺寸", () => {
    const rect = clampCrop({ x: 0.95, y: 0.95, w: 0.2, h: 0.2 });
    expect(rect.x + rect.w).toBeLessThanOrEqual(1 + 1e-9);
    expect(rect.y + rect.h).toBeLessThanOrEqual(1 + 1e-9);
    expect(rect.w).toBeGreaterThanOrEqual(0.05 - 1e-9);
    expect(rect.h).toBeGreaterThanOrEqual(0.05 - 1e-9);
  });
});

describe("fitCropRect 预设比例", () => {
  it("自由=90% 居中", () => {
    const rect = fitCropRect(null, 4 / 3);
    expect(rect.x).toBeCloseTo(0.05, 9);
    expect(rect.y).toBeCloseTo(0.05, 9);
    expect(rect.w).toBeCloseTo(0.9, 9);
    expect(rect.h).toBeCloseTo(0.9, 9);
  });

  it("1:1 在 3:2 横幅图像上：归一化宽小于高（像素上为正方形）", () => {
    const imageAspect = 4000 / 3000;
    const rect = fitCropRect(1, imageAspect);
    // 像素宽高比 = (w/h)×imageAspect = 1
    expect((rect.w / rect.h) * imageAspect).toBeCloseTo(1, 6);
    expect(rect.x + rect.w / 2).toBeCloseTo(0.5, 6);
    expect(rect.y + rect.h / 2).toBeCloseTo(0.5, 6);
    expect(rect.w).toBeLessThanOrEqual(0.95 + 1e-9);
    expect(rect.h).toBeLessThanOrEqual(0.95 + 1e-9);
  });

  it("16:9 与 9:16 的像素宽高比精确命中预设（同一 3:2 图像）", () => {
    const imageAspect = 3 / 2;
    const wide = fitCropRect(16 / 9, imageAspect);
    const tall = fitCropRect(9 / 16, imageAspect);
    // 归一化宽高比 × imageAspect = 目标像素比例
    expect((wide.w / wide.h) * imageAspect).toBeCloseTo(16 / 9, 6);
    expect((tall.w / tall.h) * imageAspect).toBeCloseTo(9 / 16, 6);
    // 宽幅预设会触顶 0.95 收窄；窄幅预设高度占 0.9
    expect(wide.w).toBeCloseTo(0.95, 6);
    expect(tall.h).toBeCloseTo(0.9, 6);
    expect(tall.w).toBeCloseTo((0.9 * (9 / 16)) / imageAspect, 6);
  });

  it("非法 aspect 回退 1", () => {
    const rect = fitCropRect(1, 0);
    expect((rect.w / rect.h) * 1).toBeCloseTo(1, 6);
  });
});

describe("isFullCrop", () => {
  it("null / 全图矩形 → true；小矩形 → false", () => {
    expect(isFullCrop(null)).toBe(true);
    expect(isFullCrop({ x: 0, y: 0, w: 1, h: 1 })).toBe(true);
    expect(isFullCrop({ x: 0, y: 0, w: 0.98, h: 1 })).toBe(false);
    expect(isFullCrop({ x: 0.002, y: 0, w: 0.999, h: 1 })).toBe(false);
  });
});
