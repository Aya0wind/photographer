import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import {
  albumExportCancel,
  albumExportRun,
  albumExportStatus,
  photoLibraryList,
  subscribeAppEvents,
  type AlbumExportTask,
  type AppEvent,
  type PhotoLibrary,
} from "@/ipc/api";
import {
  findContainingLibrary,
  loadLastExportDir,
  saveLastExportDir,
} from "@/features/albums/lib/exportTarget";

/**
 * 相册/子组「导出为文件夹」对话框（M6 导出前端，计划 2026-10-09 §六）：
 * Photo Hub → LR 互操作——同卷硬链接/跨卷拷贝 + 写全量 XMP 边车，LR 引用导入；
 * 导出物为快照，双向修改互不影响。
 *
 * 三段式（同一对话框内推进）：
 * 1. 配置：目标文件夹默认建议上次导出位置（localStorage 记住）；目标落在任一
 *    照片库内 → 提示「该文件夹会被扫描忽略（内容与库内重复），建议选择照片
 *    库外」，不禁止、允许继续（计划定案：库内目标必然同卷硬链接，行为已被
 *    两级去重闭合）。
 * 2. 进度：albumExportProgress 事件推进 done/total；取消走 album_export_cancel
 *    软信号（当前文件写完即停）；打开对话框时若同作用域任务已在跑则直接呈现。
 * 3. 总结：albumExportFinished 的 exported/linked 计数 + 跳过源缺失成员的说明
 *    （缺失资产按 §五 定案跳过导出，skipped = total - exported）。
 *
 * 后端未实装（P0 骨架）时 albumExportRun 返回业务错误文案或 invoke 不可用
 * error=null，统一内联展示，与 CreateLibraryDialog 同模式。
 */

const FIELD_CLASS =
  "w-full rounded-md border border-edge bg-bg px-2.5 py-1.5 font-mono text-xs text-text-primary outline-none transition-colors focus:border-accent";

/** 导出收尾总结（albumExportFinished 事件 + 终态任务快照归并） */
interface ExportSummary {
  /** 是否全部成员成功（false=部分失败/已取消） */
  ok: boolean;
  exported: number;
  linked: number;
  total: number;
  error: string | null;
}

/** 任务终态 → 总结投影（挂载即已完成/取消/出错的任务走同一路径） */
function summaryFromTask(task: AlbumExportTask): ExportSummary {
  return {
    ok: task.status === "done" && task.error === null,
    exported: task.done,
    linked: task.linked,
    total: task.total,
    error: task.error,
  };
}

