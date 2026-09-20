import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion } from "motion/react";

import {
  importJobDelete,
  importJobsPage,
  indexKickNow,
  indexTaskPause,
  type IndexCounters,
  type IndexKind,
  type IndexStatus,
  type JobRow,
} from "@/ipc/api";
import { formatBytes, formatDateTime, formatSpeed } from "@/lib/format";
import { motionInitial, useMotionOn } from "@/lib/motion";
import { useImportStore, type ActiveJob } from "@/stores/importStore";
import { useAiStore } from "@/stores/aiStore";
import CleanCardDialogLayer from "@/features/import/CleanCardDialog";
import LogViewer from "./LogViewer";

/**
 * 右侧任务抽屉（M4.5：任务 UI 唯一入口——独立任务页已删除）：
 * - TitleBar 任务图标（运行中任务数徽标 = 运行中导入 + 有活儿的索引通道）→ 右侧
 *   滑出 360px 抽屉（surface 底、motion 动画、Esc/背板关闭）
 * - 进行中区：导入任务行（进度/速度/当前文件/暂停/继续/取消；终态行保留清卡入口）
 *   + 缩略图/EXIF/语义/人脸索引行（计数+进度条+暂停/继续）+ 失败红字 + appError 红行
 * - 历史区（下半部可滚动）：importJobsPage(afterId, 50) 分页 +「加载更多」；
 *   每行任务名/时间/终态统计 + 查看日志（LogViewer 弹层）+ × 删除
 *   （importJobDelete，动画退场，本地删除集防重拉回显）
 * - 顶部「全部暂停 / 清除已完成」（批量 importJobDelete 历史行 + 隐藏本地终态行）
 * - indexStatus 1.5s 轮询；总结弹窗全局挂在 AppShell（SummaryModalHost）
 */

/** 索引状态轮询间隔（后端任务账快照；事件驱动之外的自愈兜底） */
const INDEX_POLL_MS = 1500;
/** 历史区分页大小 */
const HISTORY_PAGE_SIZE = 50;

/** 抽屉内索引通道展示序 */
const INDEX_KINDS: readonly IndexKind[] = ["thumb", "exif", "ai", "face"];

const EMPTY_COUNTERS: IndexCounters = { pending: 0, running: 0, done: 0, failed: 0, total: 0 };

/** 通道是否有活儿（进行中） */
function indexActive(counters: IndexCounters): boolean {
  return counters.pending > 0 || counters.running > 0;
}

/** 运行中任务总数（徽标口径）：运行中导入任务 + 有活儿的索引通道 */
export function countRunningTasks(
  activeJobs: Record<number, ActiveJob>,
  indexStatus: IndexStatus | null,
): number {
  let count = 0;
  for (const job of Object.values(activeJobs)) {
    if (job.status === "running" || job.status === "paused") count += 1;
  }
  if (indexStatus !== null) {
    for (const kind of INDEX_KINDS) {
      if (indexActive(indexStatus[kind] ?? EMPTY_COUNTERS)) count += 1;
    }
  }
  return count;
}

/** indexStatus 轮询（抽屉常驻挂载驱动；供徽标/索引行/全局搜索提示共用） */
function useIndexStatusPolling(): void {
  const refreshIndexStatus = useAiStore((s) => s.refreshIndexStatus);
  useEffect(() => {
    void refreshIndexStatus();
    const timer = window.setInterval(() => void refreshIndexStatus(), INDEX_POLL_MS);
    return () => window.clearInterval(timer);
  }, [refreshIndexStatus]);
}

// --- 图标 ---------------------------------------------------------------------------

function GlyphPause({ size = 11 }: { size?: number }) {
  return (
    <svg viewBox="0 0 12 12" width={size} height={size} fill="currentColor" aria-hidden="true">
      <rect x="2.5" y="2" width="2.4" height="8" rx="0.6" />
      <rect x="7.1" y="2" width="2.4" height="8" rx="0.6" />
    </svg>
  );
}

