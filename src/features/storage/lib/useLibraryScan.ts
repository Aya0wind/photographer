import { useCallback, useEffect, useRef, useState } from "react";

import {
  photoLibraryScanCancel,
  photoLibraryScanStatus,
  type AppEvent,
  type LibraryScanStatus,
  subscribeAppEvents,
} from "@/ipc/api";

/**
 * 照片库扫描状态（M4f 登记闭环，计划 §三）：「从文件夹建立」的批量登记与
 * 手动放文件的增量扫描共用一条后端登记管道，前端状态来源两路——
 * - `photo_library_scan_status`：挂载拉一次（页面打开时扫描已在跑的场景）；
 * - `libraryScanProgress` / `libraryScanFinished` 事件：实时推进 + 收尾。
 *
 * 最近同步时间为**会话内**从 libraryScanFinished 事件记录（2026-10-09 定案：
 * 不进契约、不持久化，重启后未知显示「—」）。
 */

/** 单库扫描状态（photo_library_scan_status 行 + 事件合并后的前端投影） */
export interface LibraryScanState {
  status: LibraryScanStatus["status"];
  total: number;
  registered: number;
  skipped: number;
  error: string | null;
}

/** 扫描收尾摘要（通知用）：crossLibraryDuplicates 为契约扩展字段——后端
 *  M4b 未实现前不发送，这里防御性读取（undefined 不报错、不显示提示）。
 *  语义（计划 §四）：跨库重复**照常登记**不去重，该计数是 registered 的
 *  子集，绝不能让用户误解为这 N 张没有导入。 */
export interface LibraryScanFinishSummary {
  libraryId: string;
  registered: number;
  skipped: number;
  /** 跨库重复计数（registered 的子集）；后端未发 = undefined */
  crossLibraryDuplicates?: number;
  /** 收尾时刻（epoch 毫秒，会话内） */
  at: number;
}

function rowToState(row: LibraryScanStatus): LibraryScanState {
  return {
    status: row.status,
    total: row.total,
    registered: row.registered,
    skipped: row.skipped,
    error: row.error,
  };
}

/** 事件负载上的契约扩展字段防御读取（number 且有限才采纳） */
function readCrossLibraryDuplicates(payload: object): number | undefined {
  const value = (payload as { crossLibraryDuplicates?: unknown }).crossLibraryDuplicates;
  return typeof value === "number" && Number.isFinite(value) && value > 0 ? value : undefined;
}

/**
 * 订阅照片库扫描进度/收尾并维护状态表。
 * @param onFinished 收尾回调（登记完成通知：含跨库重复提示数据）；用 ref
 *   持有，调用方闭包变化不触发重订阅。
 */
export function useLibraryScan(onFinished?: (summary: LibraryScanFinishSummary) => void): {
  /** libraryId → 扫描状态（无记录 = 该库当前无任务信息） */
  scans: Record<string, LibraryScanState>;
  /** libraryId → 最近同步时刻（epoch 毫秒；仅本会话内收过 libraryScanFinished） */
  lastSyncAt: Record<string, number>;
  /** 取消某库在跑的扫描（photo_library_scan_cancel 软信号；api 层静默失败） */
  cancelScan: (libraryId: string) => void;
} {
  const [scans, setScans] = useState<Record<string, LibraryScanState>>({});
  const [lastSyncAt, setLastSyncAt] = useState<Record<string, number>>({});
  const finishedRef = useRef(onFinished);
  finishedRef.current = onFinished;

  const applyFinished = useCallback((summary: LibraryScanFinishSummary): void => {
    setScans((prev) => ({
      ...prev,
      [summary.libraryId]: {
        status: "done",
        total: prev[summary.libraryId]?.total ?? summary.registered + summary.skipped,
        registered: summary.registered,
        skipped: summary.skipped,
        error: null,
      },
    }));
    setLastSyncAt((prev) => ({ ...prev, [summary.libraryId]: summary.at }));
    finishedRef.current?.(summary);
  }, []);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;

    // 挂载拉一次：页面打开时扫描已在跑（从文件夹建立后台任务/增量扫描）
    void photoLibraryScanStatus().then((rows) => {
      if (cancelled || rows.length === 0) return;
      setScans((prev) => {
        const next = { ...prev };
        for (const row of rows) next[row.libraryId] = rowToState(row);
        return next;
      });
    });

    void subscribeAppEvents((event: AppEvent) => {
      if (event.type === "libraryScanProgress") {
        setScans((prev) => ({
          ...prev,
          [event.libraryId]: {
            status: "running",
            total: event.total,
            registered: event.registered,
            skipped: prev[event.libraryId]?.skipped ?? 0,
            error: null,
          },
        }));
      } else if (event.type === "libraryScanFinished") {
        applyFinished({
          libraryId: event.libraryId,
          registered: event.registered,
          skipped: event.skipped,
          crossLibraryDuplicates: readCrossLibraryDuplicates(event),
          at: Date.now(),
        });
      }
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
  }, [applyFinished]);

  const cancelScan = useCallback((libraryId: string): void => {
    void photoLibraryScanCancel(libraryId);
  }, []);

  return { scans, lastSyncAt, cancelScan };
}
