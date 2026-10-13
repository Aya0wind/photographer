import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook } from "@testing-library/react";

import {
  resetThumbPipelineForTests,
  fetchAssetThumb,
  useAssetThumbUrl,
  warmImageDecode,
  PENDING_RETRY_MS,
  emitAssetEventForTests,
} from "./thumbPipeline";
import { assetThumbGet, type ThumbGetResult } from "@/ipc/api";
import { convertFileSrc } from "@tauri-apps/api/core";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return { ...actual, assetThumbGet: vi.fn() };
});

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: vi.fn((p: string) => `asset://${p}`),
}));

const thumbMock = vi.mocked(assetThumbGet);
const convertMock = vi.mocked(convertFileSrc);

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
  convertMock.mockReset().mockImplementation((p: string) => `asset://${p}`);
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

it("thumbnail URL cache evicts old entries instead of growing with the whole library", async () => {
  vi.mocked(assetThumbGet).mockImplementation(async (id) => ({status:"ready",path:`${id}.jpg`}));
  for (let id=1; id<=2049; id++) await fetchAssetThumb(id,512);
  const calls=vi.mocked(assetThumbGet).mock.calls.length;
  await fetchAssetThumb(2049,512);
  expect(vi.mocked(assetThumbGet).mock.calls.length).toBe(calls);
  await fetchAssetThumb(1,512);
  expect(vi.mocked(assetThumbGet).mock.calls.length).toBe(calls+1);
});

it("临时 IPC 错误不会永久缓存为失败", async () => {
  thumbMock.mockRejectedValueOnce(new Error("database is locked"))
    .mockResolvedValueOnce({status:"ready",path:"recovered.jpg"});
  expect(await fetchAssetThumb(901,512)).toEqual({kind:"pending"});
  expect(await fetchAssetThumb(901,512)).toEqual({kind:"url",url:"asset://recovered.jpg"});
});

