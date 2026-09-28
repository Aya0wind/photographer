import { beforeEach, describe, expect, it, vi } from "vitest";

import { assetsPage, type AssetDto, type AssetFilters, type CullDecisionValue } from "@/ipc/api";

import {
  buildFinishApply,
  cullKeyAction,
  collectAssetIdsByFilters,
  deriveProgress,
  indexAfterDecision,
  scopeDescKey,
  withDecision,
} from "./cullingCore";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    assetsPage: vi.fn(),
  };
});

const assetsPageMock = vi.mocked(assetsPage);

function asset(id: number): AssetDto {
  return {
    id,
    path: `Y:/photo/${id}.jpg`,
    name: `${id}.jpg`,
    kind: "photo",
    capturedAt: "2026-01-01T00:00:00Z",
    camera: null,
    sizeBytes: 1,
  };
}

beforeEach(() => {
  assetsPageMock.mockReset();
});

describe("决定状态机（withDecision）", () => {
  it("accept / reject / undo（回未定）转移", () => {
    type Map0 = Map<number, CullDecisionValue | null>;
    let decisions: Map0 = new Map();
    decisions = withDecision(decisions, 1, "accepted") as typeof decisions;
    expect(decisions.get(1)).toBe("accepted");

    decisions = withDecision(decisions, 1, "rejected") as typeof decisions;
    expect(decisions.get(1)).toBe("rejected");

    // undo：回未定 = 删行（与 cull_decision 表「无行」语义一致）
    decisions = withDecision(decisions, 1, null) as typeof decisions;
    expect(decisions.has(1)).toBe(false);
  });

  it("同值写入返回原引用（避免无谓重渲）", () => {
    const decisions = withDecision(new Map(), 1, "accepted");
    expect(withDecision(decisions, 1, "accepted")).toBe(decisions);
    expect(withDecision(decisions, 2, null)).toBe(decisions);
  });
});

describe("进度计数派生（deriveProgress）", () => {
  it("已选/已剔除/未定/总数从决定表派生", () => {
    const decisions = new Map<number, CullDecisionValue>([
      [1, "accepted"],
      [2, "accepted"],
      [3, "rejected"],
    ]);
    expect(deriveProgress(10, decisions)).toEqual({
      accepted: 2,
      rejected: 1,
      undecided: 7,
      total: 10,
    });
  });

  it("空决定表 = 全未定；脏数据（决定多于总数）不出现负未定", () => {
    expect(deriveProgress(0, new Map())).toEqual({
      accepted: 0,
      rejected: 0,
      undecided: 0,
      total: 0,
    });
    const dirty = new Map<number, CullDecisionValue>([
      [1, "accepted"],
      [2, "rejected"],
    ]);
    expect(deriveProgress(1, dirty).undecided).toBe(0);
    expect(deriveProgress(1, dirty).total).toBe(2);
  });
});

describe("键盘映射（cullKeyAction）", () => {
  it("←/→ 翻片 · 空格/↑ 选入 · X/↓ 剔除 · U 回未定", () => {
    expect(cullKeyAction({ key: "ArrowLeft" })).toBe("prev");
    expect(cullKeyAction({ key: "ArrowRight" })).toBe("next");
    expect(cullKeyAction({ key: " " })).toBe("accept");
    expect(cullKeyAction({ key: "Spacebar" })).toBe("accept");
    expect(cullKeyAction({ key: "x" })).toBe("reject");
    expect(cullKeyAction({ key: "X" })).toBe("reject");
    expect(cullKeyAction({ key: "ArrowUp" })).toBe("accept");
    expect(cullKeyAction({ key: "ArrowDown" })).toBe("reject");
    expect(cullKeyAction({ key: "u" })).toBe("undecided");
    expect(cullKeyAction({ key: "U" })).toBe("undecided");
  });

  it("其余键与修饰键组合不接管（让位应用快捷键）", () => {
    expect(cullKeyAction({ key: "a" })).toBeNull();
    expect(cullKeyAction({ key: "?" })).toBeNull();
    expect(cullKeyAction({ key: "x", ctrlKey: true })).toBeNull();
    expect(cullKeyAction({ key: "ArrowLeft", metaKey: true })).toBeNull();
    expect(cullKeyAction({ key: " ", altKey: true })).toBeNull();
  });
});

describe("决定后翻页（indexAfterDecision）", () => {
  it("向后推进；末张停住；空会话归零", () => {
    expect(indexAfterDecision(0, 10)).toBe(1);
    expect(indexAfterDecision(9, 10)).toBe(9);
    expect(indexAfterDecision(0, 0)).toBe(0);
  });
});

describe("收尾映射组装（buildFinishApply）", () => {
  it("默认全关：星级 null", () => {
    expect(
      buildFinishApply({ acceptedFlag: false, acceptedRating: null, rejectRejected: false }),
    ).toEqual({ acceptedFlag: false, acceptedRating: null, rejectRejected: false });
  });

  it("星级规整：1-5 整数保留，越界/非整数回 null", () => {
    expect(
      buildFinishApply({ acceptedFlag: true, acceptedRating: 5, rejectRejected: true }),
    ).toEqual({ acceptedFlag: true, acceptedRating: 5, rejectRejected: true });
    expect(buildFinishApply({ acceptedFlag: false, acceptedRating: 0, rejectRejected: false }).acceptedRating).toBeNull();
    expect(buildFinishApply({ acceptedFlag: false, acceptedRating: 6, rejectRejected: false }).acceptedRating).toBeNull();
    expect(buildFinishApply({ acceptedFlag: false, acceptedRating: 3.5, rejectRejected: false }).acceptedRating).toBeNull();
  });
});

describe("筛选结果 id 快照（collectAssetIdsByFilters）", () => {
  it("keyset 分页循环直取全量（跨多页 + 短页止步）", async () => {
    assetsPageMock.mockImplementation(async (afterId, _limit, filters) => {
      expect(_limit).toBe(500);
      // 空筛选不携带 filters 字段（与 assetsPage 契约一致）
      if (afterId === 0) {
        expect(filters).toBeUndefined();
        return Array.from({ length: 500 }, (_, i) => asset(i + 1));
      }
      return Array.from({ length: 3 }, (_, i) => asset(500 + i + 1));
    });
    const ids = await collectAssetIdsByFilters();
    expect(ids).toHaveLength(503);
    expect(ids[0]).toBe(1);
    expect(ids[502]).toBe(503);
  });

  it("筛选条件原样透传；失败页即止", async () => {
    const filters: AssetFilters = { kinds: ["raw"] };
    assetsPageMock.mockImplementation(async (afterId, _limit, passed) => {
      expect(passed).toEqual(filters);
      return afterId === 0 ? [asset(9)] : [];
    });
    await expect(collectAssetIdsByFilters(filters)).resolves.toEqual([9]);
  });
});

describe("来源描述键（scopeDescKey）", () => {
  it("相册根/子组/查询三分", () => {
    expect(scopeDescKey({ kind: "album", albumId: 1, subgroup: null })).toBe("album");
    expect(scopeDescKey({ kind: "album", albumId: 1, subgroup: "成片" })).toBe("albumSubgroup");
    expect(scopeDescKey({ kind: "query", assetIds: [1] })).toBe("query");
  });
});
