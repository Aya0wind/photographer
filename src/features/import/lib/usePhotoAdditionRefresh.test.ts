import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { subscribeAppEvents, type AppEvent } from "@/ipc/api";
import { usePhotoAdditionRefresh } from "./usePhotoAdditionRefresh";

vi.mock("@/ipc/api", () => ({ subscribeAppEvents: vi.fn() }));
afterEach(() => vi.useRealTimers());

describe("照片登记进度刷新", () => {
  it("连续进度合并为每秒一次，完成立即刷新并取消待执行刷新", async () => {
    vi.useFakeTimers();
    let listener: (event: AppEvent) => void = () => {};
    vi.mocked(subscribeAppEvents).mockImplementation(async callback => { listener = callback; return () => {}; });
    const refresh = vi.fn();
    renderHook(() => usePhotoAdditionRefresh(refresh));
    await act(async () => {});
    act(() => {
      for (let i = 1; i <= 50; i++) listener({ type: "libraryScanProgress", libraryId: "photos", total: 100, registered: i });
      vi.advanceTimersByTime(999);
    });
    expect(refresh).not.toHaveBeenCalled();
    act(() => vi.advanceTimersByTime(1));
    expect(refresh).toHaveBeenCalledTimes(1);
    act(() => {
      listener({ type: "libraryScanProgress", libraryId: "photos", total: 100, registered: 51 });
      listener({ type: "libraryScanFinished", libraryId: "photos", registered: 100, skipped: 0 });
      vi.advanceTimersByTime(1000);
    });
    expect(refresh).toHaveBeenCalledTimes(2);
  });

  it("卸载后不再刷新，并解除订阅", async () => {
    vi.useFakeTimers();
    let listener: (event: AppEvent) => void = () => {};
    const off = vi.fn();
    vi.mocked(subscribeAppEvents).mockImplementation(async callback => { listener = callback; return off; });
    const refresh = vi.fn();
    const hook = renderHook(() => usePhotoAdditionRefresh(refresh));
    await act(async () => {});
    act(() => listener({ type: "libraryScanProgress", libraryId: "photos", total: 100, registered: 1 }));
    hook.unmount();
    act(() => vi.advanceTimersByTime(1000));
    expect(refresh).not.toHaveBeenCalled();
    expect(off).toHaveBeenCalledOnce();
  });
});
