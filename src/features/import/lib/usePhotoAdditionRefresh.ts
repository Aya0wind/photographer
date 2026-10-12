import { useEffect, useRef } from "react";
import { subscribeAppEvents } from "@/ipc/api";

/** 登记进度合并刷新：最多每秒一次，结束时立即对齐最终照片列表。 */
export function usePhotoAdditionRefresh(refresh: () => void) {
  const callback = useRef(refresh);
  callback.current = refresh;
  useEffect(() => {
    let cancelled = false;
    let off: (() => void) | null = null;
    let timer: number | null = null;
    const schedule = () => {
      if (timer !== null) return;
      timer = window.setTimeout(() => {
        timer = null;
        if (!cancelled) callback.current();
      }, 1000);
    };
    void subscribeAppEvents(event => {
      if (cancelled) return;
      if (event.type === "importSessionFinished" || event.type === "libraryScanFinished") {
        if (timer !== null) window.clearTimeout(timer);
        timer = null;
        callback.current();
      } else if ((event.type === "importFileCompleted" && event.state === "verified") ||
        (event.type === "libraryScanProgress" && event.registered > 0)) schedule();
    }).then(unlisten => { if (cancelled) unlisten(); else off = unlisten; }).catch(() => {});
    return () => {
      cancelled = true;
      if (timer !== null) window.clearTimeout(timer);
      off?.();
    };
  }, []);
}