function GlyphPlay({ size = 11 }: { size?: number }) {
  return (
    <svg viewBox="0 0 12 12" width={size} height={size} fill="currentColor" aria-hidden="true">
      <path d="M3.2 2.2v7.6a.6.6 0 0 0 .92.5l5.9-3.8a.6.6 0 0 0 0-1L4.12 1.7a.6.6 0 0 0-.92.5z" />
    </svg>
  );
}

// --- TitleBar 开关按钮（含徽标） ------------------------------------------------------

export function TaskDrawerToggle({
  open,
  onClick,
}: {
  open: boolean;
  onClick: () => void;
}) {
  const { t } = useTranslation();
  const activeJobs = useImportStore((s) => s.activeJobs);
  const indexStatus = useAiStore((s) => s.indexStatus);
  const running = countRunningTasks(activeJobs, indexStatus);

  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={t("taskdrawer.open")}
      aria-pressed={open}
      title={t("taskdrawer.open")}
      data-testid="taskdrawer-toggle"
      className={`pointer-events-auto relative flex h-7 w-8 items-center justify-center rounded-md border transition-colors ${
        open || running > 0
          ? "border-accent/60 text-accent"
          : "border-edge text-text-secondary hover:border-text-muted hover:text-text-primary"
      }`}
    >
      <svg
        viewBox="0 0 16 16"
        width="14"
        height="14"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
      >
        <path d="M5.5 3.5h-2A1.5 1.5 0 0 0 2 5v7a1.5 1.5 0 0 0 1.5 1.5h9A1.5 1.5 0 0 0 14 12V5a1.5 1.5 0 0 0-1.5-1.5h-2" />
        <rect x="5.5" y="1.5" width="5" height="3" rx="1" />
        <path d="M4.8 8.6l1.4 1.4 2.6-2.6" />
      </svg>
      {running > 0 && (
        <span
          className="absolute -right-1 -top-1 flex h-3.5 min-w-3.5 items-center justify-center rounded-full bg-accent px-0.5 font-mono text-[9px] font-bold leading-none text-black"
          data-testid="taskdrawer-count"
        >
          {running}
        </span>
      )}
    </button>
  );
}

// --- 行视图 -------------------------------------------------------------------------

