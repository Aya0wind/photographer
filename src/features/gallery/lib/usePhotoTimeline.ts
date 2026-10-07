import { useCallback, useEffect, useRef, useState } from "react";
import { assetGroupDates, assetsSeek, type AssetDto, type AssetFilters, type AssetGroupDate } from "@/ipc/api";

const PAGE_SIZE = 100;

/** Date catalog is independent of thumbnail pages. Seek and prepend never walk intervening pages. */
export function usePhotoTimeline({ enabled, scopeKey, filters, currentAssets, onBeforeJump, onJumpFinished, onReplace, onPrepend }: {
  enabled: boolean;
  scopeKey: string;
  filters?: AssetFilters;
  currentAssets: () => AssetDto[];
  onBeforeJump: () => void;
  onJumpFinished: () => void;
  onReplace: (assets: AssetDto[]) => void;
  onPrepend: (assets: AssetDto[]) => void;
}) {
  const [dates, setDates] = useState<AssetGroupDate[]>([]);
  const generation = useRef(0);
  const earlier = useRef(false);
  const loadingEarlier = useRef(false);
  const filtersRef = useRef(filters);
  filtersRef.current = filters;
  useEffect(() => {
    const seq = ++generation.current;
    earlier.current = false;
    loadingEarlier.current = false;
    setDates([]);
    if (enabled) void assetGroupDates(filtersRef.current).then((result) => {
      if (generation.current === seq) setDates(result);
    });
    return () => { generation.current += 1; };
  }, [enabled, scopeKey]);

  const jump = useCallback(async (entry: AssetGroupDate) => {
    const seq = ++generation.current;
    earlier.current = false;
    loadingEarlier.current = false;
    onBeforeJump();
    try {
      const page = await assetsSeek(entry.coverAssetId, PAGE_SIZE, filtersRef.current);
      if (seq !== generation.current) return;
      if (!page.length) throw new Error("timeline.unavailable");
      earlier.current = true;
      onReplace(page);
    } finally {
      if (seq === generation.current) onJumpFinished();
    }
  }, [onBeforeJump, onJumpFinished, onReplace]);

  const prepend = useCallback(async () => {
    if (!earlier.current || loadingEarlier.current) return;
    const anchor = currentAssets()[0];
    if (!anchor) return;
    const seq = generation.current;
    loadingEarlier.current = true;
    try {
      const page = await assetsSeek(anchor.id, PAGE_SIZE, filtersRef.current, true);
      if (seq !== generation.current) return;
      earlier.current = page.length === PAGE_SIZE;
      if (page.length) onPrepend(page);
    } finally {
      if (seq === generation.current) loadingEarlier.current = false;
    }
  }, [currentAssets, onPrepend]);

  return { dates, jump, prepend };
}
