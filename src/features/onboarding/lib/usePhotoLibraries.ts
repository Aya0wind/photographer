import { useEffect, useState } from "react";

import { photoLibraryList, subscribeAppEvents, type AppEvent, type PhotoLibrary } from "@/ipc/api";

/**
 * 照片库登记表 hook（2026-10-09 单库多照片库）：拉 photo_library_list +
 * photoLibrariesChanged 事件重拉（新建/移除登记/重定位/在线状态翻转全走它）。
 * null=尚未拉到（首帧），[]=确实没有照片库；后端不可用回退 []（自然降级，
 * 门禁按「无照片库」处理——引导页可跳过，不阻塞开发调试）。
 */
export function usePhotoLibraries(): PhotoLibrary[] | null {
  const [libraries, setLibraries] = useState<PhotoLibrary[] | null>(null);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    const pull = () => {
      void photoLibraryList().then((list) => {
        if (!cancelled) setLibraries(list);
      });
    };
    pull();
    void subscribeAppEvents((event: AppEvent) => {
      if (event.type === "photoLibrariesChanged") pull();
    })
      .then((off) => {
        if (cancelled) off();
        else unlisten = off;
      })
      .catch(() => {
        // 非 Tauri 环境（vite dev 预览）静默
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  return libraries;
}
