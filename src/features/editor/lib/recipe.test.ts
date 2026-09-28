import { describe, expect, it } from "vitest";

import type { EditRecipe } from "@/ipc/api";
import {
  defaultRecipe,
  initRecipeHistory,
  newLayerId,
  recipeEquals,
  recipeForPersist,
  recipeReducer,
  rotatedSize,
  type RecipeHistory,
} from "./recipe";

const CTX = { width: 4000, height: 3000 };

function historyOf(recipe: EditRecipe): RecipeHistory {
  return { past: [], present: recipe, future: [] };
}

function textLayer(overrides: Partial<EditRecipe["textLayers"][number]> = {}) {
  return {
    id: "t1",
    x: 0.2,
    y: 0.3,
    text: "你好",
    sizeRel: 0.05,
    color: "#FFFFFF",
    ...overrides,
  };
}

function stroke(id = "s1") {
  return {
    id,
    color: "#FF5252",
    widthRel: 0.01,
    points: [
      { x: 0.1, y: 0.1 },
      { x: 0.5, y: 0.4 },
      { x: 0.8, y: 0.9 },
    ],
  };
}

describe("recipeReducer 撤销/重做", () => {
  it("操作进栈：undo 恢复、redo 重放", () => {
    let h = initRecipeHistory(null);
    h = recipeReducer(h, { type: "rotate", delta: 1 }, CTX);
    h = recipeReducer(h, { type: "strokeAdd", stroke: stroke() }, CTX);
    expect(h.present.brushStrokes).toHaveLength(1);
    expect(h.present.rotateQuarter).toBe(1);
    expect(h.past).toHaveLength(2);

    h = recipeReducer(h, { type: "undo" }, CTX);
    expect(h.present.brushStrokes).toHaveLength(0);
    expect(h.present.rotateQuarter).toBe(1);
    expect(h.future).toHaveLength(1);

    h = recipeReducer(h, { type: "undo" }, CTX);
    expect(h.present.rotateQuarter).toBe(0);
    expect(h.past).toHaveLength(0);

    h = recipeReducer(h, { type: "redo" }, CTX);
    expect(h.present.rotateQuarter).toBe(1);
    h = recipeReducer(h, { type: "redo" }, CTX);
    expect(h.present.brushStrokes).toHaveLength(1);
    // 新操作清空 redo 分支
    h = recipeReducer(h, { type: "undo" }, CTX);
    h = recipeReducer(h, { type: "rotate", delta: -1 }, CTX);
    expect(h.future).toHaveLength(0);
  });

  it("undo/redo 保留当前 output（输出设置不随几何撤销）", () => {
    let h = initRecipeHistory(null);
    h = recipeReducer(h, { type: "rotate", delta: 1 }, CTX);
    h = recipeReducer(h, { type: "setOutput", patch: { quality: 75, longEdge: 2560 } }, CTX);
    expect(h.past).toHaveLength(1); // setOutput 不进栈
    h = recipeReducer(h, { type: "undo" }, CTX);
    expect(h.present.rotateQuarter).toBe(0);
    expect(h.present.output.quality).toBe(75);
    expect(h.present.output.longEdge).toBe(2560);
  });

  it("commitFrom：连续手势把手势前快照压栈（文字拖动场景）", () => {
    const base = defaultRecipe();
    const snapshot = { ...base, textLayers: [textLayer({ x: 0.2 })] };
    let h = historyOf(snapshot);
    // 手势中的位置更新不进栈
    h = recipeReducer(h, { type: "textUpdate", id: "t1", patch: { x: 0.7 } }, CTX);
    expect(h.past).toHaveLength(0);
    expect(h.present.textLayers[0].x).toBeCloseTo(0.7);
    // 手势结束：压入手势前快照
    h = recipeReducer(h, { type: "commitFrom", snapshot }, CTX);
    expect(h.past).toHaveLength(1);
    h = recipeReducer(h, { type: "undo" }, CTX);
    expect(h.present.textLayers[0].x).toBeCloseTo(0.2);
  });

  it("reset 清空历史与内容", () => {
    let h = initRecipeHistory({ ...defaultRecipe(), rotateQuarter: 2 });
    h = recipeReducer(h, { type: "reset" }, CTX);
    expect(h.present).toEqual(defaultRecipe());
    expect(h.past).toHaveLength(0);
    expect(h.future).toHaveLength(0);
  });
});

