import { useEffect, useRef } from "react";

/** 结果数变化后重新观察哨兵，让不足一屏的结果继续加载。 */
export function usePageSentinel(enabled: boolean, resultCount: number, appendPage: () => Promise<void>) {
  const sentinelRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const el = sentinelRef.current;
    if (!el || !enabled || typeof IntersectionObserver === "undefined") return;
    const io = new IntersectionObserver(entries => {
      if (entries.some(entry => entry.isIntersecting)) void appendPage();
    }, { rootMargin: "800px 0px" });
    io.observe(el);
    return () => io.disconnect();
  }, [enabled, appendPage, resultCount]);
  return sentinelRef;
}