/** 导入任务行（进行中）：标题/进度条/速度/当前文件 + 暂停/继续/取消 */
function ImportActiveRow({ job }: { job: ActiveJob }) {
  const { t } = useTranslation();
  const jobModes = useImportStore((s) => s.jobModes);
  const pauseJob = useImportStore((s) => s.pauseJob);
  const resumeJob = useImportStore((s) => s.resumeJob);
  const cancelJob = useImportStore((s) => s.cancelJob);
  const mode = jobModes[job.jobId] ?? "copy";
  const isPaused = job.status === "paused";
  // 进度条用已结算口径：跳过的重复文件同样推进
  const pct =
    job.totalBytes > 0
      ? Math.min(100, ((job.settledBytes ?? job.doneBytes) / job.totalBytes) * 100)
      : 0;

  return (
    <div
      className="rounded-lg border border-edge bg-bg/60 p-2.5"
      data-testid="taskdrawer-import"
      data-phase={isPaused ? "paused" : "running"}
    >
      <div className="flex items-center justify-between gap-2">
        <h3 className="truncate text-xs font-semibold text-text-primary">
          {t(mode === "move" ? "importCard.jobMove" : "importCard.jobCopy", { id: job.jobId })}
        </h3>
        <span
          className={`shrink-0 rounded px-1.5 py-0.5 text-[10px] font-medium ${
            isPaused ? "bg-yellow-400/15 text-yellow-300" : "bg-accent/15 text-accent"
          }`}
        >
          {t(isPaused ? "jobStatus.paused" : "jobStatus.running")}
        </span>
      </div>
      <div
        className="mt-2 h-1.5 w-full overflow-hidden rounded-full bg-panel"
        role="progressbar"
        aria-valuenow={Math.round(pct)}
        aria-valuemin={0}
        aria-valuemax={100}
        data-testid="taskdrawer-import-progress"
      >
        <div
          className={`h-full rounded-full transition-[width] duration-150 ${
            isPaused ? "bg-yellow-300/70" : "bg-accent"
          }`}
          style={{ width: `${pct}%` }}
        />
      </div>
      <div className="mt-1.5 flex items-center justify-between gap-2 text-[11px] text-text-secondary tabular-nums">
        <span className="shrink-0 font-mono">{formatSpeed(job.bytesPerSec)}</span>
        <span className="font-mono text-text-muted">
          {formatBytes(job.doneBytes)} / {formatBytes(job.totalBytes)}
        </span>
      </div>
      <p
        className="mt-1 truncate font-mono text-[11px] text-text-muted"
        title={job.currentFile}
        data-testid="taskdrawer-import-file"
      >
        {job.currentFile || "—"}
      </p>
      <div className="mt-2 flex gap-1.5">
        <button
          type="button"
          onClick={() => (isPaused ? void resumeJob(job.jobId) : void pauseJob(job.jobId))}
          className="flex flex-1 items-center justify-center gap-1 rounded-md border border-edge px-2 py-1 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
          data-testid="taskdrawer-import-toggle"
        >
          {isPaused ? <GlyphPlay /> : <GlyphPause />}
          {t(isPaused ? "tasks.resume" : "tasks.pause")}
        </button>
        <button
          type="button"
          onClick={() => void cancelJob(job.jobId)}
          className="rounded-md border border-edge px-2.5 py-1 text-[11px] text-text-secondary transition-colors hover:border-red-400 hover:text-red-400"
          data-testid="taskdrawer-import-cancel"
        >
          {t("tasks.cancel")}
        </button>
      </div>
    </div>
  );
}

/** 导入任务行（终态 done/cancelled）：计数行 + 清卡入口（总结弹窗全局自动弹出） */
function ImportFinishedRow({
  job,
  onClean,
}: {
  job: ActiveJob;
  onClean: (jobId: number) => void;
}) {
  const { t } = useTranslation();
  const jobModes = useImportStore((s) => s.jobModes);
  const jobSources = useImportStore((s) => s.jobSources);
  const summary = useImportStore((s) => s.summary);
  const mode = jobModes[job.jobId] ?? "copy";
  const cancelled = job.status === "cancelled";
  const stats = summary && summary.jobId === job.jobId ? summary.stats : null;
  const done = stats ? stats.doneFiles : job.doneFiles;
  const skipped = stats ? stats.skippedDuplicates : 0;
  const failed = stats ? stats.failedFiles : 0;
  // 清卡入口只对 volume/MTP 源任务显示（folder 源是本地纳管，不可清）
  const cleanable = !cancelled && jobSources[job.jobId] !== undefined && jobSources[job.jobId] !== "folder";

  return (
    <div
      className="rounded-lg border border-edge/60 bg-bg/40 p-2.5"
      data-testid="taskdrawer-import"
      data-phase={cancelled ? "cancelled" : "done"}
    >
      <div className="flex items-center gap-2">
        <span
          className={`flex h-4 w-4 shrink-0 items-center justify-center rounded-full text-[10px] ${
            cancelled ? "bg-panel text-text-muted" : "bg-emerald-400/15 text-emerald-400"
          }`}
          aria-hidden="true"
        >
          {cancelled ? "×" : "✓"}
        </span>
        <h3 className="truncate text-xs font-semibold text-text-primary">
          {cancelled
            ? t("jobStatus.cancelled")
            : t(mode === "move" ? "summary.titleMove" : "summary.title")}
          <span className="ml-1 font-mono text-[10px] font-normal text-text-muted">#{job.jobId}</span>
        </h3>
      </div>
      {!cancelled && (
        <p className="mt-1.5 text-[11px] text-text-secondary tabular-nums" data-testid="taskdrawer-import-counts">
          {t(mode === "move" ? "importCard.countRowMove" : "importCard.countRow", {
            done,
            skipped,
            failed,
          })}
        </p>
      )}
      {cleanable && (
        <button
          type="button"
          onClick={() => onClean(job.jobId)}
          className="mt-2 w-full rounded-md border border-edge px-2 py-1 text-[11px] text-text-secondary transition-colors hover:border-red-400 hover:text-red-400"
          data-testid="taskdrawer-import-clean"
        >
          {t("clean.entry")}
        </button>
      )}
    </div>
  );
}

