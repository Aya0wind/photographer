import { Fragment, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion } from "motion/react";

import type { JobRow, JobStatus } from "@/ipc/api";
import { indexKickNow, indexTaskPause, type IndexKind } from "@/ipc/api";
import { formatBytes, formatDateTime, formatDuration, formatSpeed } from "@/lib/format";
import { useAiStore } from "@/stores/aiStore";
import { useImportStore, type ActiveJob, type JobSummary } from "@/stores/importStore";
import CleanCardDialogLayer from "@/features/import/CleanCardDialog";
import LogViewer from "./LogViewer";

/**
 * 任务中心（A 密度）：顶部当前任务卡（大进度条/速度/已完成/当前文件滚动/
 * 暂停恢复取消）+ 下方历史任务表（JobRow 游标分页，行展开日志）。
 * importSessionFinished 自动弹出总结弹窗（成功/跳过/失败三卡片 + 失败重试）。
 */

const STATUS_BADGE: Record<JobStatus, string> = {
  running: "bg-accent/15 text-accent",
  paused: "bg-yellow-400/15 text-yellow-300",
  done: "bg-emerald-400/15 text-emerald-400",
  cancelled: "bg-panel text-text-muted",
  failed: "bg-red-400/15 text-red-400",
};

function StatusBadge({ status }: { status: JobStatus }) {
  const { t } = useTranslation();
  return (
    <span className={`rounded px-1.5 py-0.5 text-[11px] font-medium ${STATUS_BADGE[status]}`}>
      {t(`jobStatus.${status}`)}
    </span>
  );
}

/** 当前文件名滚动条：仅溢出时启用跑马灯（复制两份无缝循环） */
function FileNameTicker({ text }: { text: string }) {
  const boxRef = useRef<HTMLDivElement>(null);
  const [overflow, setOverflow] = useState(false);

  useEffect(() => {
    const el = boxRef.current;
    setOverflow(Boolean(el && el.scrollWidth > el.clientWidth));
  }, [text]);

  const lineClass = "whitespace-nowrap pr-12 font-mono text-[11px] text-text-secondary";

  return (
    <div ref={boxRef} className="overflow-hidden" data-testid="current-file">
      <span className={`inline-flex ${overflow ? "sp-marquee-track" : ""}`}>
        <span className={lineClass}>{text || "—"}</span>
        {overflow && (
          <span className={lineClass} aria-hidden="true">
            {text}
          </span>
        )}
      </span>
    </div>
  );
}

/** 索引任务卡：启动恢复事件带出待处理项数 + index_status 三类计数 + 立即开始/暂停。
 *  索引=库级后台任务（缩略图/EXIF 深提取/语义），退出重开自动恢复；
 *  计数快照进页面拉一次，indexTaskProgress 事件经 aiStore 驱动重拉。 */
function IndexTaskCard() {
  const { t } = useTranslation();
  const indexPending = useImportStore((s) => s.indexPending);
  const status = useAiStore((s) => s.indexStatus);
  const refreshIndexStatus = useAiStore((s) => s.refreshIndexStatus);

  useEffect(() => {
    void refreshIndexStatus();
  }, [refreshIndexStatus]);

  if (indexPending === null) return null;

  // 有待办的索引类型才给「立即开始」（indexKickNow 幂等；失败静默——计数以快照/事件为准）
  const kickable: IndexKind[] = status
    ? ([
        status.thumb.pending > 0 ? "thumb" : null,
        status.exif.pending > 0 ? "exif" : null,
        status.ai.total > 0 && status.ai.done < status.ai.total ? "ai" : null,
      ].filter((kind): kind is IndexKind => kind !== null))
    : [];

  async function kick(kind: IndexKind): Promise<void> {
    try {
      await indexKickNow(kind);
    } catch {
      return; // ai 模型未就绪等业务错误：任务中心静默，设置页 AI tab 有引导
    }
    void refreshIndexStatus();
  }

  return (
    <section
      className="flex items-center justify-between gap-3 rounded-lg border border-edge bg-surface px-4 py-3"
      data-testid="task-index"
    >
      <div className="flex min-w-0 items-center gap-2.5">
        <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-panel text-text-muted">
          <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <rect x="3.5" y="4.5" width="9" height="8" rx="1.5" />
            <path d="M5.5 4.5V3h5v1.5M5.5 8h5M5.5 10.5h3" />
          </svg>
        </span>
        <div className="min-w-0">
          <span className="block truncate text-sm text-text-primary">
            {t("tasks.index.pending", { count: indexPending })}
          </span>
          {status && (
            <span
              className="mt-0.5 block font-mono text-[11px] tabular-nums text-text-muted"
              data-testid="task-index-status"
            >
              {[
                t("tasks.index.statusThumb", { ...status.thumb }),
                t("tasks.index.statusExif", { ...status.exif }),
                t("tasks.index.statusAi", { ...status.ai }),
              ].join("　")}
            </span>
          )}
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-1.5">
        {kickable.map((kind) => (
          <button
            key={kind}
            type="button"
            onClick={() => void kick(kind)}
            className="rounded-md bg-accent px-2.5 py-1 text-[11px] font-medium text-black transition-colors hover:brightness-110"
            data-testid={`task-index-kick-${kind}`}
          >
            {t("tasks.index.kick")}
          </button>
        ))}
        <button
          type="button"
          onClick={() => void indexTaskPause()}
          className="rounded-md border border-edge px-3 py-1 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
          data-testid="task-index-pause"
        >
          {t("tasks.index.pause")}
        </button>
      </div>
    </section>
  );
}

