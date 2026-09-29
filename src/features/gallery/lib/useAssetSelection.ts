import { useCallback, useEffect, useState } from "react";
import type { AssetDto } from "@/ipc/api";

/** 图库和相册共用的选择状态；页面决定操作对象来自哪个结果集。 */
export function useAssetSelection() {
  const [selecting, setSelecting] = useState(false);
  const [selected, setSelected] = useState<number[]>([]);
  const toggleSelected = useCallback((asset: AssetDto) => {
    setSelected((previous) => previous.includes(asset.id)
      ? previous.filter((id) => id !== asset.id)
      : [...previous, asset.id]);
  }, []);
  const ctrlSelect = useCallback((asset: AssetDto) => {
    setSelecting(true);
    toggleSelected(asset);
  }, [toggleSelected]);
  const exitSelection = useCallback(() => {
    setSelecting(false);
    setSelected([]);
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

  return { selecting, selected, setSelected, toggleSelected, ctrlSelect, exitSelection, contextTargets };
}