export default function ExportAlbumDialog({
  albumId,
  albumName,
  /** 子分组名（null=整个相册）——与后端 album_export_run 契约一致 */
  subgroup,
  /** 当前作用域成员数（可选；仅配置页「共 N 张」提示用） */
  itemCount,
  onClose,
}: {
  albumId: number;
  albumName: string;
  subgroup: string | null;
  itemCount?: number;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  // --- 配置段状态 ----------------------------------------------------------------------
  const [outputDir, setOutputDir] = useState<string>(() => loadLastExportDir() ?? "");
  const [libraries, setLibraries] = useState<Pick<PhotoLibrary, "id" | "name" | "rootPath">[]>([]);
  const [starting, setStarting] = useState(false);
  const [startError, setStartError] = useState<string | null>(null);
  // --- 进度/总结段状态 ------------------------------------------------------------------
  const [phase, setPhase] = useState<"config" | "running" | "finished">("config");
  const [task, setTask] = useState<AlbumExportTask | null>(null);
  const [summary, setSummary] = useState<ExportSummary | null>(null);

  /** 在跑任务的 id（事件过滤用）；run 返回前的早期事件不匹配任何 id，由挂载
   *  status 拉取兜底，见下 */
  const taskIdRef = useRef<number | null>(null);
  /** 最近一次已知 total（finished 事件不带 total，skipped 说明用它补差） */
  const taskTotalRef = useRef<number>(0);

  const dir = outputDir.trim();
  const insideLibrary = findContainingLibrary(dir, libraries);

  // 照片库清单（库内提示判定用；失败自然降级 []——无库即无所谓库内）
  useEffect(() => {
    let cancelled = false;
    void photoLibraryList().then((rows) => {
      if (!cancelled) setLibraries(rows);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  // 事件订阅：进度推进 + 收尾总结（对话框整个生命周期只订一次；taskId 未知时忽略）
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    void subscribeAppEvents((event: AppEvent) => {
      if (event.type === "albumExportProgress" && event.taskId === taskIdRef.current) {
        taskTotalRef.current = event.total;
        setTask((prev) =>
          prev === null
            ? prev
            : { ...prev, status: "running", done: event.done, total: event.total },
        );
      } else if (event.type === "albumExportFinished" && event.taskId === taskIdRef.current) {
        setTask((prev) =>
          prev === null
            ? prev
            : {
                ...prev,
                status: "done",
                done: event.exported,
                linked: event.linked,
                error: event.error ?? null,
              },
        );
        setSummary({
          ok: event.ok,
          exported: event.exported,
          linked: event.linked,
          total: taskTotalRef.current,
          error: event.error ?? null,
        });
        setPhase("finished");
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
    // 订阅只做一次：handler 经 ref 间接读任务 id，不随状态重建
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 挂载拉一次任务状态：同作用域任务已在跑 → 直接进进度段（用户关掉对话框再打开）
  useEffect(() => {
    let cancelled = false;
    void albumExportStatus().then((current) => {
      if (cancelled || current === null) return;
      const scoped =
        current.albumId === albumId && (current.subgroup ?? null) === subgroup;
      if (!scoped) return;
      if (current.status === "queued" || current.status === "running") {
        taskIdRef.current = current.id;
        taskTotalRef.current = current.total;
        setTask(current);
        setPhase("running");
      }
    });
    return () => {
      cancelled = true;
    };
  }, [albumId, subgroup]);

  async function pickDirectory(): Promise<void> {
    try {
      const picked = await openDialog({
        directory: true,
        defaultPath: dir.length > 0 ? dir : undefined,
      });
      if (typeof picked === "string" && picked.length > 0) setOutputDir(picked);
    } catch {
      // 用户取消/对话框不可用：保持现状
    }
  }

  async function start(): Promise<void> {
    if (dir === "" || starting) return;
    setStarting(true);
    setStartError(null);
    const result = await albumExportRun(albumId, subgroup, dir);
    setStarting(false);
    if (!result.ok) {
      // null=invoke 不可用（后端未连接/命令未实装）；字符串=后端业务 Err 文案透传
      setStartError(result.error === null ? t("albums.export.unavailable") : result.error);
      return;
    }
    // 记住本次位置：下次对话框默认建议（计划：默认建议库外路径=记住上次位置）
    saveLastExportDir(dir);
    taskIdRef.current = result.task.id;
    taskTotalRef.current = result.task.total;
    setTask(result.task);
    if (result.task.status === "queued" || result.task.status === "running") {
      setPhase("running");
    } else {
      // 极小相册可能在 run 返回前就收尾（或骨架态直接终态）：按任务快照出总结
      setSummary(summaryFromTask(result.task));
      setPhase("finished");
    }
  }

  const scopeLabel = subgroup === null ? albumName : `${albumName} ‹ ${subgroup}`;
  const pct =
    task !== null && task.total > 0
      ? Math.min(100, Math.round((task.done / task.total) * 100))
      : 0;

  return (
    <div
      className="fixed inset-0 z-[70] flex items-center justify-center bg-black/60"
      onClick={onClose}
      role="dialog"
      aria-modal="true"
      aria-label={t("albums.export.title")}
      data-testid="album-export-dialog"
      data-phase={phase}
    >
      <div
        className="w-[460px] rounded-xl border border-edge bg-surface p-5"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="text-sm font-semibold text-text-primary">{t("albums.export.title")}</h2>
        <p className="mt-1 truncate text-xs text-text-secondary" data-testid="album-export-scope">
          {scopeLabel}
          {typeof itemCount === "number" && itemCount > 0 && (
            <span className="ml-1.5 font-mono tabular-nums text-text-muted">
              {t("albums.export.count", { count: itemCount })}
            </span>
          )}
        </p>

        {phase === "config" && (
          <>
            <p className="mt-2 text-xs leading-relaxed text-text-secondary">
              {t("albums.export.desc")}
            </p>

            <label className="mt-3 flex flex-col gap-1 text-xs text-text-secondary">
              {t("albums.export.outputDir")}
              <div className="flex gap-2">
                <input
                  type="text"
                  value={outputDir}
                  onChange={(e) => {
                    setOutputDir(e.target.value);
                    setStartError(null);
                  }}
                  placeholder={t("albums.export.placeholder")}
                  className={FIELD_CLASS}
                  autoFocus
                  data-testid="album-export-dir"
                />
                <button
                  type="button"
                  onClick={() => void pickDirectory()}
                  className="shrink-0 rounded-md border border-edge px-3 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
                  data-testid="album-export-browse"
                >
                  {t("albums.export.browse")}
                </button>
              </div>
            </label>

            {/* 目标在照片库内：不禁止，提示扫描忽略（计划 §六定案文案） */}
            {insideLibrary !== null && (
              <p
                className="mt-2 rounded-md border border-amber-500/40 bg-amber-500/10 px-2.5 py-1.5 text-[11px] leading-relaxed text-amber-300"
                role="status"
                data-testid="album-export-inside-hint"
                data-library={insideLibrary.name}
              >
                {t("albums.export.insideLibrary", { name: insideLibrary.name })}
              </p>
            )}

            {startError !== null && (
              <p
                className="mt-2 text-[11px] text-red-400"
                role="alert"
                data-testid="album-export-error"
              >
                {startError}
              </p>
            )}

            <div className="mt-4 flex justify-end gap-2">
              <button
                type="button"
                onClick={onClose}
                className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
                data-testid="album-export-cancel-config"
              >
                {t("common.cancel")}
              </button>
              <button
                type="button"
                disabled={dir === "" || starting}
                onClick={() => void start()}
                className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
                data-testid="album-export-confirm"
              >
                {starting ? t("albums.export.exporting") : t("albums.export.confirm")}
              </button>
            </div>
          </>
        )}

        {phase === "running" && task !== null && (
          <>
            <p className="mt-3 font-mono text-[11px] break-all text-text-muted" data-testid="album-export-output">
              {task.outputDir}
            </p>
            <div
              className="mt-3 flex items-center gap-3"
              data-testid="album-export-progress"
              data-done={task.done}
              data-total={task.total}
            >
              <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-edge/70">
                <div
                  className="h-full rounded-full bg-accent transition-[width]"
                  style={{ width: `${pct}%` }}
                  aria-hidden="true"
                />
              </div>
              <span className="shrink-0 font-mono text-[11px] tabular-nums text-text-secondary">
                {t("albums.export.progress", { done: task.done, total: task.total })}
              </span>
            </div>
            <p className="mt-2 text-[11px] leading-relaxed text-text-muted">
              {t("albums.export.runningHint")}
            </p>
            <div className="mt-4 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => void albumExportCancel()}
                className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-red-400/70 hover:text-red-400"
                data-testid="album-export-cancel"
              >
                {t("albums.export.cancelRun")}
              </button>
              <button
                type="button"
                onClick={onClose}
                className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110"
                data-testid="album-export-background"
              >
                {t("albums.export.runInBackground")}
              </button>
            </div>
          </>
        )}

        {phase === "finished" && summary !== null && (
          <div data-testid="album-export-summary" data-ok={summary.ok}>
            <h3
              className={
                summary.ok
                  ? "mt-3 text-sm font-medium text-text-primary"
                  : "mt-3 text-sm font-medium text-amber-300"
              }
              data-testid="album-export-summary-title"
            >
              {t(summary.ok ? "albums.export.doneTitle" : "albums.export.partialTitle")}
            </h3>
            <p className="mt-1.5 text-xs text-text-secondary" data-testid="album-export-summary-exported">
              {t("albums.export.summaryExported", { count: summary.exported })}
            </p>
            {summary.exported > 0 && (
              <p className="mt-1 text-[11px] text-text-muted" data-testid="album-export-summary-linked">
                {t("albums.export.summaryLinked", {
                  linked: summary.linked,
                  copied: summary.exported - summary.linked,
                })}
              </p>
            )}
            {/* 跳过源缺失成员的说明（缺失资产按 §五 定案跳过导出） */}
            {summary.total - summary.exported > 0 && (
              <p
                className="mt-1.5 rounded-md border border-edge bg-panel/55 px-2.5 py-1.5 text-[11px] leading-relaxed text-text-secondary"
                data-testid="album-export-summary-skipped"
              >
                {t("albums.export.summarySkipped", { count: summary.total - summary.exported })}
              </p>
            )}
            {summary.error !== null && (
              <p className="mt-1.5 text-[11px] text-red-400" role="alert" data-testid="album-export-summary-error">
                {summary.error}
              </p>
            )}
            <div className="mt-4 flex justify-end">
              <button
                type="button"
                onClick={onClose}
                className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110"
                data-testid="album-export-close"
              >
                {t("common.close")}
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