function CurrentJobCard({ job }: { job: ActiveJob | null }) {
  const { t } = useTranslation();
  const pauseJob = useImportStore((s) => s.pauseJob);
  const resumeJob = useImportStore((s) => s.resumeJob);
  const cancelJob = useImportStore((s) => s.cancelJob);
  const jobModes = useImportStore((s) => s.jobModes);

  if (!job) {
    return (
      <section className="rounded-lg border border-edge bg-surface p-6 text-center" data-testid="task-idle">
        <p className="text-sm text-text-muted">{t("tasks.idle")}</p>
      </section>
    );
  }

  const pct = job.totalBytes > 0 ? Math.min(100, (job.doneBytes / job.totalBytes) * 100) : 0;
  const isActive = job.status === "running" || job.status === "paused";
  const isMove = (jobModes[job.jobId] ?? "copy") === "move";

  return (
    <section className="rounded-lg border border-edge bg-surface p-4" data-testid="task-current">
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-2.5">
          <h2 className="text-sm font-semibold text-text-primary">
            {t(isMove ? "tasks.currentJobMove" : "tasks.currentJob", { id: job.jobId })}
          </h2>
          <StatusBadge status={job.status} />
        </div>
        {isActive && (
          <div className="flex gap-1.5">
            <button
              type="button"
              onClick={() =>
                job.status === "running" ? void pauseJob(job.jobId) : void resumeJob(job.jobId)
              }
              className="rounded-md border border-edge px-3 py-1 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
            >
              {job.status === "running" ? t("tasks.pause") : t("tasks.resume")}
            </button>
            <button
              type="button"
              onClick={() => void cancelJob(job.jobId)}
              className="rounded-md border border-edge px-3 py-1 text-xs text-text-secondary transition-colors hover:border-red-400 hover:text-red-400"
            >
              {t("tasks.cancel")}
            </button>
          </div>
        )}
      </div>

      {/* 大进度条：doneBytes / totalBytes */}
      <div
        className="mt-3 h-2.5 w-full overflow-hidden rounded-full bg-panel"
        role="progressbar"
        aria-valuenow={Math.round(pct)}
        aria-valuemin={0}
        aria-valuemax={100}
        data-testid="task-progress"
      >
        <div
          className={`h-full rounded-full transition-[width] duration-150 ${job.status === "paused" ? "bg-yellow-300/70" : "bg-accent"}`}
          style={{ width: `${pct}%` }}
        />
      </div>

      <div className="mt-2 flex items-center justify-between gap-4">
        <div className="flex min-w-0 flex-1 items-center gap-3 text-xs text-text-secondary tabular-nums">
          <span className="font-mono">{formatSpeed(job.bytesPerSec)}</span>
          <span>
            {t("tasks.filesProgress", { done: job.doneFiles, total: job.totalFiles })}
          </span>
          <span className="font-mono text-text-muted">
            {formatBytes(job.doneBytes)} / {formatBytes(job.totalBytes)}
          </span>
        </div>
        <div className="w-[40%] min-w-0">
          <FileNameTicker text={job.currentFile} />
        </div>
      </div>
    </section>
  );
}

