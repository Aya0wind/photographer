import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  assetsPage,
  type AssetDto,
  type AssetFilters,
  type CullDecisionValue,
} from "@/ipc/api";

import {
  buildAiRules,
  buildFinishApply,
  buildKeepOnlyDecisions,
  clampZoomPoint,
  collectAssetIdsByFilters,
  comparisonMembers,
  cullKeyAction,
  deriveProgress,
  indexAfterDecision,
  scopeDescKey,
  withDecision,
  zoomEnterPoint,
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
  it("←/→ 翻片 · 空格/↑ 选入 · X/↓ 剔除 · U 回未定 · C 对比切换（V2）", () => {
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
    expect(cullKeyAction({ key: "c" })).toBe("compare");
    expect(cullKeyAction({ key: "C" })).toBe("compare");
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

// --- V2：放大跨图保持 ---------------------------------------------------------------

describe("放大锚点（clampZoomPoint / zoomEnterPoint）", () => {
  it("越界收夹到 0-1；NaN 回中心分量", () => {
    expect(clampZoomPoint(0.3, 0.7)).toEqual({ x: 0.3, y: 0.7 });
    expect(clampZoomPoint(-1, 2)).toEqual({ x: 0, y: 1 });
    expect(clampZoomPoint(NaN, Number.POSITIVE_INFINITY)).toEqual({ x: 0.5, y: 0.5 });
  });

  it("按下 Z 起始锚点：无记忆居中，有记忆回上次位置（会话内记忆）", () => {
    expect(zoomEnterPoint(null)).toEqual({ x: 0.5, y: 0.5 });
    expect(zoomEnterPoint({ x: 0.25, y: 0.8 })).toEqual({ x: 0.25, y: 0.8 });
  });
});

// --- V2：对比视图组员选取 -----------------------------------------------------------

describe("comparisonMembers（同连拍组优先，不足补相邻）", () => {
  const items = [
    { assetId: 1, burstId: null }, // 0
    { assetId: 2, burstId: 11 }, // 1
    { assetId: 3, burstId: 11 }, // 2 ← 当前（组 11 兄弟：1、4）
    { assetId: 4, burstId: 11 }, // 3
    { assetId: 5, burstId: null }, // 4
  ];

  it("首位恒为当前片；同组兄弟按距离序补位", () => {
    expect(comparisonMembers(items, 2, 2)).toEqual([2, 1]);
    expect(comparisonMembers(items, 2, 3)).toEqual([2, 1, 3]);
    expect(comparisonMembers(items, 3, 2)).toEqual([3, 2]);
  });

  it("无组：相邻补位（等距左侧优先）；组员不足补相邻", () => {
    expect(comparisonMembers(items, 0, 3)).toEqual([0, 1, 2]);
    expect(comparisonMembers(items, 4, 2)).toEqual([4, 3]);
    // 当前在组内但组员只有 3 个，count=4 → 补最近相邻（左侧 0 距离 2，右侧 4 距离 2？等距左侧优先）
    expect(comparisonMembers(items, 2, 4)).toEqual([2, 1, 3, 0]);
  });

  it("会话总张数不足 count 全量铺开；边界输入回空", () => {
    expect(comparisonMembers(items.slice(0, 2), 0, 4)).toEqual([0, 1]);
    expect(comparisonMembers([], 0, 2)).toEqual([]);
    expect(comparisonMembers(items, -1, 2)).toEqual([]);
    expect(comparisonMembers(items, 5, 2)).toEqual([]);
  });
});

// --- V2：连拍组一键留张 --------------------------------------------------------------

describe("buildKeepOnlyDecisions（本组只留这张）", () => {
  const items = [
    { assetId: 1, burstId: 11 },
    { assetId: 2, burstId: 11 },
    { assetId: 3, burstId: 11 },
    { assetId: 4, burstId: null },
  ];

  it("当前 accepted + 组内未定项批量 rejected（一次 decision_apply）", () => {
    const decisions = new Map<number, CullDecisionValue | null>([[1, "accepted"]]);
    expect(buildKeepOnlyDecisions(items, 2, decisions)).toEqual([
      { assetId: 2, decision: "accepted" },
      { assetId: 3, decision: "rejected" },
    ]);
  });

  it("已手动决定的兄弟不动（尊重手动）；当前已 accepted 免重写", () => {
    const decisions = new Map<number, CullDecisionValue | null>([
      [1, "accepted"],
      [2, "accepted"],
      [3, "rejected"],
    ]);
    expect(buildKeepOnlyDecisions(items, 2, decisions)).toEqual([]);
  });

  it("无组 / 当前片不在会话内返回空", () => {
    expect(buildKeepOnlyDecisions(items, 4, new Map())).toEqual([]);
    expect(buildKeepOnlyDecisions(items, 99, new Map())).toEqual([]);
  });
});

// --- V3：AI 挑图规则组装 -------------------------------------------------------------

describe("buildAiRules（表单态 → cull_ai_prescan 规则）", () => {
  it("合法表单原样组装（预览/应用共用同形 DTO）", () => {
    expect(
      buildAiRules({
        eyesEnabled: true,
        eyesSensitivity: "strong",
        blurEnabled: true,
        blurSensitivity: "weak",
        burstKeepSharpest: true,
        groupExemptFaces: "5",
        maxAccepted: "30",
      }),
    ).toEqual({
      eyes: { enabled: true, sensitivity: "strong" },
      blur: { enabled: true, sensitivity: "weak" },
      burstKeepSharpest: true,
      groupExemptFaces: 5,
      maxAccepted: 30,
    });
  });

  it("敏感度脏值回 normal；豁免人数非法/负数回 0（=关）", () => {
    const rules = buildAiRules({
      eyesEnabled: false,
      eyesSensitivity: "ultra",
      blurEnabled: false,
      blurSensitivity: "",
      burstKeepSharpest: false,
      groupExemptFaces: "abc",
      maxAccepted: "",
    });
    expect(rules.eyes.sensitivity).toBe("normal");
    expect(rules.blur.sensitivity).toBe("normal");
    expect(rules.groupExemptFaces).toBe(0);
    expect(rules.maxAccepted).toBeNull();
    expect(buildAiRules({
      eyesEnabled: false,
      eyesSensitivity: "normal",
      blurEnabled: false,
      blurSensitivity: "normal",
      burstKeepSharpest: false,
      groupExemptFaces: "-3",
      maxAccepted: "",
    }).groupExemptFaces).toBe(0);
  });

  it("精选上限：空/0/非法/负数回 null（=不限）", () => {
    const base = {
      eyesEnabled: true,
      eyesSensitivity: "normal",
      blurEnabled: true,
      blurSensitivity: "normal",
      burstKeepSharpest: false,
      groupExemptFaces: "0",
    };
    expect(buildAiRules({ ...base, maxAccepted: "" }).maxAccepted).toBeNull();
    expect(buildAiRules({ ...base, maxAccepted: "0" }).maxAccepted).toBeNull();
    expect(buildAiRules({ ...base, maxAccepted: "-5" }).maxAccepted).toBeNull();
    expect(buildAiRules({ ...base, maxAccepted: "abc" }).maxAccepted).toBeNull();
    expect(buildAiRules({ ...base, maxAccepted: "1" }).maxAccepted).toBe(1);
  });
});
