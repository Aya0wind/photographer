import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion } from "motion/react";

import { cleanApply, cleanCandidates, type CleanCandidateDto } from "@/ipc/api";
import { formatBytes } from "@/lib/format";
import { useImportStore } from "@/stores/importStore";

/**
 * 安全清卡对话框（危险操作强确认，spec 安全分级）：入口在导入完成总结
 * （全局进度卡完成态 / 任务中心总结弹窗，volume/MTP 源任务）与任务中心历史行。
 *
 * 流程：打开即调 clean_candidates 预览（文件数/总容量大数字 + 明细前 N 条滚动
 * + 指纹复验说明）→ 勾选「我确认已按规则入库」后红色危险按钮才可用 →
 * clean_apply 仅发命令，进行中/完成态由 cleanStarted/cleanFinished 事件驱动
 * （后端删除前逐文件复验指纹）；完成态展示 成功/失败/释放，失败 errors 可展开。
 */

const DETAIL_LIMIT = 8;

interface CleanCardDialogProps {
  jobId: number;
  onClose: () => void;
}

/** 对话框本体（挂载即打开态）：候选预览 → 强确认 → 事件驱动进行中/完成 */
function CleanCardDialog({ jobId, onClose }: CleanCardDialogProps) {
  const { t } = useTranslation();
  const [candidates, setCandidates] = useState<CleanCandidateDto[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [confirmed, setConfirmed] = useState(false);
  const [showErrors, setShowErrors] = useState(false);
  const clean = useImportStore((s) => s.clean);

  // 挂载即拉取候选清单（重置确认态）
  useEffect(() => {
    let cancelled = false;
    void cleanCandidates(jobId).then((list) => {
      if (cancelled) return;
      setLoading(false);
      setCandidates(list);
    });
    return () => {
      cancelled = true;
    };
  }, [jobId]);

  // 本任务的清卡状态（事件驱动）；其他任务进行中的清卡不影响本对话框
  const target = clean !== null && clean.jobId === jobId ? clean : null;
  const phase = target?.phase ?? null;
  const stats = target?.stats ?? null;
  const count = candidates?.length ?? 0;
  const totalBytes = candidates?.reduce((sum, c) => sum + c.size, 0) ?? 0;

  async function apply(): Promise<void> {
    // 只发命令：进行中/完成态由 cleanStarted/cleanFinished 事件驱动
    await cleanApply(jobId);
  }

  return (
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 0.15, ease: "easeOut" }}
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
      role="dialog"
      aria-modal="true"
      aria-label={t("clean.title")}
    >
      <motion.div
        initial={{ opacity: 0, y: 10 }}
        animate={{ opacity: 1, y: 0 }}
        exit={{ opacity: 0, y: 8 }}
        transition={{ duration: 0.15, ease: "easeOut" }}
        className="flex max-h-[80vh] w-[460px] flex-col overflow-hidden rounded-xl border border-edge bg-surface p-5 shadow-2xl"
        data-testid="clean-dialog"
      >
        <h2 className="text-sm font-semibold text-text-primary">{t("clean.title")}</h2>

        {phase === "running" && target && (
          <div className="mt-4 flex flex-col items-center gap-3 py-6" data-testid="clean-running">
            <div
              className="h-6 w-6 animate-spin rounded-full border-2 border-edge border-t-accent"
              aria-hidden="true"
            />
            <p className="text-xs text-text-secondary">
              {t("clean.running", { count: target.count, size: formatBytes(target.bytes) })}
            </p>
          </div>
        )}

        {phase === "finished" && stats && (
          <div className="mt-4 flex flex-col gap-3" data-testid="clean-finished">
            <div className="grid grid-cols-3 gap-2">
              <div className="rounded-lg bg-bg px-3 py-2.5 text-center">
                <div className="text-xl font-semibold tabular-nums text-emerald-400">
                  {stats.deleted}
                </div>
                <div className="mt-0.5 text-xs text-text-muted">{t("clean.deleted")}</div>
              </div>
              <div className="rounded-lg bg-bg px-3 py-2.5 text-center">
                <div className="text-xl font-semibold tabular-nums text-red-400">
                  {stats.failed}
                </div>
                <div className="mt-0.5 text-xs text-text-muted">{t("clean.failedCount")}</div>
              </div>
              <div className="rounded-lg bg-bg px-3 py-2.5 text-center">
                <div className="text-xl font-semibold tabular-nums text-accent">
                  {formatBytes(stats.freedBytes)}
                </div>
                <div className="mt-0.5 text-xs text-text-muted">{t("clean.freed")}</div>
              </div>
            </div>

            {stats.errors.length > 0 && (
              <div className="rounded-lg border border-edge bg-bg p-2.5">
                <button
                  type="button"
                  onClick={() => setShowErrors((v) => !v)}
                  aria-expanded={showErrors}
                  className="flex w-full items-center justify-between text-xs text-text-secondary transition-colors hover:text-text-primary"
                  data-testid="clean-errors-toggle"
                >
                  {t("clean.failedList", { count: stats.errors.length })}
                  <svg
                    viewBox="0 0 16 16"
                    width="10"
                    height="10"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="1.6"
                    className={`shrink-0 text-text-muted transition-transform ${showErrors ? "rotate-90" : ""}`}
                    aria-hidden="true"
                  >
                    <path d="M5 3l5 5-5 5" />
                  </svg>
                </button>
                {showErrors && (
                  <ul className="mt-2 max-h-32 space-y-0.5 overflow-y-auto" data-testid="clean-errors">
                    {stats.errors.map((error, index) => (
                      <li
                        key={`${error}-${index}`}
                        className="truncate font-mono text-[11px] text-red-400"
                        title={error}
                      >
                        {error}
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            )}

            <button
              type="button"
              autoFocus
              onClick={onClose}
              className="mt-1 w-full rounded-md bg-accent px-4 py-2 text-sm font-medium text-black transition-colors hover:brightness-110"
            >
              {t("common.done")}
            </button>
          </div>
        )}

        {phase === null && (
          <>
            {loading && (
              <p className="mt-4 py-6 text-center text-xs text-text-muted" data-testid="clean-loading">
                {t("clean.loading")}
              </p>
            )}

            {!loading && candidates !== null && candidates.length === 0 && (
              <div className="mt-4 flex flex-col items-center gap-3 py-4" data-testid="clean-empty">
                <p className="text-xs leading-relaxed text-text-muted">{t("clean.empty")}</p>
                <button
                  type="button"
                  onClick={onClose}
                  className="rounded-md border border-edge px-4 py-1.5 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
                >
                  {t("common.done")}
                </button>
              </div>
            )}

            {!loading && candidates !== null && candidates.length > 0 && (
              <>
                {/* 大数字：文件数 / 总容量 */}
                <div className="mt-4 grid grid-cols-2 gap-2" data-testid="clean-summary">
                  <div className="rounded-lg bg-bg px-3 py-3 text-center">
                    <div className="text-2xl font-semibold tabular-nums text-text-primary">{count}</div>
                    <div className="mt-0.5 text-xs text-text-muted">{t("clean.fileCount")}</div>
                  </div>
                  <div className="rounded-lg bg-bg px-3 py-3 text-center">
                    <div className="text-2xl font-semibold tabular-nums text-text-primary">
                      {formatBytes(totalBytes)}
                    </div>
                    <div className="mt-0.5 text-xs text-text-muted">{t("clean.totalSize")}</div>
                  </div>
                </div>

                {/* 明细（前 N 条，可滚动） */}
                <ul
                  className="sp-scroll mt-3 max-h-40 space-y-0.5 overflow-y-auto rounded-lg border border-edge bg-bg p-2"
                  data-testid="clean-list"
                >
                  {candidates.slice(0, DETAIL_LIMIT).map((candidate) => (
                    <li
                      key={candidate.src}
                      className="flex items-center justify-between gap-2"
                      title={candidate.src}
                    >
                      <span className="truncate font-mono text-[11px] text-text-secondary">
                        {candidate.src}
                      </span>
                      <span className="shrink-0 font-mono text-[11px] text-text-muted tabular-nums">
                        {formatBytes(candidate.size)}
                      </span>
                    </li>
                  ))}
                  {candidates.length > DETAIL_LIMIT && (
                    <li className="pt-1 text-center text-[11px] text-text-muted">
                      {t("clean.more", { count: candidates.length - DETAIL_LIMIT })}
                    </li>
                  )}
                </ul>

                <p className="mt-2 text-[11px] leading-relaxed text-text-muted">
                  {t("clean.verifyNote")}
                </p>

                {/* 强确认门控 */}
                <label className="mt-3 flex cursor-pointer items-center gap-2 text-xs text-text-secondary">
                  <input
                    type="checkbox"
                    checked={confirmed}
                    onChange={(e) => setConfirmed(e.target.checked)}
                    className="h-3.5 w-3.5 accent-[#F0A83C]"
                    data-testid="clean-confirm"
                  />
                  {t("clean.confirmLabel")}
                </label>

                <div className="mt-4 flex justify-end gap-2">
                  <button
                    type="button"
                    onClick={onClose}
                    className="rounded-md border border-edge px-4 py-1.5 text-xs text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
                  >
                    {t("common.cancel")}
                  </button>
                  <button
                    type="button"
                    disabled={!confirmed}
                    onClick={() => void apply()}
                    className="rounded-md bg-[#C42B1C] px-4 py-1.5 text-xs font-medium text-white transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
                    data-testid="clean-apply"
                  >
                    {t("clean.apply", { count, size: formatBytes(totalBytes) })}
                  </button>
                </div>
              </>
            )}
          </>
        )}
      </motion.div>
    </motion.div>
  );
}

/** 挂载辅助：AnimatePresence 统一包住条件渲染（退场 150ms） */
export default function CleanCardDialogLayer({
  open,
  jobId,
  onClose,
}: {
  open: boolean;
  jobId: number;
  onClose: () => void;
}) {
  return (
    <AnimatePresence>
      {open && <CleanCardDialog key={jobId} jobId={jobId} onClose={onClose} />}
    </AnimatePresence>
  );
}
