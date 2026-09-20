import { beforeEach, describe, expect, it, vi } from "vitest";

import { assetViewMark } from "@/ipc/api";
import {
  VIEW_MARK_DEBOUNCE_MS,
  markAssetViewed,
  resetViewMarkForTests,
} from "./viewMark";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    assetViewMark: vi.fn(),
  };
});

const markMock = vi.mocked(assetViewMark);

beforeEach(() => {
  markMock.mockReset().mockResolvedValue(undefined);
  resetViewMarkForTests();
});

describe("浏览打点去抖（markAssetViewed）", () => {
  it("首次打开打点；30s 内重复打开不打点；超过 30s 再次打点", () => {
    markAssetViewed(7, 0);
    expect(markMock).toHaveBeenCalledTimes(1);
    expect(markMock).toHaveBeenCalledWith(7);

    // 30s 窗口内（含边界前一毫秒）：不重复打点
    markAssetViewed(7, 1000);
    markAssetViewed(7, VIEW_MARK_DEBOUNCE_MS - 1);
    expect(markMock).toHaveBeenCalledTimes(1);

    // 超过 30s：重新打点
    markAssetViewed(7, VIEW_MARK_DEBOUNCE_MS);
    expect(markMock).toHaveBeenCalledTimes(2);
  });

  it("不同资产互不影响去抖窗口", () => {
    markAssetViewed(1, 0);
    markAssetViewed(2, 0);
    expect(markMock).toHaveBeenCalledTimes(2);
    expect(markMock).toHaveBeenCalledWith(1);
    expect(markMock).toHaveBeenCalledWith(2);
  });
});