function HistoryTable({ onOpenClean }: { onOpenClean: (jobId: number) => void }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const history = useImportStore((s) => s.history);
  const loadHistory = useImportStore((s) => s.loadHistory);
  const [expandedId, setExpandedId] = useState<number | null>(null);

  return (
    <section className="rounded-lg border border-edge bg-surface" data-testid="task-history">
      <div className="flex h-9 items-center justify-between border-b border-edge px-3">
        <h2 className="text-xs font-semibold text-text-secondary">{t("tasks.history")}</h2>
      </div>
      {history.rows.length === 0 ? (
        // 空态：引导文案 + 去导入（标题处不再重复“暂无历史任务”）
        <div className="flex flex-col items-center gap-2.5 px-3 py-8 text-center" data-testid="task-history-empty">
          <p className="text-xs text-text-muted">{t("tasks.historyEmptyHint")}</p>
          <button
            type="button"
            onClick={() => navigate("/import")}
            className="rounded-md border border-edge px-3 py-1 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
          >
            {t("tasks.goImport")}
          </button>
        </div>
      ) : (
        <table className="w-full table-fixed border-collapse text-xs">
          <thead>
            <tr className="text-left text-[11px] text-text-muted">
              <th className="w-6 px-2 py-1.5" aria-label={t("tasks.expand")} />
              <th className="w-20 px-2 py-1.5 font-normal">{t("tasks.columnStatus")}</th>
              <th className="px-2 py-1.5 font-normal">{t("tasks.columnDevice")}</th>
              <th className="w-24 px-2 py-1.5 text-right font-normal">{t("tasks.columnFiles")}</th>
              <th className="w-24 px-2 py-1.5 text-right font-normal">{t("tasks.columnDuration")}</th>
              <th className="w-36 px-2 py-1.5 text-right font-normal">{t("tasks.columnStartedAt")}</th>
              <th className="w-20 px-2 py-1.5" aria-label={t("tasks.columnActions")} />
            </tr>
          </thead>
          <tbody>
            {history.rows.map((row: JobRow) => {
              const expanded = expandedId === row.id;
              const duration = row.finishedAt !== null ? row.finishedAt - row.startedAt : null;
              // 已完成 + volume/MTP 源（folder 源是本地纳管）才可清卡
              const cleanable = row.status === "done" && !row.deviceId.startsWith("FOLDER:");
              return (
                <Fragment key={row.id}>
                  <tr
                    className={`cursor-pointer border-t border-edge/60 hover:bg-panel/30 ${expanded ? "bg-panel/30" : ""}`}
                    onClick={() => setExpandedId(expanded ? null : row.id)}
                    data-testid={`history-row-${row.id}`}
                  >
                    <td className="px-2 py-1.5">
                      <svg
                        viewBox="0 0 16 16"
                        width="10"
                        height="10"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="1.6"
                        className={`text-text-muted transition-transform ${expanded ? "rotate-90" : ""}`}
                        aria-hidden="true"
                      >
                        <path d="M5 3l5 5-5 5" />
                      </svg>
                    </td>
                    <td className="px-2 py-1.5">
                      <StatusBadge status={row.status} />
                    </td>
                    <td className="truncate px-2 py-1.5 text-text-secondary" title={row.deviceName}>
                      {row.deviceName}
                    </td>
                    <td className="px-2 py-1.5 text-right font-mono text-text-secondary tabular-nums">
                      {row.totalFiles}
                    </td>
                    <td className="px-2 py-1.5 text-right font-mono text-[11px] text-text-muted tabular-nums">
                      {formatDuration(duration)}
                    </td>
                    <td className="px-2 py-1.5 text-right font-mono text-[11px] text-text-muted">
                      {formatDateTime(row.startedAt)}
                    </td>
                    <td className="px-2 py-1.5 text-right">
                      {cleanable && (
                        <button
                          type="button"
                          onClick={(e) => {
                            e.stopPropagation();
                            onOpenClean(row.id);
                          }}
                          className="text-[11px] text-text-muted transition-colors hover:text-red-400"
                          data-testid={`history-clean-${row.id}`}
                          title={t("clean.entry")}
                        >
                          {t("clean.entryShort")}
                        </button>
                      )}
                    </td>
                  </tr>
                  {expanded && (
                    <tr key={`${row.id}-logs`} className="border-t border-edge/60">
                      <td colSpan={7} className="bg-bg/40 px-3 py-2">
                        <LogViewer jobId={row.id} />
                      </td>
                    </tr>
                  )}
                </Fragment>
              );
            })}
          </tbody>
        </table>
      )}
      {!history.exhausted && (
        <div className="border-t border-edge/60 p-2 text-center">
          <button
            type="button"
            disabled={history.loading}
            onClick={() => void loadHistory()}
            className="rounded-md border border-edge px-3 py-1 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent disabled:opacity-40"
          >
            {history.loading ? t("tasks.loading") : t("tasks.loadMore")}
          </button>
        </div>
      )}
    </section>
  );
}