/** 索引通道行：计数 + 进度条 + 暂停（indexTaskPause）/继续（indexKickNow）+ 失败红字 */
function IndexTaskRow({ kind, counters }: { kind: IndexKind; counters: IndexCounters }) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const total = Math.max(counters.total, counters.done + counters.pending + counters.running);
  const pct = total > 0 ? Math.min(100, (counters.done / total) * 100) : 0;

  async function togglePause(): Promise<void> {
    setBusy(true);
    if (counters.running > 0) {
      await indexTaskPause();
    } else {
      try {
        await indexKickNow(kind);
      } catch {
        // 模型未就绪等业务错误：抽屉内静默（设置页 AI tab 有完整引导）
      }
    }
    setBusy(false);
    void useAiStore.getState().refreshIndexStatus();
  }

  return (
    <div
      className="rounded-lg border border-edge/60 bg-bg/40 p-2.5"
      data-testid={`taskdrawer-index-${kind}`}
      data-kind={kind}
      data-running={indexActive(counters)}
    >
      <div className="flex items-center justify-between gap-2">
        <h3 className="truncate text-xs font-medium text-text-primary">
          {t(`backgroundTask.index.${kind}`)}
        </h3>
        <div className="flex shrink-0 items-center gap-1.5">
          <span className="rounded bg-accent/15 px-1.5 py-0.5 text-[10px] font-medium text-accent">
            {t("jobStatus.running")}
          </span>
          <button
            type="button"
            onClick={() => void togglePause()}
            disabled={busy}
            aria-label={counters.running > 0 ? t("tasks.pause") : t("tasks.resume")}
            title={counters.running > 0 ? t("tasks.pause") : t("tasks.resume")}
            className="flex h-5 w-5 items-center justify-center rounded border border-edge text-text-secondary transition-colors hover:border-accent hover:text-accent disabled:opacity-40"
            data-testid="taskdrawer-index-toggle"
          >
            {counters.running > 0 ? <GlyphPause size={10} /> : <GlyphPlay size={10} />}
          </button>
        </div>
      </div>
      <div
        className="mt-2 h-1.5 w-full overflow-hidden rounded-full bg-panel"
        role="progressbar"
        aria-valuenow={Math.round(pct)}
        aria-valuemin={0}
        aria-valuemax={100}
        data-testid="taskdrawer-index-progress"
      >
        <div
          className="h-full rounded-full bg-accent transition-[width] duration-150"
          style={{ width: `${pct}%` }}
        />
      </div>
      <div className="mt-1.5 flex items-center justify-between gap-2 text-[11px] tabular-nums text-text-secondary">
        <span>{t("backgroundTask.progress", { done: counters.done, total })}</span>
        <span className="text-text-muted">
          {t("backgroundTask.queue", { pending: counters.pending, running: counters.running })}
        </span>
      </div>
      {counters.failed > 0 && (
        <p className="mt-1 text-[10px] font-medium text-red-400" data-testid="taskdrawer-index-failed">
          {t("settings.ai.index.failed", { count: counters.failed })}
        </p>
      )}
    </div>
  );
}

