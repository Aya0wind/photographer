import { describe, expect, it } from "vitest";

import {
  SIMILARITY_BADGE_CLASS,
  SIMILARITY_HIGH_THRESHOLD,
  SIMILARITY_MID_THRESHOLD,
  similarityTier,
} from "./scoreBadge";

/** 角标三态阈值（常量导出供测试对齐）：<60% 灰 / 60–75% muted / >75% accent */
describe("相似度角标分档", () => {
  it("阈值边界：<0.60 low；0.60–0.75（含端点）mid；>0.75 high", () => {
    expect(similarityTier(0)).toBe("low");
    expect(similarityTier(SIMILARITY_MID_THRESHOLD - 0.01)).toBe("low");
    expect(similarityTier(SIMILARITY_MID_THRESHOLD)).toBe("mid");
    expect(similarityTier(0.7)).toBe("mid");
    expect(similarityTier(SIMILARITY_HIGH_THRESHOLD)).toBe("mid");
    expect(similarityTier(SIMILARITY_HIGH_THRESHOLD + 0.01)).toBe("high");
    expect(similarityTier(1)).toBe("high");
  });

  it("三档样式互不相同；high 档 accent 强调、low 档灰", () => {
    const classes = (["high", "mid", "low"] as const).map((tier) => SIMILARITY_BADGE_CLASS[tier]);
    expect(new Set(classes).size).toBe(3);
    expect(SIMILARITY_BADGE_CLASS.high).toContain("accent");
    expect(SIMILARITY_BADGE_CLASS.low).toContain("white/45");
  });
});