describe("rotate 变换", () => {
  it("4×90° 旋转恒等（含图层与裁剪）", () => {
    const recipe: EditRecipe = {
      ...defaultRecipe(),
      crop: { x: 0.1, y: 0.15, w: 0.7, h: 0.6 },
      textLayers: [textLayer()],
      brushStrokes: [stroke()],
    };
    let h = historyOf(recipe);
    for (let i = 0; i < 4; i += 1) {
      h = recipeReducer(h, { type: "rotate", delta: 1 }, CTX);
    }
    expect(h.present.rotateQuarter).toBe(0);
    const crop = h.present.crop!;
    expect(crop.x).toBeCloseTo(recipe.crop!.x, 6);
    expect(crop.y).toBeCloseTo(recipe.crop!.y, 6);
    expect(crop.w).toBeCloseTo(recipe.crop!.w, 6);
    expect(crop.h).toBeCloseTo(recipe.crop!.h, 6);
    expect(h.present.textLayers[0].x).toBeCloseTo(recipe.textLayers[0].x, 6);
    expect(h.present.textLayers[0].y).toBeCloseTo(recipe.textLayers[0].y, 6);
    expect(h.present.textLayers[0].sizeRel).toBeCloseTo(recipe.textLayers[0].sizeRel, 6);
    recipe.brushStrokes[0].points.forEach((p, i) => {
      expect(h.present.brushStrokes[0].points[i].x).toBeCloseTo(p.x, 6);
      expect(h.present.brushStrokes[0].points[i].y).toBeCloseTo(p.y, 6);
    });
  });

  it("+90° 再 -90° 回到原状", () => {
    const recipe: EditRecipe = {
      ...defaultRecipe(),
      crop: { x: 0.1, y: 0.15, w: 0.7, h: 0.6 },
      textLayers: [textLayer()],
      brushStrokes: [stroke()],
    };
    let h = historyOf(recipe);
    h = recipeReducer(h, { type: "rotate", delta: 1 }, CTX);
    const rotated = h.present;
    h = recipeReducer(h, { type: "rotate", delta: -1 }, CTX);
    expect(h.present.rotateQuarter).toBe(0);
    const back = h.present.crop!;
    expect(back.x).toBeCloseTo(recipe.crop!.x, 8);
    expect(back.y).toBeCloseTo(recipe.crop!.y, 8);
    expect(back.w).toBeCloseTo(recipe.crop!.w, 8);
    expect(back.h).toBeCloseTo(recipe.crop!.h, 8);
    expect(h.present.textLayers[0].x).toBeCloseTo(recipe.textLayers[0].x, 8);
    expect(h.present.textLayers[0].y).toBeCloseTo(recipe.textLayers[0].y, 8);
    expect(h.present.textLayers[0].sizeRel).toBeCloseTo(recipe.textLayers[0].sizeRel, 8);
    recipe.brushStrokes[0].points.forEach((p, i) => {
      expect(h.present.brushStrokes[0].points[i].x).toBeCloseTo(p.x, 8);
      expect(h.present.brushStrokes[0].points[i].y).toBeCloseTo(p.y, 8);
    });
    expect(rotated.crop).not.toBeNull();
  });

  it("+90°：crop 矩形轴交换（x:1-y-h, y:x, w:h, h:w）", () => {
    const recipe: EditRecipe = { ...defaultRecipe(), crop: { x: 0.1, y: 0.2, w: 0.5, h: 0.3 } };
    const h = recipeReducer(historyOf(recipe), { type: "rotate", delta: 1 }, CTX);
    const crop = h.present.crop!;
    expect(crop.x).toBeCloseTo(1 - 0.2 - 0.3, 8);
    expect(crop.y).toBeCloseTo(0.1, 8);
    expect(crop.w).toBeCloseTo(0.3, 8);
    expect(crop.h).toBeCloseTo(0.5, 8);
  });

  it("+90°：图层点映射 (a,b)→(1-b,a)（无裁剪时）", () => {
    const recipe: EditRecipe = { ...defaultRecipe(), textLayers: [textLayer({ x: 0.2, y: 0.3 })] };
    const h = recipeReducer(historyOf(recipe), { type: "rotate", delta: 1 }, CTX);
    expect(h.present.textLayers[0].x).toBeCloseTo(0.7, 8);
    expect(h.present.textLayers[0].y).toBeCloseTo(0.2, 8);
  });

  it("+90°：字号像素不变（sizeRel 按画布宽比缩放）", () => {
    // 4000×3000 无裁剪：旋转后画布宽 3000，sizeRel 应 ×(4000/3000)
    const recipe: EditRecipe = { ...defaultRecipe(), textLayers: [textLayer({ sizeRel: 0.03 })] };
    const h = recipeReducer(historyOf(recipe), { type: "rotate", delta: 1 }, CTX);
    expect(h.present.textLayers[0].sizeRel).toBeCloseTo(0.03 * (4000 / 3000), 8);
  });

  it("基准图未就绪（ctx 0×0）时旋转不崩：仅步进 quarter", () => {
    const h = recipeReducer(initRecipeHistory(null), { type: "rotate", delta: 1 }, { width: 0, height: 0 });
    expect(h.present.rotateQuarter).toBe(1);
    expect(h.present.crop).toBeNull();
  });

  it("rotatedSize：奇数 quarter 交换宽高", () => {
    expect(rotatedSize({ width: 4000, height: 3000 }, 0)).toEqual({ w: 4000, h: 3000 });
    expect(rotatedSize({ width: 4000, height: 3000 }, 1)).toEqual({ w: 3000, h: 4000 });
    expect(rotatedSize({ width: 4000, height: 3000 }, 2)).toEqual({ w: 4000, h: 3000 });
    expect(rotatedSize({ width: 4000, height: 3000 }, 3)).toEqual({ w: 3000, h: 4000 });
  });
});

