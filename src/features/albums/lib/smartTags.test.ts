import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/ipc/api", async (importOriginal) => ({
  ...await importOriginal<typeof import("@/ipc/api")>(),
  searchSemantic: vi.fn(),
}));

import { searchSemantic } from "@/ipc/api";
import { DEFAULT_SMART_TAGS, indexNewTags, indexedTagCover, loadSmartTags, saveSmartTags, unindexedTags } from "./smartTags";

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
});
