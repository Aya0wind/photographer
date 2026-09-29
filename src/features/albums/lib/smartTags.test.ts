import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/ipc/api", async (importOriginal) => ({
  ...await importOriginal<typeof import("@/ipc/api")>(),
  searchSemantic: vi.fn(),
}));

import { searchSemantic } from "@/ipc/api";
import { DEFAULT_SMART_TAGS, indexNewTags, indexedTagCover, indexedTagHitCount, loadSmartTags, reindexAllTags, saveSmartTags, unindexedTags } from "./smartTags";

const searchMock = vi.mocked(searchSemantic);

beforeEach(() => {
  localStorage.clear();
  searchMock.mockReset().mockResolvedValue([{ assetId: 42, score: 0.8 }]);
});

describe("smart album tags", () => {
  it("starts with presets and keeps user additions/removals", () => {
    expect(loadSmartTags()).toEqual(DEFAULT_SMART_TAGS);
    saveSmartTags(["星轨", "夜景"]);
    expect(loadSmartTags()).toEqual(["星轨", "夜景"]);
  });

  it("indexes only tags without a stored semantic result", async () => {
    await indexNewTags(["人像", "夜景"]);
    expect(searchMock).toHaveBeenCalledTimes(2);
    expect(indexedTagCover("人像")).toBe(42);
    expect(unindexedTags(["人像", "夜景", "星轨"])).toEqual(["星轨"]);
    await indexNewTags(["人像", "夜景", "星轨"]);
    expect(searchMock).toHaveBeenCalledTimes(3);
    expect(searchMock).toHaveBeenLastCalledWith("星轨", 100);
  });

  it("reindexAllTags forces re-query of every tag (stale zero-hit cache repaired)", async () => {
    // 早于语义完成建索引：0 命中被缓存（智能相册隐藏的根因）
    searchMock.mockResolvedValueOnce([]);
    await indexNewTags(["人像"]);
    expect(indexedTagHitCount("人像")).toBe(0);
    // 语义收尾后全量重建：已索引标签也重跑查询并覆盖缓存
    await reindexAllTags(["人像", "夜景"]);
    expect(searchMock).toHaveBeenCalledTimes(3);
    expect(indexedTagHitCount("人像")).toBe(1);
    expect(indexedTagHitCount("夜景")).toBe(1);
  });
});
