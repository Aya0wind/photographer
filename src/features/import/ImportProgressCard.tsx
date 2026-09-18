import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion } from "motion/react";

import { formatBytes, formatSpeed } from "@/lib/format";
import type { ImportMode } from "@/ipc/api";
import { useImportStore } from "@/stores/importStore";
import CleanCardDialogLayer from "@/features/import/CleanCardDialog";

/**
 * 全局导入进度卡（LR 式后台导入）：挂在 AppShell，任何页面右下角常驻。
 * 活跃任务显示标题（复制/移动 + #jobId，move 复用 jobModes）、总进度条、
 * 速度与当前文件名，快捷暂停/继续/取消；点击卡片主体跳任务中心。
 * importSessionFinished/importCancelled → 终态样式（成功/跳过/失败计数行）
 * 停留 5s 自动收起，收起前点击跳任务中心（总结弹窗由 TaskCenter 复用弹出）。
 * appError → 独立错误卡（手动收起或跳任务中心查看日志）。
 *
 * v1 单任务展示：cards 数组渲染时天然支持多条堆叠，为多任务并发预留。
 */

/** 终态卡停留时长（含 AnimatePresence 150ms 退场余量由 motion 自行处理） */
export const FINISHED_LINGER_MS = 5000;

/** 终态卡快照：summary 可能被用户在任务中心关闭，进入终态时捕获计数行数据 */
interface FinishedCard {
  jobId: number;
  kind: "done" | "cancelled";
  mode: ImportMode;
  done: number;
  skipped: number;
  failed: number;
}

/** 活跃卡（running/paused）数据；与 FinishedCard 同构，便于多任务数组化 */
interface ActiveCard {
  jobId: number;
  status: "running" | "paused";
  mode: ImportMode;
  totalFiles: number;
  doneFiles: number;
  totalBytes: number;
  doneBytes: number;
  settledBytes: number;
  bytesPerSec: number;
  currentFile: string;
}

/** 12 viewBox 手写 SVG 图标（不引入图标依赖） */
function GlyphPause({ size = 12 }: { size?: number }) {
  return (
    <svg viewBox="0 0 12 12" width={size} height={size} fill="currentColor" aria-hidden="true">
      <rect x="2.5" y="2" width="2.4" height="8" rx="0.6" />
      <rect x="7.1" y="2" width="2.4" height="8" rx="0.6" />
    </svg>
  );
}

function GlyphPlay({ size = 12 }: { size?: number }) {
  return (
    <svg viewBox="0 0 12 12" width={size} height={size} fill="currentColor" aria-hidden="true">
      <path d="M3.2 2.2v7.6a.6.6 0 0 0 .92.5l5.9-3.8a.6.6 0 0 0 0-1L4.12 1.7a.6.6 0 0 0-.92.5z" />
    </svg>
  );
}

function GlyphClose({ size = 12 }: { size?: number }) {
  return (
    <svg
      viewBox="0 0 12 12"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      aria-hidden="true"
    >
      <path d="M2.8 2.8l6.4 6.4M9.2 2.8L2.8 9.2" />
    </svg>
  );
}

function GlyphCheck({ size = 14 }: { size?: number }) {
  return (
    <svg
      viewBox="0 0 16 16"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M3.5 8.5l3 3 6-6.5" />
    </svg>
  );
}

function GlyphAlert({ size = 14 }: { size?: number }) {
  return (
    <svg
      viewBox="0 0 16 16"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M8 2L1.8 13h12.4L8 2z" />
      <path d="M8 6.5v3.2M8 11.6v.1" />
    </svg>
  );
}

