import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { cullSessionFinish, type CullFinishResult, type CullSessionDto } from "@/ipc/api";
import { buildFinishApply, type CullFinishConfig, type CullProgress } from "../lib/cullingCore";

/**
 * 选片收尾映射弹窗（方案 §4 出口，V1 三开关）：
 * - 已选 → 旗标 P（acceptedFlag）
 * - 已选 → 星级 1-5（acceptedRating；null=不应用）
 * - 已剔除 → 拒绝 X（rejectRejected）
 * 确认 → cullSessionFinish；结果经 onApplied 回传页面出摘要 toast。
 * 「加入子组 / 写 XMP / 导出清单」为后续阶段出口，本版不做。
 */

/** 默认建议：旗标 + 拒绝开（最常用出口），星级关 */
export const DEFAULT_FINISH_CONFIG: CullFinishConfig = {
  acceptedFlag: true,
  acceptedRating: null,
  rejectRejected: true,
};

function SwitchRow({
  checked,
  disabled,
  label,
  testId,
  onToggle,
  children,
}: {
  checked: boolean;
  disabled?: boolean;
  label: string;
  testId: string;
  onToggle: (next: boolean) => void;
  children?: React.ReactNode;
}) {
  return (
    <div
      className="flex items-center gap-3 rounded-lg border border-edge bg-panel/40 px-3 py-2.5"
      data-testid={testId}
      data-checked={checked}
    >
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        disabled={disabled}
        onClick={() => onToggle(!checked)}
        className={`relative h-5 w-9 shrink-0 rounded-full transition-colors disabled:opacity-40 ${
          checked ? "bg-accent" : "bg-edge"
        }`}
        aria-label={label}
        data-testid={`${testId}-switch`}
      >
        <span
          className={`absolute top-0.5 h-4 w-4 rounded-full bg-white shadow transition-all ${
            checked ? "left-[18px]" : "left-0.5"
          }`}
        />
      </button>
      <div className="min-w-0 flex-1">{children ?? <span className="text-xs">{label}</span>}</div>
    </div>
  );
}

export interface CullFinishDialogProps {
  session: CullSessionDto;
  progress: CullProgress;
  onClose: () => void;
  onApplied: (result: CullFinishResult) => void;
  /** finish 失败（后端未连接/命令暂不可用）回调（页面 toast） */
  onFailed: () => void;
}

