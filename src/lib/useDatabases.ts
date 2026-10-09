import { useEffect, useState } from "react";

import {
  databaseList,
  subscribeAppEvents,
  type AppEvent,
  type DatabaseList,
} from "@/ipc/api";

/**
 * 数据库注册表 hook（2026-10-09 多数据库修正）：拉 database_list +
 * databasesChanged 事件重拉（新建/切换/移除全走它）。
 * null=尚未拉到（首帧）；后端不可用回退空快照（门禁按「无数据库」处理，
 * 引导页可跳过，不阻塞开发调试）。GatedShell 门禁/设置页数据库卡片/
 * 侧栏当前库名共用。
 */
export function useDatabases(): DatabaseList | null {
  const [list, setList] = useState<DatabaseList | null>(null);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    const pull = () => {
      void databaseList().then((next) => {
        if (!cancelled) setList(next);
      });
    };
    pull();
    void subscribeAppEvents((event: AppEvent) => {
      if (event.type === "databasesChanged") pull();
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

  return list;
}
