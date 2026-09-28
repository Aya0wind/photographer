import { describe, expect, it } from "vitest";

import {
  bakeTextScale,
  cropToRectAttrs,
  linePointsToNorm,
  nodeAttrsToTextLayer,
  rectAttrsToCrop,
  roundTripCrop,
  roundTripStroke,
  roundTripTextLayer,
  strokeWidthPx,
  strokeToLinePoints,
  textLayerToNodeAttrs,
} from "./konvaMapping";

/**
 * recipe ↔ Konva 状态往返映射的换算函数测试（协调指令：vitest 测试重点）。
 * 保证 Konva 交互层提交的每个字段与契约归一化口径逐位一致。
 */

const CANVAS = { width: 1000, height: 750 };
const FULL = { width: 1600, height: 1200 };

describe("文字层映射", () => {
  it("recipe → Konva 节点：归一化×画布显示尺寸（左上锚点、字号=sizeRel×画布宽）", () => {
    const attrs = textLayerToNodeAttrs(
      { id: "t1", x: 0.25, y: 0.4, text: "你好\n世界", sizeRel: 0.06, color: "#FFFFFF" },
      CANVAS,
    );
    expect(attrs).toEqual({ x: 250, y: 300, fontSize: 60, text: "你好\n世界", fill: "#FFFFFF" });
  });

  it("Konva 节点 → recipe：往返无损（id 保留）", () => {
    const layer = { id: "t9", x: 0.123456, y: 0.654321, text: "多行\n文本", sizeRel: 0.055, color: "#F0A83C" };
    const back = roundTripTextLayer(layer, CANVAS);
    expect(back.id).toBe("t9");
    expect(back.x).toBeCloseTo(layer.x, 9);
    expect(back.y).toBeCloseTo(layer.y, 9);
    expect(back.sizeRel).toBeCloseTo(layer.sizeRel, 9);
    expect(back.text).toBe(layer.text);
    expect(back.color).toBe(layer.color);
  });

  it("节点属性直转：clamp 越界坐标", () => {
    const layer = nodeAttrsToTextLayer(
      { x: 2000, y: -10, fontSize: 50, text: "x", fill: "#000000" },
      CANVAS,
      "t2",
    );
    expect(layer.x).toBe(1);
    expect(layer.y).toBe(0);
    expect(layer.sizeRel).toBeCloseTo(0.05, 9);
  });

  it("Transformer 缩放烘焙：scale 并入字号、复位为 1", () => {
    const baked = bakeTextScale({ x: 100, y: 200, fontSize: 60, scaleX: 1.5 });
    expect(baked).toEqual({ x: 100, y: 200, fontSize: 90, scaleX: 1 });
    // 非法 scale 回退 1
    expect(bakeTextScale({ x: 0, y: 0, fontSize: 60, scaleX: Number.NaN }).fontSize).toBe(60);
    expect(bakeTextScale({ x: 0, y: 0, fontSize: 60, scaleX: -2 }).fontSize).toBe(60);
  });

  it("烘焙后 sizeRel 换算仍一致（Konva 字号 → recipe 归一化）", () => {
    const baked = bakeTextScale({ x: 100, y: 200, fontSize: 60, scaleX: 1.5 });
    const sizeRel = baked.fontSize / CANVAS.width;
    expect(sizeRel).toBeCloseTo(0.09, 9);
  });
});

describe("笔迹映射", () => {
  it("recipe → Konva Line points：扁平 [x0,y0,x1,y1,...]", () => {
    const points = strokeToLinePoints(
      { points: [{ x: 0.1, y: 0.2 }, { x: 0.3, y: 0.4 }] },
      CANVAS,
    );
    expect(points).toEqual([100, 150, 300, 300]);
  });

  it("Konva points → recipe：往返无损", () => {
    const stroke = {
      points: [{ x: 0.05, y: 0.95 }, { x: 0.5, y: 0.5 }, { x: 0.99, y: 0.01 }],
    };
    const back = roundTripStroke(stroke, CANVAS);
    expect(back).toHaveLength(3);
    back.forEach((p, i) => {
      expect(p.x).toBeCloseTo(stroke.points[i].x, 9);
      expect(p.y).toBeCloseTo(stroke.points[i].y, 9);
    });
  });

  it("奇数长度 points（半途点）安全截断", () => {
    const norm = linePointsToNorm([100, 150, 300], CANVAS);
    expect(norm).toEqual([{ x: 0.1, y: 0.2 }]);
  });

  it("笔宽：widthRel ↔ px", () => {
    expect(strokeWidthPx(0.008, CANVAS)).toBeCloseTo(8, 9);
    // 最小 1px 防 0 宽
    expect(strokeWidthPx(0.0001, CANVAS)).toBe(1);
  });
});

describe("裁剪矩形映射", () => {
  it("recipe crop → Konva Rect 属性（旋转后整图显示盒）", () => {
    expect(cropToRectAttrs({ x: 0.25, y: 0.5, w: 0.5, h: 0.25 }, FULL)).toEqual({
      x: 400,
      y: 600,
      width: 800,
      height: 300,
    });
  });

  it("Rect 属性 → recipe crop：往返无损", () => {
    const crop = { x: 0.123, y: 0.456, w: 0.6, h: 0.3 };
    const back = roundTripCrop(crop, FULL);
    expect(back.x).toBeCloseTo(crop.x, 9);
    expect(back.y).toBeCloseTo(crop.y, 9);
    expect(back.w).toBeCloseTo(crop.w, 9);
    expect(back.h).toBeCloseTo(crop.h, 9);
  });

  it("rectAttrsToCrop：负宽高取绝对值、坐标 clamp", () => {
    const crop = rectAttrsToCrop({ x: -100, y: 2000, width: -800, height: 300 }, FULL);
    expect(crop.x).toBe(0);
    expect(crop.y).toBe(1);
    expect(crop.w).toBeCloseTo(0.5, 9);
    expect(crop.h).toBeCloseTo(0.25, 9);
  });
});

describe("端到端换算一致性（契约口径）", () => {
  it("同一画布尺寸下，放置点像素 → 归一化 → 节点属性链路闭合", () => {
    // Konva 画布点按 (350, 225)（3:2 画布中心偏移）→ 归一化 → 节点属性
    const norm = { x: 350 / CANVAS.width, y: 225 / CANVAS.height };
    const layer = { id: "place", ...norm, text: "", sizeRel: 0.06, color: "#FFFFFF" };
    const attrs = textLayerToNodeAttrs(layer, CANVAS);
    expect(attrs.x).toBeCloseTo(350, 9);
    expect(attrs.y).toBeCloseTo(225, 9);
  });
});