/** appError 新错误行：消息 + ×（任务页已删，日志经历史区查看） */
function ErrorRow({ message, onDismiss }: { message: string; onDismiss: () => void }) {
  const { t } = useTranslation();
  return (
    <div
      className="rounded-lg border border-red-400/40 bg-bg/60 p-2.5"
      data-testid="taskdrawer-error"
      role="alert"
    >
      <div className="flex items-center gap-2">
        <span className="flex h-4 w-4 shrink-0 items-center justify-center rounded-full bg-red-400/15 text-[10px] text-red-400" aria-hidden="true">
          !
        </span>
        <h3 className="min-w-0 flex-1 truncate text-xs font-semibold text-red-400">
          {t("importCard.errorTitle")}
        </h3>
        <button
          type="button"
          onClick={onDismiss}
          aria-label={t("importCard.dismiss")}
          className="shrink-0 rounded p-0.5 text-text-muted transition-colors hover:text-text-primary"
          data-testid="taskdrawer-error-dismiss"
        >
          <svg viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true">
            <path d="M4 4l8 8M12 4l-8 8" />
          </svg>
        </button>
      </div>
      <p className="mt-1.5 line-clamp-2 break-all text-[11px] text-text-secondary" title={message}>
        {message}
      </p>
    </div>
  );
}

/** 历史任务行：任务名/时间/终态统计 + 查看日志 + ×（删除，动画退场） */
function HistoryRow({
  row,
  onDelete,
  onOpenLogs,
}: {
  row: JobRow;
  onDelete: (jobId: number) => void;
  onOpenLogs: (jobId: number) => void;
}) {
  const { t } = useTranslation();
  // statsJson = ImportStats JSON（游标分页行内自带）；解析失败静默空统计
  let stats: { doneFiles?: number; skippedDuplicates?: number; failedFiles?: number } = {};
  try {
    stats = JSON.parse(row.statsJson) as typeof stats;
  } catch {
    // 兼容旧数据/异常 JSON
  }
  return (
    <motion.div
      key={row.id}
      initial={false}
      exit={{ opacity: 0, x: 24 }}
      transition={{ duration: 0.15, ease: "easeOut" }}
      className="flex items-center gap-2 rounded-lg border border-edge/40 bg-bg/30 px-2.5 py-2"
      data-testid="taskdrawer-history-row"
      data-job-id={row.id}
      data-status={row.status}
    >
      <div className="min-w-0 flex-1">
        <p className="truncate text-[11px] font-medium text-text-primary" title={row.deviceName}>
          {row.deviceName}
          <span className="ml-1.5 font-mono text-[10px] font-normal text-text-muted">#{row.id}</span>
        </p>
        <p className="mt-0.5 truncate font-mono text-[10px] tabular-nums text-text-muted">
          {formatDateTime(row.startedAt)}
          {stats.doneFiles !== undefined && (
            <span className="ml-2">
              {t("importCard.countRow", {
                done: stats.doneFiles ?? 0,
                skipped: stats.skippedDuplicates ?? 0,
                failed: stats.failedFiles ?? 0,
              })}
            </span>
          )}
        </p>
      </div>
      <button
        type="button"
        onClick={() => onOpenLogs(row.id)}
        className="shrink-0 rounded-md border border-edge px-1.5 py-1 text-[10px] text-text-muted transition-colors hover:border-accent hover:text-accent"
        data-testid="taskdrawer-history-logs"
      >
        {t("taskdrawer.viewLogs")}
      </button>
      <button
        type="button"
        onClick={() => onDelete(row.id)}
        aria-label={t("taskdrawer.deleteJob")}
        title={t("taskdrawer.deleteJob")}
        className="shrink-0 rounded-md p-1 text-text-muted transition-colors hover:bg-red-400/10 hover:text-red-400"
        data-testid="taskdrawer-history-delete"
      >
        <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" aria-hidden="true">
          <path d="M2.5 4h11M6.5 4V2.5h3V4M4 4l.7 9.5h6.6L12 4M6.7 6.8v4.4M9.3 6.8v4.4" />
        </svg>
      </button>
    </motion.div>
  );
}

// --- 历史区 -------------------------------------------------------------------------

