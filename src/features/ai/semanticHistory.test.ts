import { beforeEach, describe, expect, it } from "vitest";

import {
  SEMANTIC_HISTORY_KEY,
  SEMANTIC_HISTORY_LIMIT,
  clearSemanticHistory,
  loadSemanticHistory,
  recordSemanticQuery,
} from "./semanticHistory";

beforeEach(() => {
  localStorage.clear();
});

describe("语义查询历史", () => {
  it("记录并持久化到 localStorage（JSON 数组，新→旧）", () => {
    const next = recordSemanticQuery("海边日落");
    expect(next).toEqual(["海边日落"]);
    expect(loadSemanticHistory()).toEqual(["海边日落"]);
    expect(JSON.parse(localStorage.getItem(SEMANTIC_HISTORY_KEY) ?? "[]")).toEqual([
      "海边日落",
    ]);
  });

  it("去重：重复查询移到最前，不产生重复项", () => {
    recordSemanticQuery("a");
    recordSemanticQuery("b");
    recordSemanticQuery("c");
    const next = recordSemanticQuery("a");
    expect(next).toEqual(["a", "c", "b"]);
    expect(next.filter((q) => q === "a")).toHaveLength(1);
  });

  it(`上限 ${SEMANTIC_HISTORY_LIMIT} 条：最旧的被挤出`, () => {
    for (const q of ["q1", "q2", "q3", "q4", "q5", "q6"]) recordSemanticQuery(q);
    expect(loadSemanticHistory()).toEqual(["q6", "q5", "q4", "q3", "q2"]);
  });

  it("空白查询不记录；前后空白 trim 后记录", () => {
    expect(recordSemanticQuery("   ")).toEqual([]);
    expect(loadSemanticHistory()).toEqual([]);
    expect(recordSemanticQuery("  猫  ")).toEqual(["猫"]);
  });

  it("存储损坏（非法 JSON / 非数组）静默回退空，且可继续记录", () => {
    localStorage.setItem(SEMANTIC_HISTORY_KEY, "{not json");
    expect(loadSemanticHistory()).toEqual([]);
    localStorage.setItem(SEMANTIC_HISTORY_KEY, JSON.stringify({ x: 1 }));
    expect(recordSemanticQuery("x")).toEqual(["x"]);
  });

  it("clearSemanticHistory 清空", () => {
    recordSemanticQuery("a");
    clearSemanticHistory();
    expect(loadSemanticHistory()).toEqual([]);
    expect(localStorage.getItem(SEMANTIC_HISTORY_KEY)).toBeNull();
  });
});