/** SessionFinished 总结弹窗：三卡片 + 耗时/平均速度 + 失败清单重试 + 清卡入口 */
function SummaryModal({
  summary,
  onOpenClean,
}: {
  summary: JobSummary;
  onOpenClean: (jobId: number) => void;
}) {
  const { t } = useTranslation();
  const dismissSummary = useImportStore((s) => s.dismissSummary);
  const retryFailed = useImportStore((s) => s.retryFailed);
  const jobSources = useImportStore((s) => s.jobSources);
  const [retrying, setRetrying] = useState(false);
  const [retryResult, setRetryResult] = useState<number | null | "error">(null);

  // 清卡入口只对 volume/MTP 源任务显示（folder 源是本地纳管，不可清）
  const cleanable = jobSources[summary.jobId] !== undefined && jobSources[summary.jobId] !== "folder";

  async function retry(): Promise<void> {
    setRetrying(true);
    const newJobId = await retryFailed(summary.jobId);
    setRetrying(false);
    setRetryResult(newJobId === null ? "error" : newJobId);
  }

  const isMove = summary.mode === "move";
  const movedCount = summary.stats.moved ?? summary.stats.doneFiles;
  const sourceDeleteFailed = summary.stats.sourceDeleteFailed ?? 0;

  const cards = [
    {
      key: isMove ? "moved" : "done",
      value: isMove ? movedCount : summary.stats.doneFiles,
      color: "text-emerald-400",
      testid: "summary-done",
    },
    {
      key: "skipped",
      value: summary.stats.skippedDuplicates,
      color: "text-text-secondary",
      testid: "summary-skipped",
    },
    {
      key: "failed",
      value: summary.stats.failedFiles,
      color: "text-red-400",
      testid: "summary-failed",
    },
  ];

  return (
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 0.18, ease: "easeOut" }}
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
      role="dialog"
      aria-modal="true"
      aria-label={t(isMove ? "summary.titleMove" : "summary.title")}
    >
      <motion.div
        initial={{ opacity: 0, y: 12 }}
        animate={{ opacity: 1, y: 0 }}
        exit={{ opacity: 0, y: 8 }}
        transition={{ duration: 0.18, ease: "easeOut" }}
        className="max-h-[80vh] w-[460px] overflow-y-auto rounded-xl border border-edge bg-surface p-6 shadow-2xl"
        data-testid="summary-modal"
      >
        <h2 className="text-base font-semibold text-text-primary">
          {t(isMove ? "summary.titleMove" : "summary.title")}
          <span className="ml-1.5 font-mono text-xs text-text-muted">#{summary.jobId}</span>
        </h2>

        <div className="mt-4 grid grid-cols-3 gap-2">
          {cards.map((card) => (
            <div key={card.key} className="rounded-lg bg-bg px-3 py-2.5 text-center" data-testid={card.testid}>
              <div className={`text-xl font-semibold tabular-nums ${card.color}`}>{card.value}</div>
              <div className="mt-0.5 text-xs text-text-muted">{t(`summary.${card.key}`)}</div>
            </div>
          ))}
        </div>

        <p className="mt-3 text-center text-xs text-text-secondary tabular-nums">
          {t("summary.meta", {
            duration: formatDuration(summary.stats.elapsedMs),
            speed: formatSpeed(summary.stats.bytesPerSec),
            size: formatBytes(summary.stats.doneBytes),
          })}
        </p>

        {isMove && sourceDeleteFailed > 0 && (
          <p className="mt-1 text-center text-[11px] text-yellow-300" role="status" data-testid="summary-source-delete-failed">
            {t("summary.sourceDeleteFailed", { count: sourceDeleteFailed })}
          </p>
        )}

        {(summary.failures.length > 0 || summary.stats.failedFiles > 0) && (
          <div className="mt-4 rounded-lg border border-edge bg-bg p-2.5">
            <div className="flex items-center justify-between">
              <h3 className="text-xs font-medium text-text-secondary">{t("summary.failedList")}</h3>
              <button
                type="button"
                disabled={retrying || retryResult !== null}
                onClick={() => void retry()}
                className="rounded-md border border-edge px-2.5 py-1 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent disabled:opacity-40"
              >
                {t("summary.retry")}
              </button>
            </div>
            {summary.failures.length > 0 ? (
              <ul className="mt-2 max-h-32 space-y-0.5 overflow-y-auto" data-testid="summary-failures">
                {summary.failures.map((failure, index) => (
                  <li key={`${failure.src}-${index}`} className="truncate font-mono text-[11px] text-red-400" title={failure.src}>
                    {failure.src}
                  </li>
                ))}
              </ul>
            ) : (
              <p className="mt-2 text-[11px] text-text-muted">{t("summary.failedInLogs")}</p>
            )}
            {retryResult === "error" && (
              <p className="mt-2 text-[11px] text-red-400" role="alert">
                {t("summary.retryError")}
              </p>
            )}
            {typeof retryResult === "number" && (
              <p className="mt-2 text-[11px] text-emerald-400">{t("summary.retryCreated", { id: retryResult })}</p>
            )}
          </div>
        )}

        <button
          type="button"
          autoFocus
          onClick={dismissSummary}
          className="mt-5 w-full rounded-md bg-accent px-4 py-2 text-sm font-medium text-black transition-colors hover:brightness-110"
        >
          {t("summary.close")}
        </button>

        {cleanable && (
          <button
            type="button"
            onClick={() => onOpenClean(summary.jobId)}
            className="mt-2 w-full rounded-md border border-edge px-4 py-1.5 text-xs text-text-secondary transition-colors hover:border-red-400 hover:text-red-400"
            data-testid="summary-clean"
          >
            {t("clean.entry")}
          </button>
        )}
      </motion.div>
    </motion.div>
  );
}