it("索引就绪事件清除已有的永久失败缓存", async () => {
  thumbMock.mockResolvedValueOnce({status:"unavailable"})
    .mockResolvedValueOnce({status:"ready",path:"indexed.jpg"});
  const {result}=renderHook(()=>useAssetThumbUrl(902,512,true));
  await act(async()=>{await flush();});
  expect(result.current.status).toBe("failed");
  act(()=>emitAssetEventForTests({type:"thumbnailReady",assetId:902,size:1024,path:"indexed.jpg"}));
  await act(async()=>{await flush();});
  expect(result.current.status).toBe("ready");
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

// --- pending 兜底重拉（RAW 远图大图升级停滞的根因回归） ------------------------------
//
// thumbnailReady 事件可丢失且无回执：后端生成队列满时直接丢任务不发事件
// （thumb.rs 明言依赖「前端滚动重试自愈」——查看器不滚动）；事件也可能
// 先于命令响应到达（重查被 in-flight 去重吞并）或早于 hook 订阅发出。
// 修复：pending 态每 PENDING_RETRY_MS 重拉直至 settled。

describe("useAssetThumbUrl：pending 周期兜底重拉", () => {
  it("事件永远不到（后端队满丢任务）：pending 周期重拉直至 ready", async () => {
    vi.useFakeTimers();
    try {
      thumbMock.mockReset();
      thumbMock
        .mockResolvedValueOnce({ status: "pending" })
        .mockResolvedValueOnce({ status: "ready", path: "C:\\t\\raw-embed\\701.jpg" });
      const { result } = renderHook(() => useAssetThumbUrl(701, 6000, true, "high"));

      // 首次请求 pending；不发任何事件（模拟回执丢失）
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(result.current.status).toBe("loading");
      expect(thumbMock).toHaveBeenCalledTimes(1);

      // 事件永不到达 → 2.5s 兜底重拉命中缓存 → ready（不再依赖切图自救）
      await act(async () => {
        await vi.advanceTimersByTimeAsync(PENDING_RETRY_MS);
      });
      expect(result.current.status).toBe("ready");
      expect(result.current.url).toContain("701");
      expect(thumbMock).toHaveBeenCalledTimes(2);

      // settled 后停止重拉
      await act(async () => {
        await vi.advanceTimersByTimeAsync(PENDING_RETRY_MS * 3);
      });
      expect(thumbMock).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("事件早于 hook 订阅发出（挂载前错过）：重查无门，周期兜底自愈", async () => {
    vi.useFakeTimers();
    try {
      thumbMock.mockReset();
      thumbMock
        .mockResolvedValueOnce({ status: "pending" })
        .mockResolvedValueOnce({ status: "ready", path: "C:\\t\\raw-embed\\702.jpg" });

      // 事件在 hook 挂载订阅之前已发出——监听集合里还没有人，直接丢
      act(() => {
        emitAssetEventForTests({
          type: "thumbnailReady",
          assetId: 702,
          size: 2048,
          path: "C:\\t\\512\\702.jpg",
        });
      });

      const { result } = renderHook(() => useAssetThumbUrl(702, 6000, true, "high"));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(result.current.status).toBe("loading");

      await act(async () => {
        await vi.advanceTimersByTimeAsync(PENDING_RETRY_MS);
      });
      expect(result.current.status).toBe("ready");
    } finally {
      vi.useRealTimers();
    }
  });

  it("就绪事件先于旧失败响应到达：立即重查，旧响应不能覆盖就绪状态或缓存", async () => {
    vi.useFakeTimers();
    try {
      thumbMock.mockReset();
      let resolveFirst!: (result: ThumbGetResult) => void;
      thumbMock.mockImplementationOnce(() => new Promise<ThumbGetResult>(resolve => { resolveFirst=resolve; }))
        .mockResolvedValueOnce({status:"ready",path:"new-thumb.jpg"});
      const {result}=renderHook(()=>useAssetThumbUrl(703,6000,true,"high"));
      await act(async()=>{await vi.advanceTimersByTimeAsync(0);});
      expect(thumbMock).toHaveBeenCalledTimes(1);
      act(()=>emitAssetEventForTests({type:"thumbnailReady",assetId:703,size:1024,path:"new-thumb.jpg"}));
      await act(async()=>{await vi.advanceTimersByTimeAsync(0);});
      expect(thumbMock).toHaveBeenCalledTimes(2);
      expect(result.current.status).toBe("ready");
      await act(async()=>{resolveFirst({status:"unavailable"});await vi.advanceTimersByTimeAsync(0);});
      expect(result.current.status).toBe("ready");
      expect(await fetchAssetThumb(703,6000)).toEqual({kind:"url",url:"asset://new-thumb.jpg"});
      expect(thumbMock).toHaveBeenCalledTimes(2);
    } finally {vi.useRealTimers();}
  });

  it("failed 结算即停止重拉；enabled=false 不请求不轮询", async () => {
    vi.useFakeTimers();
    try {
      thumbMock.mockReset().mockResolvedValue({ status: "unavailable" });
      const { result } = renderHook(() => useAssetThumbUrl(704, 6000, true, "high"));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(result.current.status).toBe("failed");
      await act(async () => {
        await vi.advanceTimersByTimeAsync(PENDING_RETRY_MS * 3);
      });
      expect(thumbMock).toHaveBeenCalledTimes(1); // failed 缓存拦截，无重拉

      const disabled = renderHook((props: { enabled: boolean }) =>
        useAssetThumbUrl(705, 6000, props.enabled, "high"),
        { initialProps: { enabled: false } },
      );
      await act(async () => {
        await vi.advanceTimersByTimeAsync(PENDING_RETRY_MS * 3);
      });
      expect(disabled.result.current.status).toBe("loading");
      expect(thumbMock).toHaveBeenCalledTimes(1); // 未发过请求
    } finally {
      vi.useRealTimers();
    }
  });
});

// --- missing 终态（源文件被第三方移动/删除，2026-09-28 边界测试修复） ------------------

describe("missing 终态（源文件被移动/删除）", () => {
  it("missing 带 cachedPath：返回 missing+尽力 url，并缓存结果（同 key 不再发请求）", async () => {
    thumbMock.mockResolvedValueOnce({ status: "missing", cachedPath: "C:\\thumbs\\cache\\601.jpg" });
    const r1 = await fetchAssetThumb(601, 240);
    expect(r1).toEqual({ kind: "missing", url: "asset://C:\\thumbs\\cache\\601.jpg" });

    // 防风暴：缺失是磁盘事实，同 key 不再发请求；缓存结果保持 missing 语义（带 url）
    const calls = thumbMock.mock.calls.length;
    const r2 = await fetchAssetThumb(601, 240);
    expect(r2).toEqual(r1);
    expect(thumbMock.mock.calls.length).toBe(calls);
  });

  it("missing 无 cachedPath / convertFileSrc 抛错（非 Tauri）：url=null 按无缓存处理", async () => {
    thumbMock.mockResolvedValueOnce({ status: "missing", cachedPath: null });
    const r1 = await fetchAssetThumb(602, 240);
    expect(r1).toEqual({ kind: "missing", url: null });

    convertMock.mockImplementationOnce(() => {
      throw new Error("non-tauri");
    });
    thumbMock.mockResolvedValueOnce({ status: "missing", cachedPath: "C:\\gone\\603.jpg" });
    const r2 = await fetchAssetThumb(603, 240);
    expect(r2).toEqual({ kind: "missing", url: null });
  });

  it("hook：missing 是终态（有/无缓存皆然），不排周期重拉", async () => {
    vi.useFakeTimers();
    try {
      thumbMock.mockReset().mockResolvedValue({ status: "missing", cachedPath: "C:\\cache\\604.jpg" });
      const withCache = renderHook(() => useAssetThumbUrl(604, 6000, true, "high"));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(withCache.result.current).toEqual({
        url: "asset://C:\\cache\\604.jpg",
        status: "missing",
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(PENDING_RETRY_MS * 4);
      });
      expect(thumbMock).toHaveBeenCalledTimes(1); // 无周期重发

      thumbMock.mockClear().mockResolvedValue({ status: "missing", cachedPath: null });
      const noCache = renderHook(() => useAssetThumbUrl(605, 6000, true, "high"));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(noCache.result.current).toEqual({ url: null, status: "missing" });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(PENDING_RETRY_MS * 4);
      });
      expect(thumbMock).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("missing 后 assetsPresenceChanged：按资产清缓存重查，瓦片复活（fetch 层）", async () => {
    thumbMock.mockResolvedValueOnce({ status: "missing", cachedPath: null });
    const r1 = await fetchAssetThumb(607, 240);
    expect(r1).toEqual({ kind: "missing", url: null });
    // 缺失是终态缓存：不触发事件时同 key 不再发请求
    const again = await fetchAssetThumb(607, 240);
    expect(again).toEqual({ kind: "missing", url: null });
    expect(thumbMock).toHaveBeenCalledTimes(1);

    // 源文件移回/库回线（扫描轮 presence 精准事件）：按资产失效 → 重查取新结果
    emitAssetEventForTests({
      type: "assetsPresenceChanged",
      libraryId: "lib-1",
      assetIds: [607],
    });
    thumbMock.mockResolvedValueOnce({ status: "ready", path: "C:/t/607.jpg" });
    const r2 = await fetchAssetThumb(607, 240);
    expect(r2).toEqual({ kind: "url", url: expect.stringContaining("607") });
  });

  it("hook：assetsPresenceChanged 命中本资产 → missing 终态复活重查", async () => {
    vi.useFakeTimers();
    try {
      thumbMock.mockReset().mockResolvedValueOnce({ status: "missing", cachedPath: null });
      const view = renderHook(() => useAssetThumbUrl(608, 6000, true, "high"));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(view.result.current).toEqual({ url: null, status: "missing" });
      expect(thumbMock).toHaveBeenCalledTimes(1);

      // 他库/他资产的事件不复活；随后本资产事件到达 → 立即重查
      thumbMock.mockClear().mockResolvedValueOnce({ status: "ready", path: "C:/t/608.jpg" });
      await act(async () => {
        emitAssetEventForTests({
          type: "assetsPresenceChanged",
          libraryId: "lib-1",
          assetIds: [999],
        });
      });
      expect(thumbMock).not.toHaveBeenCalled();
      await act(async () => {
        emitAssetEventForTests({
          type: "assetsPresenceChanged",
          libraryId: "lib-1",
          assetIds: [608],
        });
      });
      expect(view.result.current).toEqual({
        url: "asset://C:/t/608.jpg",
        status: "ready",
      });
    } finally {
      vi.useRealTimers();
    }
  });

  it("missing 后 thumbnailReady：按资产清除缓存，重查取新结果（不复活旧缓存）", async () => {
    thumbMock.mockResolvedValueOnce({ status: "missing", cachedPath: null });
    const r1 = await fetchAssetThumb(606, 240);
    expect(r1).toEqual({ kind: "missing", url: null });

    // 同资产任一档位补齐（如用户把文件移回 + 重扫）：事件清除 failedCache → 重查发新请求
    emitAssetEventForTests({
      type: "thumbnailReady",
      assetId: 606,
      size: 240,
      path: "C:\\t\\606.jpg",
    });
    thumbMock.mockResolvedValueOnce({ status: "ready", path: "C:\\t\\606.jpg" });
    const r2 = await fetchAssetThumb(606, 240);
    expect(r2).toEqual({ kind: "url", url: expect.stringContaining("606") });
  });
});

// --- photoLibrariesChanged 删库联动（2026-10-09） ------------------------------------
//
// 删除照片库只发该事件（不换库）：SQLite 无 AUTOINCREMENT，删掉 id 最大的库后
// 新导入会复用旧 assetId——缓存不整表清，新资产命中旧库缩略图=张冠李戴。

describe("photoLibrariesChanged：整表清缓存", () => {
  it("ready 命中缓存后事件到达 → 缓存被清，第二次请求重新走管线", async () => {
    thumbMock.mockResolvedValueOnce({ status: "ready", path: "C:\\t\\old\\801.jpg" });
    const r1 = await fetchAssetThumb(801, 240);
    expect(r1).toEqual({ kind: "url", url: "asset://C:\\t\\old\\801.jpg" });
    expect(thumbMock).toHaveBeenCalledTimes(1);

    // 命中会话缓存：不再发请求
    await fetchAssetThumb(801, 240);
    expect(thumbMock).toHaveBeenCalledTimes(1);

    // 删库事件 → 整表清 → 同 key 再请求重新走管线（拿到新库的图）
    emitAssetEventForTests({ type: "photoLibrariesChanged" });
    thumbMock.mockResolvedValueOnce({ status: "ready", path: "C:\\t\\new\\801.jpg" });
    const r2 = await fetchAssetThumb(801, 240);
    expect(thumbMock).toHaveBeenCalledTimes(2);
    expect(r2).toEqual({ kind: "url", url: "asset://C:\\t\\new\\801.jpg" });
  });

  it("failed/missing 终态缓存一并清（防风暴缓存不再拦截重查）", async () => {
    thumbMock.mockResolvedValueOnce({ status: "missing", cachedPath: null });
    expect(await fetchAssetThumb(802, 240)).toEqual({ kind: "missing", url: null });
    // 终态缓存拦截：同 key 不再发请求
    await fetchAssetThumb(802, 240);
    expect(thumbMock).toHaveBeenCalledTimes(1);

    emitAssetEventForTests({ type: "photoLibrariesChanged" });
    thumbMock.mockResolvedValueOnce({ status: "ready", path: "C:\\t\\new\\802.jpg" });
    const r = await fetchAssetThumb(802, 240);
    expect(thumbMock).toHaveBeenCalledTimes(2);
    expect(r).toEqual({ kind: "url", url: "asset://C:\\t\\new\\802.jpg" });
  });

  it("事件时在途的旧请求结算不回写缓存（清空不被复活）", async () => {
    // 首请求悬挂（删库事件先到，旧请求之后才结算）
    const pending = fetchAssetThumb(803, 240);
    await flush();
    emitAssetEventForTests({ type: "photoLibrariesChanged" });

    resolvers[0]({ status: "ready", path: "C:\\t\\old\\803.jpg" });
    // 旧请求本身仍拿到结果（清的是缓存，不吊死调用方）
    expect(await pending).toEqual({ kind: "url", url: "asset://C:\\t\\old\\803.jpg" });

    // 但缓存未回写：新请求（新库同 id）重新走管线，不命中旧结果
    thumbMock.mockResolvedValueOnce({ status: "ready", path: "C:\\t\\new\\803.jpg" });
    const fresh = await fetchAssetThumb(803, 240);
    expect(fresh).toEqual({ kind: "url", url: "asset://C:\\t\\new\\803.jpg" });
  });
});