/** 活跃任务卡：进度条 + 速度/当前文件 + 暂停/继续/取消快捷键 */
function ActiveCardView({ card, onClose }: { card: ActiveCard; onClose: () => void }) {
  const { t } = useTranslation();
  const pauseJob = useImportStore((s) => s.pauseJob);
  const resumeJob = useImportStore((s) => s.resumeJob);
  const cancelJob = useImportStore((s) => s.cancelJob);

  // 进度条用已结算口径：跳过的重复文件同样推进（真机：重复导入时条不动）
  const pct =
    card.totalBytes > 0
      ? Math.min(100, ((card.settledBytes ?? card.doneBytes) / card.totalBytes) * 100)
      : 0;
  const isPaused = card.status === "paused";

  return (
    <motion.div
      key={`active-${card.jobId}`}
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: 8 }}
      transition={{ duration: 0.15, ease: "easeOut" }}
      className="pointer-events-auto rounded-lg border border-edge bg-surface p-3 shadow-lg"
      data-testid="import-card"
      data-phase={isPaused ? "paused" : "running"}
    >
      {/* 标题行独立于可点击主体：关闭钮不得嵌套在 button 内（HTML 禁止嵌套交互元素） */}
      <div className="flex items-center justify-between gap-2">
        <h3 className="truncate text-xs font-semibold text-text-primary">
          {t(card.mode === "move" ? "importCard.jobMove" : "importCard.jobCopy", { id: card.jobId })}
        </h3>
        <div className="flex shrink-0 items-center gap-1.5">
          <span
            className={`rounded px-1.5 py-0.5 text-[10px] font-medium ${
              isPaused ? "bg-yellow-400/15 text-yellow-300" : "bg-accent/15 text-accent"
            }`}
          >
            {t(isPaused ? "jobStatus.paused" : "jobStatus.running")}
          </span>
          <button
            type="button"
            onClick={onClose}
            aria-label={t("common.close")}
            title={t("importCard.dismissHint")}
            data-testid="import-card-close"
            className="rounded p-0.5 text-text-muted transition-colors hover:text-text-primary"
          >
            <svg viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true">
              <path d="M4 4l8 8M12 4l-8 8" />
            </svg>
          </button>
        </div>
      </div>
      {/* 信息区：纯展示，不跳转（用户规定：卡片无需点击进任务页，任务中心有侧栏入口） */}
      <div data-testid="import-card-body" className="mt-2">
        <div
          className="h-1.5 w-full overflow-hidden rounded-full bg-panel"
          role="progressbar"
          aria-valuenow={Math.round(pct)}
          aria-valuemin={0}
          aria-valuemax={100}
          data-testid="import-card-progress"
        >
          <div
            className={`h-full rounded-full transition-[width] duration-150 ${
              isPaused ? "bg-yellow-300/70" : "bg-accent"
            }`}
            style={{ width: `${pct}%` }}
          />
        </div>

        <div className="mt-2 flex items-center justify-between gap-2 text-[11px] text-text-secondary tabular-nums">
          <span className="shrink-0 font-mono">{formatSpeed(card.bytesPerSec)}</span>
          <span className="font-mono text-text-muted">
            {formatBytes(card.doneBytes)} / {formatBytes(card.totalBytes)}
          </span>
        </div>

        <p
          className="mt-1.5 truncate font-mono text-[11px] text-text-muted"
          title={card.currentFile}
          data-testid="import-card-file"
        >
          {card.currentFile || "—"}
        </p>
      </div>

      <div className="mt-2.5 flex gap-1.5 border-t border-edge/60 pt-2.5">
        <button
          type="button"
          onClick={() => (isPaused ? void resumeJob(card.jobId) : void pauseJob(card.jobId))}
          className="flex flex-1 items-center justify-center gap-1 rounded-md border border-edge px-2 py-1 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
          data-testid="import-card-toggle"
        >
          {isPaused ? <GlyphPlay /> : <GlyphPause />}
          {t(isPaused ? "tasks.resume" : "tasks.pause")}
        </button>
        <button
          type="button"
          onClick={() => void cancelJob(card.jobId)}
          className="rounded-md border border-edge px-2.5 py-1 text-[11px] text-text-secondary transition-colors hover:border-red-400 hover:text-red-400"
          data-testid="import-card-cancel"
        >
          {t("tasks.cancel")}
        </button>
      </div>
    </motion.div>
  );
}