export default function CullFinishDialog({
  session,
  progress,
  onClose,
  onApplied,
  onFailed,
}: CullFinishDialogProps) {
  const { t } = useTranslation();
  const [config, setConfig] = useState<CullFinishConfig>(DEFAULT_FINISH_CONFIG);
  const [busy, setBusy] = useState(false);

  // Esc 关弹窗（浮层键盘流此时让位）
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        if (!busy) onClose();
      }
    };
    window.addEventListener("keydown", onKey, { capture: true });
    return () => window.removeEventListener("keydown", onKey, { capture: true });
  }, [busy, onClose]);

  function patch(next: Partial<CullFinishConfig>): void {
    setConfig((prev) => ({ ...prev, ...next }));
  }

  async function confirm(): Promise<void> {
    if (busy) return;
    setBusy(true);
    const result = await cullSessionFinish(session.id, buildFinishApply(config));
    setBusy(false);
    if (result === null) {
      onFailed();
      return;
    }
    onApplied(result);
  }

  const anyExport = config.acceptedFlag || config.acceptedRating !== null || config.rejectRejected;

  return (
    <div
      className="fixed inset-0 z-[80] flex items-center justify-center bg-black/55 p-6"
      role="dialog"
      aria-modal="true"
      aria-label={t("culling.finish.title")}
      onClick={(e) => {
        if (e.target === e.currentTarget && !busy) onClose();
      }}
      data-testid="cull-finish-overlay"
    >
      <div
        className="w-[420px] overflow-hidden rounded-xl border border-edge bg-surface shadow-2xl"
        onClick={(e) => e.stopPropagation()}
        data-testid="cull-finish-dialog"
      >
        <div className="border-b border-edge px-4 py-3">
          <h2 className="text-sm font-semibold text-text-primary" data-testid="cull-finish-title">
            {t("culling.finish.title")}
          </h2>
          <p className="mt-1 text-xs text-text-muted" data-testid="cull-finish-summary">
            {t("culling.finish.summary", {
              name: session.name,
              accepted: progress.accepted,
              rejected: progress.rejected,
              undecided: progress.undecided,
            })}
          </p>
        </div>

        <div className="space-y-2 p-4" data-testid="cull-finish-options">
          <SwitchRow
            checked={config.acceptedFlag}
            label={t("culling.finish.toFlag")}
            testId="cull-finish-flag"
            onToggle={(v) => patch({ acceptedFlag: v })}
          >
            <span className="block text-xs text-text-primary">
              {t("culling.finish.toFlag")}
              <span className="ml-1.5 font-mono text-[10px] text-text-muted">
                {t("culling.finish.acceptedCount", { count: progress.accepted })}
              </span>
            </span>
          </SwitchRow>

          <SwitchRow
            checked={config.acceptedRating !== null}
            label={t("culling.finish.toRating")}
            testId="cull-finish-rating"
            onToggle={(v) => patch({ acceptedRating: v ? (config.acceptedRating ?? 3) : null })}
          >
            <span className="block text-xs text-text-primary">
              {t("culling.finish.toRating")}
              <span className="ml-1.5 font-mono text-[10px] text-text-muted">
                {t("culling.finish.acceptedCount", { count: progress.accepted })}
              </span>
            </span>
            <span className="mt-1.5 flex items-center gap-1" data-testid="cull-finish-rating-picker">
              {[1, 2, 3, 4, 5].map((value) => (
                <button
                  key={value}
                  type="button"
                  disabled={config.acceptedRating === null}
                  aria-pressed={config.acceptedRating === value}
                  aria-label={`${t("culling.finish.toRating")} ${value}`}
                  onClick={() => patch({ acceptedRating: value })}
                  className={`rounded p-0.5 transition-colors disabled:opacity-30 ${
                    config.acceptedRating !== null && value <= config.acceptedRating
                      ? "text-accent"
                      : "text-text-muted hover:text-text-secondary"
                  }`}
                  data-testid="cull-finish-rating-star"
                  data-value={value}
                >
                  <svg
                    viewBox="0 0 16 16"
                    width="16"
                    height="16"
                    fill={config.acceptedRating !== null && value <= config.acceptedRating ? "currentColor" : "none"}
                    stroke="currentColor"
                    strokeWidth="1.4"
                    aria-hidden="true"
                  >
                    <path d="M8 1.8l1.8 3.7 4 .6-2.9 2.8.7 4L8 11l-3.6 1.9.7-4L2.2 6.1l4-.6z" />
                  </svg>
                </button>
              ))}
            </span>
          </SwitchRow>

          <SwitchRow
            checked={config.rejectRejected}
            label={t("culling.finish.toReject")}
            testId="cull-finish-reject"
            onToggle={(v) => patch({ rejectRejected: v })}
          >
            <span className="block text-xs text-text-primary">
              {t("culling.finish.toReject")}
              <span className="ml-1.5 font-mono text-[10px] text-text-muted">
                {t("culling.finish.rejectedCount", { count: progress.rejected })}
              </span>
            </span>
          </SwitchRow>

          <p className="pt-1 text-[11px] leading-relaxed text-text-muted" data-testid="cull-finish-hint">
            {t("culling.finish.hint")}
          </p>
        </div>

        <div className="flex items-center justify-end gap-2 border-t border-edge px-4 py-3">
          <button
            type="button"
            onClick={onClose}
            disabled={busy}
            className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:bg-panel hover:text-text-primary disabled:opacity-40"
            data-testid="cull-finish-cancel"
          >
            {t("common.cancel")}
          </button>
          <button
            type="button"
            onClick={() => void confirm()}
            disabled={busy || !anyExport}
            className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="cull-finish-confirm"
            data-busy={busy}
          >
            {busy ? t("culling.finish.busy") : t("culling.finish.apply")}
          </button>
        </div>
      </div>
    </div>
  );
}
