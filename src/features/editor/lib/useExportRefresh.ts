import { useEffect, useRef } from "react";
import { subscribeAppEvents } from "@/ipc/api";

/** 成片登记完成即刷新当前结果集；相册成员关系继续由既有导出服务维护。 */
export function useExportRefresh(refresh: () => void) {
  const callback = useRef(refresh);
  callback.current = refresh;
  useEffect(() => {
    let cancelled = false;
    let off: (() => void) | null = null;
    void subscribeAppEvents((event) => {
      if (event.type === "exportTaskFinished" && event.ok && event.newAssetId != null) callback.current();
    }).then((value) => { if (cancelled) value(); else off = value; }).catch(() => {});
    return () => { cancelled = true; off?.(); };
  }, []);
}
