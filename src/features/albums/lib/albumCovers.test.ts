import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    searchSemantic: vi.fn(),
    assetThumbGet: vi.fn(),
  };
});
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
}));

import { searchSemantic } from "@/ipc/api";
import {
  ALBUM_COVER_CONCURRENCY,
  fetchAlbumCoverAssetId,
  runTaskPool,
} from "./albumCovers";

const searchSemanticMock = vi.mocked(searchSemantic);

/** 等待微任务+宏任务清空（并发池补位断言用；setTimeout 0 非真实等待） */
const flush = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  localStorage.clear();
  searchSemanticMock.mockReset().mockResolvedValue([]);
});

describe("标签封面资产（fetchAlbumCoverAssetId）", () => {
  it("命中：searchSemantic(tag, 1) 返回首条资产 id", async () => {
    searchSemanticMock.mockResolvedValue([{ assetId: 9, score: 0.9 }]);

    await expect(fetchAlbumCoverAssetId("日落")).resolves.toBe(9);
    expect(searchSemanticMock).toHaveBeenCalledWith("日落", 1);
  });

  it("无命中 → null（占位）", async () => {
    searchSemanticMock.mockResolvedValue([]);
    await expect(fetchAlbumCoverAssetId("雪")).resolves.toBeNull();
  });

  it("搜索命令失败（模型未就绪等）静默 null", async () => {
    searchSemanticMock.mockRejectedValue("语义模型未就绪");
    await expect(fetchAlbumCoverAssetId("雪")).resolves.toBeNull();
  });
});

describe("runTaskPool 并发池", () => {
  it(`并发不超过 ${ALBUM_COVER_CONCURRENCY}；完成一个补位下一个`, async () => {
    let inFlight = 0;
    let peak = 0;
    const gates: Array<() => void> = [];
    const started: number[] = [];

    const tasks = Array.from({ length: 5 }, (_, i) => async () => {
      started.push(i);
      inFlight += 1;
      peak = Math.max(peak, inFlight);
      await new Promise<void>((resolve) => gates.push(resolve));
      inFlight -= 1;
    });

    const pool = runTaskPool(tasks, ALBUM_COVER_CONCURRENCY);
    await flush();
    expect(started).toEqual([0, 1, 2]);
    expect(peak).toBe(ALBUM_COVER_CONCURRENCY);

    // 释放第一个 → 第 4 个补位（保持提交序）
    gates[0]();
    await flush();
    expect(started).toEqual([0, 1, 2, 3]);
    expect(peak).toBe(ALBUM_COVER_CONCURRENCY);

    // 释放其余已启动的 → 第 5 个补位；再全部放行后池收尾
    gates.slice(1).forEach((release) => release());
    await flush();
    expect(started).toEqual([0, 1, 2, 3, 4]);
    gates.forEach((release) => release());
    await pool;
    expect(peak).toBe(ALBUM_COVER_CONCURRENCY);
  });

  it("单任务异常不中断池（兜底继续补位）", async () => {
    const started: number[] = [];
    await runTaskPool(
      [
        async () => {
          started.push(0);
          throw new Error("boom");
        },
        async () => {
          started.push(1);
        },
      ],
      2,
    );
    expect(started).toEqual([0, 1]);
  });
});
