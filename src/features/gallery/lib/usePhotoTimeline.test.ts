import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { assetFixture } from "@/test/fixtures";
import { assetGroupDates, assetsSeek } from "@/ipc/api";
import { usePhotoTimeline } from "./usePhotoTimeline";

vi.mock("@/ipc/api", () => ({ assetGroupDates: vi.fn(), assetsSeek: vi.fn() }));
const dates = [{ date: "2020-01-01", count: 2, coverAssetId: 10 }];
beforeEach(() => {
  vi.mocked(assetGroupDates).mockReset().mockResolvedValue(dates);
  vi.mocked(assetsSeek).mockReset();
});
describe("timeline page navigation", () => {
  it("seeks one page directly and loads newer photos with the same filters", async () => {
    const onReplace = vi.fn();
    const onPrepend = vi.fn();
    const onBeforeJump = vi.fn();
    const onJumpFinished = vi.fn();
    const filters = { flagged: true };
    vi.mocked(assetsSeek).mockResolvedValueOnce([assetFixture(10)]).mockResolvedValueOnce([assetFixture(20)]);
    const { result } = renderHook(() => usePhotoTimeline({ enabled: true, scopeKey: "favorites", filters, currentAssets: () => [assetFixture(10)], onBeforeJump, onJumpFinished, onReplace, onPrepend }));
    await waitFor(() => expect(result.current.dates).toEqual(dates));
    await act(() => result.current.jump(dates[0]));
    expect(assetsSeek).toHaveBeenCalledWith(10, 100, filters);
    expect(onBeforeJump).toHaveBeenCalledOnce();
    expect(onJumpFinished).toHaveBeenCalledOnce();
    expect(onReplace).toHaveBeenCalledWith([assetFixture(10)]);
    await act(() => result.current.prepend());
    expect(assetsSeek).toHaveBeenLastCalledWith(10, 100, filters, true);
    expect(onPrepend).toHaveBeenCalledWith([assetFixture(20)]);
  });
  it("discards a seek response after the filter scope changes", async () => {
    let resolve!: (value: ReturnType<typeof assetFixture>[]) => void;
    vi.mocked(assetsSeek).mockImplementation(() => new Promise((done) => { resolve = done; }));
    const onReplace = vi.fn();
    const options = { enabled: true, currentAssets: () => [], onBeforeJump: vi.fn(), onJumpFinished: vi.fn(), onReplace, onPrepend: vi.fn() };
    const { result, rerender } = renderHook(({ scopeKey }) => usePhotoTimeline({ ...options, scopeKey }), { initialProps: { scopeKey: "old" } });
    await waitFor(() => expect(result.current.dates).toEqual(dates));
    let pending!: Promise<void>;
    act(() => { pending = result.current.jump(dates[0]); });
    rerender({ scopeKey: "new" });
    await act(async () => { resolve([assetFixture(10)]); await pending; });
    expect(onReplace).not.toHaveBeenCalled();
  });
});
