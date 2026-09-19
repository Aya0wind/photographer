import { beforeEach, describe, expect, it, vi } from "vitest";

import { resetThumbPipelineForTests, fetchAssetThumb, warmImageDecode } from "./thumbPipeline";
import { assetThumbGet } from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return { ...actual, assetThumbGet: vi.fn() };
});

const thumbMock = vi.mocked(assetThumbGet);

/** 可控在途请求：resolvers 按调用序存放，测试手动放行 */
let resolvers: Array<(path: string | null) => void> = [];

/** flush 全部微任务与定时器宏任务（信号量放行链路跨多个 tick） */
async function flush(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

beforeEach(() => {
  resetThumbPipelineForTests();
  resolvers = [];
  thumbMock.mockReset().mockImplementation(
    () => new Promise<string | null>((resolve) => resolvers.push(resolve)),
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
    resolvers[0]?.("C:\\t\\1.jpg");
    await flush();
    expect(thumbMock.mock.calls[base][0]).toBe(200);

    // 再放行下一个在途（id2）→ 才轮到 low=100
    resolvers.shift();
    resolvers[0]?.(null);
    await flush();
    expect(thumbMock.mock.calls[base + 1][0]).toBe(100);

    resolvers.forEach((r) => r(null));
    await Promise.all(holders).catch(() => undefined);
  });

  it("默认 low：网格/胶片条调用不插队", async () => {
    const holders = Array.from({ length: 6 }, (_, i) => fetchAssetThumb(i + 1, 240));
    await flush();
    const base = thumbMock.mock.calls.length;

    void fetchAssetThumb(100, 240);
    await flush();
    resolvers[0]?.(null);
    await flush();
    expect(thumbMock.mock.calls[base][0]).toBe(100);

    resolvers.forEach((r) => r(null));
    await Promise.all(holders).catch(() => undefined);
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