/** 终态卡（完成/已取消）：计数行 + 5s 自动收起；点击跳任务中心看总结 */
function FinishedCardView({ card }: { card: FinishedCard }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const jobSources = useImportStore((s) => s.jobSources);
  const [cleanOpen, setCleanOpen] = useState(false);
  const isMove = card.mode === "move";
  // 清卡入口只对 volume/MTP 源任务显示（folder 源是本地纳管，不可清）
  const cleanable = card.kind === "done" && jobSources[card.jobId] !== undefined
    ? jobSources[card.jobId] !== "folder"
    : false;

  return (
    <motion.div
      key={`finished-${card.jobId}`}
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: 8 }}
      transition={{ duration: 0.15, ease: "easeOut" }}
      className="pointer-events-auto rounded-lg border border-edge bg-surface p-3 shadow-lg"
      data-testid="import-card"
      data-phase={card.kind}
    >
      <button
        type="button"
        onClick={() => navigate("/tasks")}
        className="block w-full text-left"
        data-testid="import-card-body"
      >
        <div className="flex items-center gap-2">
          {card.kind === "done" ? (
            <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-emerald-400/15 text-emerald-400">
              <GlyphCheck />
            </span>
          ) : (
            <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-panel text-text-muted">
              <GlyphClose />
            </span>
          )}
          <h3 className="truncate text-xs font-semibold text-text-primary">
            {card.kind === "done"
              ? t(isMove ? "summary.titleMove" : "summary.title")
              : t("jobStatus.cancelled")}
            <span className="ml-1 font-mono text-[10px] font-normal text-text-muted">#{card.jobId}</span>
          </h3>
        </div>
        {card.kind === "done" && (
          <p className="mt-2 text-[11px] text-text-secondary tabular-nums" data-testid="import-card-counts">
            {t(isMove ? "importCard.countRowMove" : "importCard.countRow", {
              done: card.done,
              skipped: card.skipped,
              failed: card.failed,
            })}
          </p>
        )}
        <p className="mt-1.5 text-[10px] text-text-muted">{t("importCard.viewSummary")}</p>
      </button>

      {cleanable && (
        <div className="mt-2.5 border-t border-edge/60 pt-2.5">
          <button
            type="button"
            onClick={() => setCleanOpen(true)}
            className="w-full rounded-md border border-edge px-2 py-1 text-[11px] text-text-secondary transition-colors hover:border-red-400 hover:text-red-400"
            data-testid="import-card-clean"
          >
            {t("clean.entry")}
          </button>
        </div>
      )}
      <CleanCardDialogLayer open={cleanOpen} jobId={card.jobId} onClose={() => setCleanOpen(false)} />
    </motion.div>
  );
}

/** 错误卡：appError 消息；「查看任务」跳任务中心查日志，× 手动收起 */
function ErrorCardView({ message, onDismiss }: { message: string; onDismiss: () => void }) {
  const { t } = useTranslation();
  const navigate = useNavigate();

  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: 8 }}
      transition={{ duration: 0.15, ease: "easeOut" }}
      className="pointer-events-auto rounded-lg border border-red-400/40 bg-surface p-3 shadow-lg"
      data-testid="import-card-error"
      data-phase="error"
      role="alert"
    >
      <div className="flex items-center gap-2">
        <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-red-400/15 text-red-400">
          <GlyphAlert />
        </span>
        <h3 className="min-w-0 flex-1 truncate text-xs font-semibold text-red-400">
          {t("importCard.errorTitle")}
        </h3>
      </div>
      <p className="mt-1.5 line-clamp-2 break-all text-[11px] text-text-secondary" title={message}>
        {message}
      </p>
      <div className="mt-2.5 flex gap-1.5 border-t border-edge/60 pt-2.5">
        <button
          type="button"
          onClick={() => navigate("/tasks")}
          className="flex-1 rounded-md border border-edge px-2 py-1 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
        >
          {t("importCard.viewTasks")}
        </button>
        <button
          type="button"
          onClick={onDismiss}
          aria-label={t("importCard.dismiss")}
          className="flex items-center justify-center rounded-md border border-edge px-2 py-1 text-text-secondary transition-colors hover:border-red-400 hover:text-red-400"
        >
          <GlyphClose />
        </button>
      </div>
    </motion.div>
  );
}

