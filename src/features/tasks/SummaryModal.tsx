import { useState } from "react";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion } from "motion/react";

import { formatBytes, formatDuration, formatSpeed } from "@/lib/format";
import { useImportStore, type JobSummary } from "@/stores/importStore";
import { motionInitial, useMotionOn } from "@/lib/motion";
import CleanCardDialogLayer from "@/features/import/CleanCardDialog";

/**
 * 导入完成总结弹窗（全局挂载，M4.5 wave-3 第 8 项）：importSessionFinished 后
 * 任何页面弹出（此前在任务页，任务页已并入抽屉删除）。三卡片 + 耗时/速度 +
 * 失败清单重试 + 清卡入口（volume/MTP 源）。
 */

function SummaryModal({
  summary,
  onOpenClean,
}: {
  summary: JobSummary;
  onOpenClean: (jobId: number) => void;
}) {
  const { t } = useTranslation();
  const motionOn = useMotionOn();
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
      initial={motionInitial(motionOn, { opacity: 0 })}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 0.18, ease: "easeOut" }}
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
      role="dialog"
      aria-modal="true"
      aria-label={t(isMove ? "summary.titleMove" : "summary.title")}
    >
      <motion.div
        initial={motionInitial(motionOn, { opacity: 0, y: 12 })}
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

/** 全局挂载宿主（AppShell）：summary 存在即弹 + 清卡对话框共用实例 */
export default function SummaryModalHost() {
  const summary = useImportStore((s) => s.summary);
  const [cleanJobId, setCleanJobId] = useState<number | null>(null);

  return (
    <>
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
    </>
  );
}
