import { describe, expect, it } from "vitest";

import { levelForZoom, levelLabelKey, zoomForLevel, type MapLevel } from "./hierarchy";

describe("levelForZoom 边界", () => {
  it("全球 → 国家层", () => {
    expect(levelForZoom(0)).toBe(0);
    expect(levelForZoom(3.49)).toBe(0);
  });
  it("国家 → 省层", () => {
    expect(levelForZoom(3.5)).toBe(1);
    expect(levelForZoom(6.49)).toBe(1);
  });
  it("省 → 市层", () => {
    expect(levelForZoom(6.5)).toBe(2);
    expect(levelForZoom(9.49)).toBe(2);
  });
  it("市 → 县层（最深，2026-09-29 定案）", () => {
    expect(levelForZoom(9.5)).toBe(3);
    expect(levelForZoom(18)).toBe(3);
  });
});

describe("zoomForLevel 往返", () => {
  it("每层目标 zoom 落在该层区间内（flyTo 后 zoomend 不立即反向切层）", () => {
    for (const level of [0, 1, 2, 3] as MapLevel[]) {
      expect(levelForZoom(zoomForLevel(level))).toBe(level);
    }
  });
});

describe("levelLabelKey", () => {
  it("层级文案键", () => {
    expect(levelLabelKey(0)).toBe("map.level.country");
    expect(levelLabelKey(3)).toBe("map.level.county");
  });
});