/** 历史任务分页（importJobsPage keyset；打开抽屉时刷新首页） */
function useHistory(deletedIds: ReadonlySet<number>) {
  const [rows, setRows] = useState<JobRow[]>([]);
  const [exhausted, setExhausted] = useState(false);
  const [loading, setLoading] = useState(false);
  const cursorRef = useRef<number>(0);

  const loadFirst = useCallback(async () => {
    setLoading(true);
    const page = await importJobsPage(0, HISTORY_PAGE_SIZE);
    cursorRef.current = page.length > 0 ? page[page.length - 1].id : 0;
    setRows(page);
    setExhausted(page.length < HISTORY_PAGE_SIZE);
    setLoading(false);
  }, []);

  const loadMore = useCallback(async () => {
    if (loading || exhausted) return;
    setLoading(true);
    const page = await importJobsPage(cursorRef.current, HISTORY_PAGE_SIZE);
    if (page.length > 0) {
      cursorRef.current = page[page.length - 1].id;
      setRows((prev) => {
        const seen = new Set(prev.map((r) => r.id));
        return [...prev, ...page.filter((r) => !seen.has(r.id))];
      });
    }
    if (page.length < HISTORY_PAGE_SIZE) setExhausted(true);
    setLoading(false);
  }, [loading, exhausted]);

  return { rows: rows.filter((r) => !deletedIds.has(r.id)), exhausted, loading, loadFirst, loadMore, setRows };
}

// --- 抽屉面板 -----------------------------------------------------------------------

