import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/ipc/api", async (importOriginal) => ({
  ...await importOriginal<typeof import("@/ipc/api")>(),
  searchSemantic: vi.fn(),
  subscribeAppEvents: vi.fn(),
}));

import { searchSemantic, subscribeAppEvents } from "@/ipc/api";
import type { AppEvent } from "@/ipc/api";
import { loadSmartTags, saveSmartTags } from "./smartTags";
import { initSmartTagAutoIndex, resetSmartTagAutoIndexForTests } from "./smartTagAutoIndex";

const searchMock = vi.mocked(searchSemantic);
const subscribeMock = vi.mocked(subscribeAppEvents);

/** 捕获订阅处理器（init 后即可同步驱动事件） */
async function bindHandler(): Promise<(event: AppEvent) => void> {
  let handler: ((event: AppEvent) => void) | null = null;
  subscribeMock.mockImplementationOnce(async (fn) => {
    handler = fn;
    return () => {};
  });
  await initSmartTagAutoIndex();
  expect(handler).not.toBeNull();
  return handler!;
}

beforeEach(() => {
  localStorage.clear();
  resetSmartTagAutoIndexForTests();
  subscribeMock.mockReset();
  searchMock.mockReset().mockResolvedValue([{ assetId: 7, score: 0.9 }]);
});

describe("smart tag auto index (aiIndexFinished)", () => {
  it("rebuilds all tags when a semantic backfill round finishes with new embeddings", async () => {
    saveSmartTags(["人像", "夜景"]);
    const handler = await bindHandler();
    handler({ type: "aiIndexFinished", done: 12 });
    // 订阅处理器同步启动重建；flush 微任务链
    await vi.waitFor(() => expect(searchMock).toHaveBeenCalledTimes(2));
    expect(searchMock).toHaveBeenCalledWith("人像", 100);
    expect(searchMock).toHaveBeenCalledWith("夜景", 100);
  });

  it("ignores zero-progress rounds and non-semantic events", async () => {
    saveSmartTags(["人像"]);
    const handler = await bindHandler();
    handler({ type: "aiIndexFinished", done: 0 });
    handler({ type: "indexTaskProgress", kind: "ai", done: 3, total: 5 });
    await Promise.resolve();
    expect(searchMock).not.toHaveBeenCalled();
  });

  it("falls back to preset tags when nothing stored", async () => {
    // 存过至少一个标签的库为准：未存储任何标签时退回预设集（非空）
    expect(loadSmartTags().length).toBeGreaterThan(0);
    saveSmartTags(["山"]);
    const handler = await bindHandler();
    handler({ type: "aiIndexFinished", done: 1 });
    // 等整个重建收尾（1 标签 = 1 次调用），避免异步尾巴泄到后续测试
    await vi.waitFor(() => expect(searchMock).toHaveBeenCalledTimes(1));
    expect(searchMock).toHaveBeenCalledWith("山", 100);
  });

  it("swallows search failures (models not ready)", async () => {
    saveSmartTags(["人像"]);
    searchMock.mockRejectedValue(new Error("backend unavailable"));
    const handler = await bindHandler();
    handler({ type: "aiIndexFinished", done: 2 });
    await vi.waitFor(() => expect(searchMock).toHaveBeenCalledTimes(1));
    // 等第一轮重建完整收尾（catch+finally 落地、running 复位）再发下一轮
    await new Promise((resolve) => setTimeout(resolve, 0));
    // 不抛错、不复位绑定：下一轮收尾可再触发
    handler({ type: "aiIndexFinished", done: 3 });
    await vi.waitFor(() => expect(searchMock).toHaveBeenCalledTimes(2));
  });
});
