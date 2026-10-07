import { useCallback, useEffect, useRef, useState, type SetStateAction } from "react";
import type { AssetDto } from "@/ipc/api";

/** 切换选项（2026-09-29 Shift 区间选择）：shift=从锚点到该照片的区间
 *  翻转——被点照片未选中 → 整段选中；已选中 → 整段取消（连续两次
 *  Shift 同段即翻转回空，用户定案）。order=当前视图的完整有序 id 列表
 *  （AssetGrid 自算传入，自定义网格的页面自行提供）。 */
export interface ToggleOptions {
  shift?: boolean;
  order?: readonly number[];
}

/** 图库和相册共用的选择状态；页面决定操作对象来自哪个结果集。 */
export function useAssetSelection() {
  const [selecting, setSelecting] = useState(false);
  const [selected, setSelected] = useState<number[]>([]);
  /** 区间锚点：最近一次普通点击（含 Ctrl 进入多选）的照片。Shift 点击
   *  不移动锚点（Explorer 语义：连续 Shift 始终从原锚点拉区间）；退出
   *  多选或锚点不在当前视图时重新起锚。 */
  const anchorRef = useRef<number | null>(null);
  const replaceSelected = useCallback((next: SetStateAction<number[]>) => {
    setSelecting(true);
    setSelected(next);
  }, []);

  const applyToggle = useCallback((asset: AssetDto, opts?: ToggleOptions) => {
    if (opts?.shift && anchorRef.current !== null && opts.order) {
      const anchorIdx = opts.order.indexOf(anchorRef.current);
      const targetIdx = opts.order.indexOf(asset.id);
      if (anchorIdx >= 0 && targetIdx >= 0) {
        const [lo, hi] = anchorIdx < targetIdx ? [anchorIdx, targetIdx] : [targetIdx, anchorIdx];
        const range = opts.order.slice(lo, hi + 1);
        setSelected((previous) => {
          // 区间意图 = 被点照片目标态的翻转（选中→整段取消 / 未选→整段选中）
          const select = !previous.includes(asset.id);
          const next = new Set(previous);
          for (const id of range) {
            if (select) next.add(id);
            else next.delete(id);
          }
          return [...next];
        });
        return;
      }
      // 锚点不在当前视图（换筛选/翻页）：按无锚点语义重新起锚
    }
    anchorRef.current = asset.id;
    setSelected((previous) => previous.includes(asset.id)
      ? previous.filter((id) => id !== asset.id)
      : [...previous, asset.id]);
  }, []);

  const toggleSelected = useCallback((asset: AssetDto, opts?: ToggleOptions) => {
    applyToggle(asset, opts);
  }, [applyToggle]);
  const ctrlSelect = useCallback((asset: AssetDto, opts?: ToggleOptions) => {
    setSelecting(true);
    applyToggle(asset, opts);
  }, [applyToggle]);
  const exitSelection = useCallback(() => {
    setSelecting(false);
    setSelected([]);
    anchorRef.current = null;
  }, []);

  useEffect(() => {
    if (!selecting) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        exitSelection();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selecting, exitSelection]);

  const contextTargets = useCallback((asset: AssetDto, assets: ReadonlyMap<number, AssetDto>) => {
    if (selecting) {
      if (selected.includes(asset.id)) {
        return selected.map((id) => assets.get(id)).filter((item): item is AssetDto => item !== undefined);
      }
      setSelected([asset.id]);
    }
    return [asset];
  }, [selecting, selected]);

  return { selecting, selected, setSelected: replaceSelected, toggleSelected, ctrlSelect, exitSelection, contextTargets };
}
