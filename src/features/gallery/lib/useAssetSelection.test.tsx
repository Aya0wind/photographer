import { describe, expect, it } from "vitest";
import { act, renderHook } from "@testing-library/react";

import { useAssetSelection } from "./useAssetSelection";
import { assetFixture } from "@/test/fixtures";
import type { AssetDto } from "@/ipc/api";

function asset(id: number): AssetDto {
  return assetFixture(id, { capturedAt: "2026-09-18T10:00:00" });
}

const ORDER = [1, 2, 3, 4, 5];

/** Shift 区间选择（2026-09-29）：锚点 + 有序列表驱动的并集区间。 */
describe("useAssetSelection：Shift 区间选择", () => {
  it("点击 A → Shift+点击 C：A/B/C 全进选中（并集，不清已有）", () => {
    const { result } = renderHook(() => useAssetSelection());
    act(() => result.current.ctrlSelect(asset(1)));
    act(() => result.current.toggleSelected(asset(3), { shift: true, order: ORDER }));
    expect(result.current.selected).toEqual([1, 2, 3]);
    expect(result.current.selecting).toBe(true);
  });

  it("反向区间（先点 C 再 Shift+点 A）同样全选；Shift 不动锚点（再 Shift+点 E 从 A 拉）", () => {
    const { result } = renderHook(() => useAssetSelection());
    const asSet = () => [...result.current.selected].sort((a, b) => a - b);
    act(() => result.current.ctrlSelect(asset(4)));
    act(() => result.current.toggleSelected(asset(2), { shift: true, order: ORDER }));
    expect(asSet()).toEqual([2, 3, 4]);
    // 首次 ctrlSelect(4) 设锚；Shift(2) 不动锚 → 再 Shift(5) 从 4 拉
    act(() => result.current.toggleSelected(asset(5), { shift: true, order: ORDER }));
    expect(asSet()).toEqual([2, 3, 4, 5]);
  });

  it("翻转：先选 1-5 再 Shift 点 3 → 1-3 取消，剩 4/5（用户场景）", () => {
    const { result } = renderHook(() => useAssetSelection());
    const asSet = () => [...result.current.selected].sort((a, b) => a - b);
    act(() => result.current.ctrlSelect(asset(1)));
    act(() => result.current.toggleSelected(asset(5), { shift: true, order: ORDER }));
    expect(asSet()).toEqual([1, 2, 3, 4, 5]);
    // 锚点仍是 1：Shift 点 3（已选中）→ 区间 1-3 整段取消
    act(() => result.current.toggleSelected(asset(3), { shift: true, order: ORDER }));
    expect(asSet()).toEqual([4, 5]);
  });

  it("翻转对部分选中的区间：被点照片选中 → 整段取消（未选的无副作用）", () => {
    const { result } = renderHook(() => useAssetSelection());
    const asSet = () => [...result.current.selected].sort((a, b) => a - b);
    act(() => result.current.ctrlSelect(asset(4)));
    act(() => result.current.ctrlSelect(asset(1)));
    // 锚点=1：区间 1-4，被点的 4 已选中 → 1/4 取消，2/3 本就未选
    act(() => result.current.toggleSelected(asset(4), { shift: true, order: ORDER }));
    expect(asSet()).toEqual([]);
  });

  it("无锚点时 Shift 退化为普通选择并起锚", () => {
    const { result } = renderHook(() => useAssetSelection());
    act(() => result.current.toggleSelected(asset(3), { shift: true, order: ORDER }));
    expect(result.current.selected).toEqual([3]);
  });

  it("锚点不在当前视图（换筛选）：Shift 重新起锚为普通选择", () => {
    const { result } = renderHook(() => useAssetSelection());
    act(() => result.current.ctrlSelect(asset(1)));
    // 新视图没有 id 1（翻页/筛选后）
    act(() => result.current.toggleSelected(asset(4), { shift: true, order: [4, 5] }));
    expect(result.current.selected).toEqual([1, 4]);
  });

  it("退出多选清锚点：再 Shift 从头起锚", () => {
    const { result } = renderHook(() => useAssetSelection());
    act(() => result.current.ctrlSelect(asset(2)));
    act(() => result.current.exitSelection());
    act(() => result.current.toggleSelected(asset(5), { shift: true, order: ORDER }));
    expect(result.current.selecting).toBe(false);
    expect(result.current.selected).toEqual([5]);
  });
});