export default function TaskCenter() {
  const { t } = useTranslation();
  const currentJobId = useImportStore((s) => s.currentJobId);
  const activeJobs = useImportStore((s) => s.activeJobs);
  const summary = useImportStore((s) => s.summary);
  const loadHistory = useImportStore((s) => s.loadHistory);
  // 清卡对话框：总结弹窗与历史行共用一个实例
  const [cleanJobId, setCleanJobId] = useState<number | null>(null);

  // 进入页面时重置加载历史首页（幂等：loading 防重入）
  useEffect(() => {
    void loadHistory(true);
  }, [loadHistory]);

  const currentJob = currentJobId !== null ? activeJobs[currentJobId] ?? null : null;
  const current =
    currentJob && (currentJob.status === "running" || currentJob.status === "paused")
      ? currentJob
      : null;

  return (
    <div className="flex h-full flex-col">
      <header className="flex h-12 shrink-0 items-center border-b border-edge px-4">
        <h1 className="text-sm font-semibold text-text-primary">{t("tasks.title")}</h1>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto p-2">
        <div className="flex flex-col gap-2">
          <IndexTaskCard />
          <CurrentJobCard job={current} />
          <HistoryTable onOpenClean={setCleanJobId} />
        </div>
      </div>
      <AnimatePresence>
        {summary && (
          <SummaryModal key={summary.jobId} summary={summary} onOpenClean={setCleanJobId} />
        )}
      </AnimatePresence>
      <CleanCardDialogLayer
        open={cleanJobId !== null}
        jobId={cleanJobId ?? 0}
        onClose={() => setCleanJobId(null)}
      />
    </div>
  );
}
