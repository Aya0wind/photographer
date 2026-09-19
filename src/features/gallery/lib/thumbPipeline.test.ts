import { beforeEach, describe, expect, it, vi } from "vitest";

import { resetThumbPipelineForTests, fetchAssetThumb, warmImageDecode } from "./thumbPipeline";
import { assetThumbGet, type ThumbGetResult } from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return { ...actual, assetThumbGet: vi.fn() };
});

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: vi.fn((p: string) => `asset://${p}`),
}));

const thumbMock = vi.mocked(assetThumbGet);

/** 可控在途请求：resolvers 按调用序存放，测试手动放行 */
let resolvers: Array<(r: ThumbGetResult) => void> = [];

/** flush 全部微任务和定时器宏任务（信号量放行链路跨多个 tick） */
async function flush(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

beforeEach(() => {
  resetThumbPipelineForTests();
  resolvers = [];
  thumbMock.mockReset().mockImplementation(
    () => new Promise<ThumbGetResult>((resolve) => resolvers.push(resolve)),
  );
});

describe("缩略图管线：信号量优先级", () => {
  it("队列满时 high 插队：先放行一个槽位，队首的 high 先发出请求", async () => {
    // 占满 6 个槽位（并发上限）
    const holders = Array.from({ length: 6 }, (_, i) => fetchAssetThumb(i + 1, 240));
    await flush();
    const base = thumbMock.mock.calls.length;
    expect(base).toBe(6);

    // 低优先级（网格/胶片条语义）先入队，再入队一个高优先级（主图语义）
    void fetchAssetThumb(100, 240, "low");
    await flush();
    void fetchAssetThumb(200, 240, "high");
    await flush();
    expect(thumbMock.mock.calls.length).toBe(base); // 队列中不发出

    // 放行一个槽位 → 队首（high=200）先拿到
    resolvers[0]?.({ status: "ready", path: "C:\\t\\1.jpg" });
    await flush();
    expect(thumbMock.mock.calls[base][0]).toBe(200);

    // 再放行下一个在途（id2）→ 才轮到 low=100
    resolvers.shift();
    resolvers[0]?.({ status: "unavailable" });
    await flush();
    expect(thumbMock.mock.calls[base + 1][0]).toBe(100);

    resolvers.forEach((r) => r({ status: "unavailable" }));
    await Promise.all(holders).catch(() => undefined);
  });

  it("默认 low：网格/胶片条调用不插队", async () => {
    const holders = Array.from({ length: 6 }, (_, i) => fetchAssetThumb(i + 1, 240));
    await flush();
    const base = thumbMock.mock.calls.length;

    void fetchAssetThumb(100, 240);
    await flush();
    resolvers[0]?.({ status: "unavailable" });
    await flush();
    expect(thumbMock.mock.calls[base][0]).toBe(100);

    resolvers.forEach((r) => r({ status: "unavailable" }));
    await Promise.all(holders).catch(() => undefined);
  });
});

describe("三态契约（pending ≠ failed）", () => {
  it("pending：不写缓存、返回 pending（等待 thumbnailReady 重试）", async () => {
    thumbMock.mockResolvedValueOnce({ status: "pending" });
    const r1 = await fetchAssetThumb(501, 240);
    expect(r1).toEqual({ kind: "pending" });

    // 事件后重试 → ready
    thumbMock.mockResolvedValueOnce({ status: "ready", path: "C:\\t\\501.jpg" });
    const r2 = await fetchAssetThumb(501, 240);
    expect(r2).toEqual({ kind: "url", url: expect.stringContaining("501") });

    // ready 结果进会话缓存：不再发请求
    const calls = thumbMock.mock.calls.length;
    const r3 = await fetchAssetThumb(501, 240);
    expect(r3.kind).toBe("url");
    expect(thumbMock.mock.calls.length).toBe(calls);
  });

  it("unavailable：返回 failed 并缓存（同 key 不再发请求）", async () => {
    thumbMock.mockResolvedValueOnce({ status: "unavailable" });
    const r1 = await fetchAssetThumb(502, 240);
    expect(r1).toEqual({ kind: "failed" });

    const calls = thumbMock.mock.calls.length;
    const r2 = await fetchAssetThumb(502, 240);
    expect(r2).toEqual({ kind: "failed" });
    expect(thumbMock.mock.calls.length).toBe(calls);
  });
});

describe("warmImageDecode（解码预热）", () => {
  it("new Image().src 预热 URL；null/undefined 静默跳过", () => {
    const created: Array<{ src: string }> = [];
    class FakeImage {
      src = "";
      constructor() {
        created.push(this);
      }
    }
    vi.stubGlobal("Image", FakeImage);
    try {
      warmImageDecode("asset://C:\\t\\a.jpg");
      warmImageDecode(null);
      warmImageDecode(undefined);
      expect(created).toHaveLength(1);
      expect(created[0].src).toBe("asset://C:\\t\\a.jpg");
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
