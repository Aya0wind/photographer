import { describe, expect, it } from "vitest";

import type { AssetDto, DuplicateGroupDto, PhotoLibrary } from "@/ipc/api";
import { assetFixture } from "@/test/fixtures";
import {
  collapseCrossLibraryDuplicates,
  compareDuplicateRank,
  isCrossLibraryGroup,
} from "./crossLibraryDuplicates";

/**
 * 跨库重复折叠（2026-10-09 定案 §四，纯显示过滤）核心机制：
 * - 只折叠跨库组（≥2 个互不相同已知 libraryId）；同库组原样透传
 * - 组代表优先级：在线 > 有高清 RAW > 评分高 > 导入早（id 小）
 * - 折叠在已加载成员中选代表（未加载成员不参与，防画廊中段出洞）
 * - 徽标/重复项列表 = 组全量（不受分页影响）
 */

function dupGroup(...assets: AssetDto[]): DuplicateGroupDto {
  return { kind: "exact", assets };
}

describe("isCrossLibraryGroup", () => {
  it("≥2 个互不相同的已知 libraryId = 跨库", () => {
    expect(
      isCrossLibraryGroup([
        assetFixture(1, { libraryId: "lib-a" }),
        assetFixture(2, { libraryId: "lib-b" }),
      ]),
    ).toBe(true);
  });

  it("全部同库（或归属未知）= 不折叠", () => {
    expect(
      isCrossLibraryGroup([
        assetFixture(1, { libraryId: "lib-a" }),
        assetFixture(2, { libraryId: "lib-a" }),
      ]),
    ).toBe(false);
    expect(
      isCrossLibraryGroup([assetFixture(1), assetFixture(2)]),
    ).toBe(false);
  });
});

describe("compareDuplicateRank（在线 > 有高清 RAW > 评分高 > 导入早）", () => {
  const lib = (id: string, status: PhotoLibrary["status"]): PhotoLibrary => ({
    id,
    name: id,
    rootPath: `D:\\${id}`,
    createdAt: "2026-10-09T00:00:00Z",
    status,
    assetCount: 0,
    sizeBytes: 0,
  });
  const libraryById = new Map([
    ["lib-a", lib("lib-a", "online")],
    ["lib-b", lib("lib-b", "offline")],
  ]);

  it("在线压倒一切：离线库的高分 RAW 输给在线的普通 JPG", () => {
    const offline = assetFixture(1, {
      libraryId: "lib-b",
      kind: "raw",
      rating: 5,
    });
    const online = assetFixture(2, { libraryId: "lib-a", rating: 0 });
    expect(compareDuplicateRank(online, offline, libraryById)).toBeLessThan(0);
  });

  it("missing 标记 = 不在线（整库离线的单文件形态）", () => {
    const missing = assetFixture(1, { libraryId: "lib-a", missing: true, rating: 5 });
    const online = assetFixture(2, { libraryId: "lib-a" });
    expect(compareDuplicateRank(online, missing, libraryById)).toBeLessThan(0);
  });

  it("库状态未知（null 表）按在线：不因过渡期数据误杀", () => {
    const a = assetFixture(1, { libraryId: "lib-x" });
    const b = assetFixture(2, { libraryId: "lib-y" });
    // 都按在线 → 落到导入早（id 小）决胜
    expect(compareDuplicateRank(a, b, null)).toBeLessThan(0);
  });

  it("同为在线：有高清 RAW（本体 RAW 或有配对）优先", () => {
    const raw = assetFixture(2, { libraryId: "lib-a", kind: "raw" });
    const pairedJpg = assetFixture(3, { libraryId: "lib-a", pairId: 2 });
    const bareJpg = assetFixture(1, { libraryId: "lib-a" });
    expect(compareDuplicateRank(raw, bareJpg, libraryById)).toBeLessThan(0);
    expect(compareDuplicateRank(pairedJpg, bareJpg, libraryById)).toBeLessThan(0);
  });

  it("同为在线同为 RAW：评分高优先；再同 → 导入早（id 小）", () => {
    const low = assetFixture(1, { libraryId: "lib-a", rating: 2 });
    const high = assetFixture(5, { libraryId: "lib-a", rating: 4 });
    expect(compareDuplicateRank(high, low, libraryById)).toBeLessThan(0);
    const early = assetFixture(3, { libraryId: "lib-a", rating: 3 });
    const late = assetFixture(9, { libraryId: "lib-a", rating: 3 });
    expect(compareDuplicateRank(early, late, libraryById)).toBeLessThan(0);
  });
});

