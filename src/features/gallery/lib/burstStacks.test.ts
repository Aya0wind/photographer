import { assetFixture } from "@/test/fixtures";
import { describe, expect, it } from "vitest";

import type { AssetDto } from "@/ipc/api";
import { collapseBursts } from "./burstStacks";

function asset(id: number, burst?: { burstId: number; burstCount?: number }): AssetDto {
  return assetFixture(id, {
    capturedAt: "2026-09-18T10:00:00",
    camera: "Canon EOS R5",
    sizeBytes: 1024,
    ...(burst ?? {}),
  });
}

describe("collapseBursts（连拍堆叠折叠）", () => {
  it("连续同 burstId 的 run 折叠为封面一张，角标 N=burstCount（含封面）", () => {
    const assets = [
      asset(1, { burstId: 9, burstCount: 3 }),
      asset(2, { burstId: 9, burstCount: 3 }),
      asset(3, { burstId: 9, burstCount: 3 }),
      asset(4),
    ];

    const collapsed = collapseBursts(assets);

    expect(collapsed.assets.map((a) => a.id)).toEqual([1, 4]);
    expect(collapsed.badges.get(1)).toBe(3);
    expect(collapsed.badges.size).toBe(1);
    // 组头计数口径保持真实照片数
    expect(collapsed.totalCount).toBe(4);
  });

  it("burstCount 缺失时角标回退实际连续段长度", () => {
    const assets = [
      asset(7, { burstId: 5 }),
      asset(8, { burstId: 5 }),
    ];

    const collapsed = collapseBursts(assets);

    expect(collapsed.assets.map((a) => a.id)).toEqual([7]);
    expect(collapsed.badges.get(7)).toBe(2);
  });

  it("burstCount<2 视为无效，同样回退段长", () => {
    const assets = [asset(1, { burstId: 3, burstCount: 1 }), asset(2, { burstId: 3, burstCount: 1 })];
    expect(collapseBursts(assets).badges.get(1)).toBe(2);
  });

  it("同 burstId 不连续不成一组；被打断后各自独立折叠", () => {
    const assets = [
      asset(1, { burstId: 9, burstCount: 2 }),
      asset(2, { burstId: 9, burstCount: 2 }),
      asset(3), // 打断（同 burstId 不跨段合并）
      asset(4, { burstId: 9, burstCount: 2 }),
      asset(5, { burstId: 9, burstCount: 2 }),
    ];

    const collapsed = collapseBursts(assets);

    expect(collapsed.assets.map((a) => a.id)).toEqual([1, 3, 4]);
    expect(collapsed.badges.get(1)).toBe(2);
    expect(collapsed.badges.get(4)).toBe(2);
    expect(collapsed.badges.size).toBe(2);
  });

  it("单张即使携带 burstId 也不成组（无角标、原样保留）", () => {
    const assets = [asset(1, { burstId: 9, burstCount: 2 }), asset(2)];

    const collapsed = collapseBursts(assets);

    expect(collapsed.assets.map((a) => a.id)).toEqual([1, 2]);
    expect(collapsed.badges.size).toBe(0);
    expect(collapsed.totalCount).toBe(2);
  });

  it("burstId 为 null/0 不参与折叠；空输入原样", () => {
    expect(collapseBursts([])).toEqual({
      assets: [],
      badges: new Map(),
      totalCount: 0,
    });

    const assets = [asset(1, { burstId: 0 }), asset(2, { burstId: 0 })];
    const collapsed = collapseBursts(assets);
    expect(collapsed.assets.map((a) => a.id)).toEqual([1, 2]);
    expect(collapsed.badges.size).toBe(0);
  });

  it("全组都是连拍：展示全是封面，计数不受影响", () => {
    const assets = [
      asset(1, { burstId: 4, burstCount: 4 }),
      asset(2, { burstId: 4, burstCount: 4 }),
      asset(3, { burstId: 4, burstCount: 4 }),
      asset(4, { burstId: 4, burstCount: 4 }),
    ];

    const collapsed = collapseBursts(assets);

    expect(collapsed.assets.map((a) => a.id)).toEqual([1]);
    expect(collapsed.badges.get(1)).toBe(4);
    expect(collapsed.totalCount).toBe(4);
  });
});
