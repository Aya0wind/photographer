import { describe, expect, it } from "vitest";

import { mergeRawJpgCards } from "./mergeRawJpg";
import type { AssetDto } from "@/ipc/api";

function a(id: number, kind: AssetDto["kind"], pairId?: number): AssetDto {
  return {
    id,
    path: `Y:\\p\\f${id}`,
    name: `f${id}`,
    kind,
    capturedAt: "2026-09-18T10:00:00",
    camera: null,
    sizeBytes: 1,
    pairId,
  };
}

describe("mergeRawJpgCards（RAW+JPG 合并展示）", () => {
  it("开关关闭：原样透出、无角标", () => {
    const assets = [a(1, "photo", 9), a(2, "raw", 9)];
    const { cards, badges } = mergeRawJpgCards(assets, false);
    expect(cards).toEqual(assets);
    expect(badges.size).toBe(0);
  });

  it("成对合并：代表=JPG、角标 RAW+JPG、保持 DESC 首现顺序", () => {
    const { cards, badges } = mergeRawJpgCards(
      [a(1, "photo", 9), a(2, "raw", 9), a(3, "photo")],
      true,
    );
    expect(cards.map((c) => c.id)).toEqual([1, 3]);
    expect(badges.get(1)).toBe("RAW+JPG");
    expect(badges.has(3)).toBe(false);
  });

  it("对内顺序无关：RAW 在前也选 JPG 为代表卡", () => {
    const { cards, badges } = mergeRawJpgCards([a(5, "raw", 9), a(4, "photo", 9)], true);
    expect(cards.map((c) => c.id)).toEqual([4]);
    expect(badges.get(4)).toBe("RAW+JPG");
  });

  it("只有 RAW 的 pair：合并为一张但不打 RAW+JPG 角标（水印由 tile 内部负责）", () => {
    const { cards, badges } = mergeRawJpgCards([a(1, "raw", 7), a(2, "raw", 7)], true);
    expect(cards.map((c) => c.id)).toEqual([1]);
    expect(badges.size).toBe(0);
  });

  it("无 pairId：不受开关影响，原样透出", () => {
    const assets = [a(1, "photo"), a(2, "raw")];
    expect(mergeRawJpgCards(assets, true).cards).toEqual(assets);
  });
});