describe("collapseCrossLibraryDuplicates（纯显示过滤）", () => {
  it("跨库组只留一张代表，隐藏副本不出现在可见列表；徽标=组全量数", () => {
    // 同一内容：lib-b 在线 JPG（id 2）与 lib-a 普通 JPG（id 1）；无库状态表 → 按在线，
    // 落到导入早 → id 1 代表
    const assets = [assetFixture(1, { libraryId: "lib-a" }), assetFixture(2, { libraryId: "lib-b" })];
    const groups = [dupGroup(...assets.slice().reverse())];
    const result = collapseCrossLibraryDuplicates(assets, groups, null);

    expect(result.visible.map((a) => a.id)).toEqual([1]);
    expect(result.badges.get(1)).toBe(2);
    expect(result.membersByVisibleId.get(1)?.map((a) => a.id).sort()).toEqual([1, 2]);
  });

  it("优先级落点：离线库副本让位，在线库副本成为代表（顺序保持首位）", () => {
    const libraryById = new Map<string, PhotoLibrary>([
      [
        "lib-a",
        {
          id: "lib-a",
          name: "A",
          rootPath: "D:\\a",
          createdAt: "",
          status: "offline",
          assetCount: 0,
          sizeBytes: 0,
        },
      ],
    ]);
    const assets = [
      assetFixture(1, { libraryId: "lib-a" }), // 离线（库 offline）
      assetFixture(2, { libraryId: "lib-b" }), // 库未知按在线 → 代表
    ];
    const groups = [dupGroup(...assets)];
    const result = collapseCrossLibraryDuplicates(assets, groups, libraryById);

    expect(result.visible.map((a) => a.id)).toEqual([2]);
    expect(result.badges.get(2)).toBe(2);
  });

  it("代表在已加载成员中选：未加载的高优先级成员不抢占（防画廊中段出洞）", () => {
    const loaded = [assetFixture(7, { libraryId: "lib-a", rating: 1 })];
    const unloaded = assetFixture(2, { libraryId: "lib-b", rating: 5 });
    const groups = [dupGroup(loaded[0], unloaded)];
    const result = collapseCrossLibraryDuplicates(loaded, groups, null);

    // 未加载成员不参与：可见 = 已加载的 id 7；徽标仍按组全量 2 张
    expect(result.visible.map((a) => a.id)).toEqual([7]);
    expect(result.badges.get(7)).toBe(2);
    expect(result.membersByVisibleId.get(7)?.map((a) => a.id).sort()).toEqual([2, 7]);
  });

  it("同库重复组不折叠（rename 策略遗留的合法态不受开关影响）", () => {
    const assets = [
      assetFixture(1, { libraryId: "lib-a" }),
      assetFixture(2, { libraryId: "lib-a" }),
    ];
    const groups = [dupGroup(...assets)];
    const result = collapseCrossLibraryDuplicates(assets, groups, null);

    expect(result.visible.map((a) => a.id)).toEqual([1, 2]);
    expect(result.badges.size).toBe(0);
  });

  it("groups=null（未加载完）原样透传；无关资产不受影响", () => {
    const assets = [assetFixture(1), assetFixture(2, { libraryId: "lib-a" })];
    expect(collapseCrossLibraryDuplicates(assets, null, null).visible).toHaveLength(2);

    const other = assetFixture(9);
    const groups = [
      dupGroup(
        assetFixture(1, { libraryId: "lib-a" }),
        assetFixture(2, { libraryId: "lib-b" }),
      ),
    ];
    const result = collapseCrossLibraryDuplicates([assets[0], other, assets[1]], groups, null);
    expect(result.visible.map((a) => a.id)).toEqual([1, 9]);
  });

  it("多组互不干扰：每组各自选代表", () => {
    const a1 = assetFixture(1, { libraryId: "lib-a" });
    const a2 = assetFixture(2, { libraryId: "lib-b" });
    const b1 = assetFixture(3, { libraryId: "lib-a", rating: 5 });
    const b2 = assetFixture(4, { libraryId: "lib-b", rating: 1 });
    const groups = [dupGroup(a1, a2), dupGroup(b1, b2)];
    const result = collapseCrossLibraryDuplicates([a1, b1, a2, b2], groups, null);

    expect(result.visible.map((a) => a.id).sort()).toEqual([1, 3]);
    expect(result.badges.get(1)).toBe(2);
    expect(result.badges.get(3)).toBe(2);
  });
});