export default function ImportProgressCard() {
  const currentJobId = useImportStore((s) => s.currentJobId);
  const activeJobs = useImportStore((s) => s.activeJobs);
  const jobModes = useImportStore((s) => s.jobModes);
  const summary = useImportStore((s) => s.summary);
  const lastError = useImportStore((s) => s.lastError);

  // 终态快照：任务进入 done/cancelled 时捕获（summary 同批写入；被关闭后仍可展示）
  const [finished, setFinished] = useState<FinishedCard | null>(null);
  // 已捕获过终态快照的任务 id：后续 store 变化（如用户关闭总结弹窗）不重复弹卡
  const capturedRef = useRef<number | null>(null);
  // 用户手动收起的任务 id：导入继续后台执行，卡片不再打扰（新任务重新出现）
  // state（非 ref）：收起即时重渲染，暂停态（无进度事件）也能立即消失
  const [dismissedJobs, setDismissedJobs] = useState<Set<number>>(() => new Set());

  useEffect(() => {
    const job = currentJobId !== null ? activeJobs[currentJobId] ?? null : null;
    if (!job || (job.status !== "done" && job.status !== "cancelled")) return;
    if (capturedRef.current === job.jobId) return;
    capturedRef.current = job.jobId;
    const stats = summary && summary.jobId === job.jobId ? summary.stats : null;
    setFinished({
      jobId: job.jobId,
      kind: job.status,
      mode: jobModes[job.jobId] ?? "copy",
      done: stats ? stats.doneFiles : job.doneFiles,
      skipped: stats ? stats.skippedDuplicates : 0,
      failed: stats ? stats.failedFiles : 0,
    });
  }, [activeJobs, currentJobId, summary, jobModes]);

  // 5s 自动收起（fake timers 可直接推进）
  useEffect(() => {
    if (!finished) return;
    const timer = setTimeout(() => setFinished(null), FINISHED_LINGER_MS);
    return () => clearTimeout(timer);
  }, [finished]);

  // 错误卡：仅展示“新到达”的错误（挂载时已存在的旧错误不再弹出）
  const [dismissedError, setDismissedError] = useState(lastError);
  const showError = lastError !== null && lastError !== dismissedError;

  const job = currentJobId !== null ? activeJobs[currentJobId] ?? null : null;

  // v1 单任务：活跃卡优先；终态卡仅在无活跃任务时停留展示（多任务并发时改为数组堆叠）
  const cards: Array<{ type: "active"; card: ActiveCard } | { type: "finished"; card: FinishedCard }> = [];
  if (job && (job.status === "running" || job.status === "paused") && !dismissedJobs.has(job.jobId)) {
    cards.push({
      type: "active",
      card: {
        jobId: job.jobId,
        status: job.status,
        mode: jobModes[job.jobId] ?? "copy",
        totalFiles: job.totalFiles,
        doneFiles: job.doneFiles,
        totalBytes: job.totalBytes,
        doneBytes: job.doneBytes,
        settledBytes: job.settledBytes ?? job.doneBytes,
        bytesPerSec: job.bytesPerSec,
        currentFile: job.currentFile,
      },
    });
  } else if (finished) {
    cards.push({ type: "finished", card: finished });
  }

  return (
    <div className="pointer-events-none fixed bottom-4 right-4 z-40 flex w-[280px] flex-col gap-2">
      <AnimatePresence>
        {showError && lastError && (
          <ErrorCardView key="error" message={lastError.message} onDismiss={() => setDismissedError(lastError)} />
        )}
        {cards.map((entry) =>
          entry.type === "active" ? (
            <ActiveCardView
              key={`active-${entry.card.jobId}`}
              card={entry.card}
              onClose={() => {
                setDismissedJobs((prev) => new Set(prev).add(entry.card.jobId));
                setFinished(null);
              }}
            />
          ) : (
            <FinishedCardView key={`finished-${entry.card.jobId}`} card={entry.card} />
          ),
        )}
      </AnimatePresence>
    </div>
  );
}