export function TaskDrawerPanel({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const motionOn = useMotionOn();
  // 常驻轮询（组件在 AppShell 永不卸载，仅开合内容）：徽标计数与全局搜索框的
  // 索引提示同源消费（M4.5 A1/A2 共用）
  useIndexStatusPolling();

  const activeJobs = useImportStore((s) => s.activeJobs);
  const pauseJob = useImportStore((s) => s.pauseJob);
  const lastError = useImportStore((s) => s.lastError);
  const indexStatus = useAiStore((s) => s.indexStatus);

  // 「清除已完成」：本地隐藏终态导入行 + 批量删除历史行
  const [clearedJobs, setClearedJobs] = useState<Set<number>>(() => new Set());
  // 错误行只响应新到达的 appError（挂载前的旧错误不打扰）
  const [dismissedError, setDismissedError] = useState(lastError);
  const showError = lastError !== null && lastError !== dismissedError;

  // 历史区：删除集（防重拉回显）+ 日志弹层
  const [deletedIds, setDeletedIds] = useState<Set<number>>(() => new Set());
  const [logJobId, setLogJobId] = useState<number | null>(null);
  const history = useHistory(deletedIds);

  // 打开抽屉时刷新历史首页（含删除集清空——重新以任务账为准）
  useEffect(() => {
    if (open) {
      setDeletedIds(new Set());
      void history.loadFirst();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // Esc 关闭（日志弹层打开时优先关弹层）
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        if (logJobId !== null) setLogJobId(null);
        else onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose, logJobId]);

  /** 活跃（running/paused）导入行 + 终态行（未清除的 done/cancelled） */
  const { active, finished } = useMemo(() => {
    const active: ActiveJob[] = [];
    const finished: ActiveJob[] = [];
    for (const job of Object.values(activeJobs)) {
      if (job.status === "running" || job.status === "paused") active.push(job);
      else if (!clearedJobs.has(job.jobId)) finished.push(job);
    }
    active.sort((a, b) => a.jobId - b.jobId);
    finished.sort((a, b) => b.jobId - a.jobId);
    return { active, finished };
  }, [activeJobs, clearedJobs]);

  const indexRows = useMemo<Array<{ kind: IndexKind; counters: IndexCounters }>>(() => {
    if (indexStatus === null) return [];
    return INDEX_KINDS.map((kind) => ({ kind, counters: indexStatus[kind] ?? EMPTY_COUNTERS })).filter(
      ({ counters }) => indexActive(counters) || counters.failed > 0,
    );
  }, [indexStatus]);

  const hasRunning =
    active.length > 0 || indexRows.length > 0;

  /** 全部暂停：运行中导入逐个暂停 + 索引通道暂停 */
  async function pauseAll(): Promise<void> {
    for (const job of active) {
      if (job.status === "running") await pauseJob(job.jobId);
    }
    if (indexRows.length > 0) await indexTaskPause();
    void useAiStore.getState().refreshIndexStatus();
  }

  /** 清除已完成：隐藏本地终态行 + 批量删除历史行（importJobDelete 循环；失败静默跳过） */
  async function clearDone(): Promise<void> {
    setClearedJobs(new Set(Object.keys(activeJobs).map(Number)));
    if (lastError !== null) setDismissedError(lastError);
    for (const row of history.rows) {
      try {
        await importJobDelete(row.id);
        setDeletedIds((prev) => new Set(prev).add(row.id));
      } catch {
        // 单行删除失败静默（下次打开抽屉重新拉取可见）
      }
    }
  }

  async function deleteJob(jobId: number): Promise<void> {
    setDeletedIds((prev) => new Set(prev).add(jobId)); // 乐观移除 + 动画退场
    try {
      await importJobDelete(jobId);
    } catch {
      setDeletedIds((prev) => {
        const next = new Set(prev);
        next.delete(jobId);
        return next;
      });
    }
  }

  // 清卡对话框（终态行入口）
  const [cleanJobId, setCleanJobId] = useState<number | null>(null);

  return (
    <AnimatePresence>
      {open && (
        <>
          <motion.div
            key="backdrop"
            initial={motionInitial(motionOn, { opacity: 0 })}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.15 }}
            onClick={onClose}
            className="fixed inset-0 z-40 bg-black/30"
            data-testid="taskdrawer-backdrop"
          />
          <motion.aside
            key="panel"
            initial={motionInitial(motionOn, { x: 360 })}
            animate={{ x: 0 }}
            exit={{ x: 360 }}
            transition={{ duration: 0.18, ease: "easeOut" }}
            className="fixed right-0 top-0 z-40 flex h-full w-[360px] flex-col border-l border-edge bg-surface shadow-2xl"
            role="dialog"
            aria-label={t("taskdrawer.title")}
            data-testid="taskdrawer"
          >
            {/* 头部：标题 + 全部暂停/清除已完成 + 关闭 */}
            <div className="flex h-10 shrink-0 items-center gap-2 border-b border-edge px-3">
              <h2 className="text-xs font-semibold text-text-primary">{t("taskdrawer.title")}</h2>
              <div className="ml-auto flex items-center gap-1.5">
                {hasRunning && (
                  <button
                    type="button"
                    onClick={() => void pauseAll()}
                    className="rounded-md border border-edge px-2 py-1 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
                    data-testid="taskdrawer-pause-all"
                  >
                    {t("taskdrawer.pauseAll")}
                  </button>
                )}
                {(finished.length > 0 || history.rows.length > 0) && (
                  <button
                    type="button"
                    onClick={() => void clearDone()}
                    className="rounded-md border border-edge px-2 py-1 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
                    data-testid="taskdrawer-clear-done"
                  >
                    {t("taskdrawer.clearDone")}
                  </button>
                )}
                <button
                  type="button"
                  onClick={onClose}
                  aria-label={t("taskdrawer.close")}
                  data-testid="taskdrawer-close"
                  className="flex h-6 w-6 items-center justify-center rounded-md text-text-muted transition-colors hover:bg-panel hover:text-text-primary"
                >
                  <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true">
                    <path d="M4 4l8 8M12 4l-8 8" />
                  </svg>
                </button>
              </div>
            </div>

            {/* 任务流水：进行中（导入/索引/错误） */}
            <div
              className="sp-scroll max-h-[55%] shrink-0 space-y-2 overflow-y-auto border-b border-edge/60 p-3"
              data-testid="taskdrawer-list"
            >
              {active.length === 0 && indexRows.length === 0 && !showError && finished.length === 0 && (
                <div className="flex flex-col items-center gap-2 px-4 py-6 text-center" data-testid="taskdrawer-empty">
                  <p className="text-xs text-text-muted">{t("taskdrawer.empty")}</p>
                  <button
                    type="button"
                    onClick={() => {
                      onClose();
                      navigate("/import");
                    }}
                    className="rounded-md border border-edge px-3 py-1 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
                  >
                    {t("tasks.goImport")}
                  </button>
                </div>
              )}
              {showError && lastError !== null && (
                <ErrorRow message={lastError.message} onDismiss={() => setDismissedError(lastError)} />
              )}
              {indexRows.map(({ kind, counters }) => (
                <IndexTaskRow key={kind} kind={kind} counters={counters} />
              ))}
              {active.map((job) => (
                <ImportActiveRow key={job.jobId} job={job} />
              ))}
              <AnimatePresence initial={false}>
                {finished.map((job) => (
                  <ImportFinishedRow key={job.jobId} job={job} onClean={setCleanJobId} />
                ))}
              </AnimatePresence>
            </div>

            {/* 历史（下半部可滚动；分页 + 删除 + 日志） */}
            <div className="sp-scroll min-h-0 flex-1 overflow-y-auto p-3" data-testid="taskdrawer-history">
              <h3 className="mb-2 text-[10px] font-semibold uppercase tracking-wider text-text-muted">
                {t("taskdrawer.history")}
              </h3>
              {history.rows.length === 0 && !history.loading ? (
                <p className="px-1 py-3 text-[11px] text-text-muted" data-testid="taskdrawer-history-empty">
                  {t("tasks.historyEmpty")}
                </p>
              ) : (
                <div className="space-y-1.5">
                  <AnimatePresence initial={false}>
                    {history.rows.map((row) => (
                      <HistoryRow
                        key={row.id}
                        row={row}
                        onDelete={(id) => void deleteJob(id)}
                        onOpenLogs={setLogJobId}
                      />
                    ))}
                  </AnimatePresence>
                  {!history.exhausted && (
                    <button
                      type="button"
                      disabled={history.loading}
                      onClick={() => void history.loadMore()}
                      className="w-full rounded-md border border-edge px-2 py-1.5 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent disabled:opacity-40"
                      data-testid="taskdrawer-history-more"
                    >
                      {history.loading ? t("tasks.loading") : t("taskdrawer.loadMore")}
                    </button>
                  )}
                </div>
              )}
            </div>
          </motion.aside>

          {/* 日志弹层（历史行「查看日志」） */}
          {logJobId !== null && (
            <div
              className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-6"
              role="dialog"
              aria-modal="true"
              aria-label={t("taskdrawer.logsTitle", { id: logJobId })}
              data-testid="taskdrawer-logs-modal"
              onClick={() => setLogJobId(null)}
            >
              <div
                className="flex max-h-[76vh] w-[560px] flex-col overflow-hidden rounded-xl border border-edge bg-surface shadow-2xl"
                onClick={(e) => e.stopPropagation()}
              >
                <div className="flex h-9 shrink-0 items-center justify-between border-b border-edge px-3">
                  <h3 className="text-xs font-semibold text-text-primary">
                    {t("taskdrawer.logsTitle", { id: logJobId })}
                  </h3>
                  <button
                    type="button"
                    onClick={() => setLogJobId(null)}
                    aria-label={t("common.close")}
                    data-testid="taskdrawer-logs-close"
                    className="rounded-md p-1 text-text-muted transition-colors hover:bg-panel hover:text-text-primary"
                  >
                    <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true">
                      <path d="M4 4l8 8M12 4l-8 8" />
                    </svg>
                  </button>
                </div>
                <div className="min-h-0 flex-1 overflow-y-auto p-2">
                  <LogViewer jobId={logJobId} />
                </div>
              </div>
            </div>
          )}
        </>
      )}
      <CleanCardDialogLayer
        open={cleanJobId !== null}
        jobId={cleanJobId ?? 0}
        onClose={() => setCleanJobId(null)}
      />
    </AnimatePresence>
  );
}