describe("cropApply 变换", () => {
  it("图层点随图像内容重基：旧画布→旋转图→新画布", () => {
    // 旧画布=全图；点 (0.5, 0.5)；新裁剪 {0.25,0.25,0.5,0.5} → 新画布内 (0.5, 0.5)
    const recipe: EditRecipe = { ...defaultRecipe(), textLayers: [textLayer({ x: 0.5, y: 0.5 })] };
    const h = recipeReducer(
      historyOf(recipe),
      { type: "cropApply", crop: { x: 0.25, y: 0.25, w: 0.5, h: 0.5 } },
      CTX,
    );
    expect(h.present.textLayers[0].x).toBeCloseTo(0.5, 8);
    expect(h.present.textLayers[0].y).toBeCloseTo(0.5, 8);
    // 旧画布 (0.2, 0.2)（=旋转图 (0.2,0.2)）→ 新画布 ((0.2-0.25)/0.5, ...) 为负 → 夹取 0
    let h2 = historyOf({ ...defaultRecipe(), textLayers: [textLayer({ x: 0.2, y: 0.2 })] });
    h2 = recipeReducer(h2, { type: "cropApply", crop: { x: 0.25, y: 0.25, w: 0.5, h: 0.5 } }, CTX);
    expect(h2.present.textLayers[0].x).toBe(0);
    expect(h2.present.textLayers[0].y).toBe(0);
  });

  it("sizeRel 按新画布宽度缩放（裁小一半 → ×2）", () => {
    const recipe: EditRecipe = { ...defaultRecipe(), textLayers: [textLayer({ sizeRel: 0.04 })] };
    const h = recipeReducer(
      historyOf(recipe),
      { type: "cropApply", crop: { x: 0, y: 0, w: 0.5, h: 1 } },
      CTX,
    );
    expect(h.present.textLayers[0].sizeRel).toBeCloseTo(0.08, 8);
  });

  it("裁剪矩形夹取：越界/超小输入被修正", () => {
    const h = recipeReducer(
      initRecipeHistory(null),
      { type: "cropApply", crop: { x: -0.5, y: 0.9, w: 2, h: 0.001 } },
      CTX,
    );
    const crop = h.present.crop;
    expect(crop).not.toBeNull();
    expect(crop!.x).toBeGreaterThanOrEqual(0);
    expect(crop!.x + crop!.w).toBeLessThanOrEqual(1 + 1e-9);
    expect(crop!.y + crop!.h).toBeLessThanOrEqual(1 + 1e-9);
    expect(crop!.w).toBeGreaterThanOrEqual(0.05 - 1e-9);
    expect(crop!.h).toBeGreaterThanOrEqual(0.05 - 1e-9);
  });
});

describe("recipeForPersist / equals / id", () => {
  it("剔除空文字层与零点笔迹", () => {
    const recipe: EditRecipe = {
      ...defaultRecipe(),
      textLayers: [textLayer({ text: "有内容" }), textLayer({ text: "  " })],
      brushStrokes: [stroke(), { id: "s2", color: "#FFF", widthRel: 0.01, points: [] }],
    };
    const persisted = recipeForPersist(recipe);
    expect(persisted.textLayers).toHaveLength(1);
    expect(persisted.brushStrokes).toHaveLength(1);
  });

  it("recipeEquals：字段级差异检出", () => {
    expect(recipeEquals(defaultRecipe(), defaultRecipe())).toBe(true);
    expect(recipeEquals(defaultRecipe(), { ...defaultRecipe(), rotateQuarter: 1 })).toBe(false);
  });

  it("newLayerId 唯一", () => {
    const ids = new Set(Array.from({ length: 50 }, () => newLayerId()));
    expect(ids.size).toBe(50);
  });
});
