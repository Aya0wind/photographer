import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { cullAiPrescan, type CullPrescanDto, type CullSessionDto } from "@/ipc/api";
import { buildAiRules, type CullAiRulesForm } from "../lib/cullingCore";

/**
 * AI 挑图弹窗（V3，方案 §3.3）：规则区 → 预览摘要 → 应用建议。
 * - 规则：闭眼/失焦检测（开关 + 弱/标准/强三档）· 连拍组自动留最锐 ·
 *   豁免-合影超 N 人（0=关）· 精选张数上限（空=不限）
 * - 预览（apply=false）只出摘要计数；应用（apply=true）写入会话为
 *   origin='ai' 预标记——仅作用于未定项，过片时可随时翻转（AI 只建议纪律）
 * - 表单态 → 契约 DTO 由 cullingCore.buildAiRules 规整（脏值兜底）
 * - 预览/应用同接口同形（cull_ai_prescan）；失败提示后端未连接，不动会话
 */

/** 默认规则：闭眼/失焦开（标准档）——最常用的剔除信号；连拍留最锐关（更激进） */
export const DEFAULT_AI_FORM: CullAiRulesForm = {
  eyesEnabled: true,
  eyesSensitivity: "normal",
  blurEnabled: true,
  blurSensitivity: "normal",
  burstKeepSharpest: false,
  groupExemptFaces: "0",
  maxAccepted: "",
};

const SENSITIVITIES: Array<{ value: "weak" | "normal" | "strong"; key: string }> = [
  { value: "weak", key: "culling.ai.sensWeak" },
  { value: "normal", key: "culling.ai.sensNormal" },
  { value: "strong", key: "culling.ai.sensStrong" },
];

function SwitchRow({
  checked,
  label,
  testId,
  onToggle,
  children,
}: {
  checked: boolean;
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
        onClick={() => onToggle(!checked)}
        className={`relative h-5 w-9 shrink-0 rounded-full transition-colors ${
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

/** 敏感度三档分段选择（弱/标准/强） */
function SensitivityPicker({
  value,
  disabled,
  testId,
  onChange,
}: {
  value: string;
  disabled: boolean;
  testId: string;
  onChange: (next: "weak" | "normal" | "strong") => void;
}) {
  const { t } = useTranslation();
  return (
    <div
      className={`flex overflow-hidden rounded-md border border-edge ${disabled ? "opacity-40" : ""}`}
      data-testid={testId}
    >
      {SENSITIVITIES.map((option) => (
        <button
          key={option.value}
          type="button"
          disabled={disabled}
          onClick={() => onChange(option.value)}
          aria-pressed={value === option.value}
          className={`px-2 py-0.5 text-[11px] transition-colors ${
            value === option.value
              ? "bg-accent font-medium text-black"
              : "text-text-secondary hover:bg-panel hover:text-text-primary"
          }`}
          data-testid={`${testId}-opt`}
          data-value={option.value}
          data-active={value === option.value}
        >
          {t(option.key)}
        </button>
      ))}
    </div>
  );
}

export interface CullAiDialogProps {
  session: CullSessionDto;
  onClose: () => void;
  /** 应用成功回传（浮层重开决定表 + toast）；预览不触发 */
  onApplied: (result: CullPrescanDto) => void;
}

export default function CullAiDialog({ session, onClose, onApplied }: CullAiDialogProps) {
  const { t } = useTranslation();
  const [form, setForm] = useState<CullAiRulesForm>(DEFAULT_AI_FORM);
  const [preview, setPreview] = useState<CullPrescanDto | null>(null);
  const [busy, setBusy] = useState<"none" | "preview" | "apply">("none");
  const [failed, setFailed] = useState(false);

  function patch(next: Partial<CullAiRulesForm>): void {
    setForm((prev) => ({ ...prev, ...next }));
    setPreview(null); // 规则一改，旧摘要作废（防误应用到另一套规则）
    setFailed(false);
  }

  // Esc 关弹窗（浮层键盘流此时让位）
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        if (busy === "none") onClose();
      }
    };
    window.addEventListener("keydown", onKey, { capture: true });
    return () => window.removeEventListener("keydown", onKey, { capture: true });
  }, [busy, onClose]);

  async function runPreview(): Promise<void> {
    if (busy !== "none") return;
    setBusy("preview");
    setFailed(false);
    const result = await cullAiPrescan(session.id, buildAiRules(form), false);
    setBusy("none");
    if (result === null) {
      setFailed(true);
      return;
    }
    setPreview(result);
  }

  async function runApply(): Promise<void> {
    if (busy !== "none" || preview === null) return;
    setBusy("apply");
    setFailed(false);
    const result = await cullAiPrescan(session.id, buildAiRules(form), true);
    setBusy("none");
    if (result === null) {
      setFailed(true);
      return;
    }
    onApplied(result);
  }

  return (
    <div
      className="fixed inset-0 z-[80] flex items-center justify-center bg-black/55 p-6"
      role="dialog"
      aria-modal="true"
      aria-label={t("culling.ai.title")}
      onClick={(e) => {
        if (e.target === e.currentTarget && busy === "none") onClose();
      }}
      data-testid="cull-ai-overlay"
    >
      <div
        className="flex max-h-[86vh] w-[460px] flex-col overflow-hidden rounded-xl border border-edge bg-surface shadow-2xl"
        onClick={(e) => e.stopPropagation()}
        data-testid="cull-ai-dialog"
      >
        <div className="border-b border-edge px-4 py-3">
          <h2 className="text-sm font-semibold text-text-primary" data-testid="cull-ai-title">
            {t("culling.ai.title")}
          </h2>
          <p className="mt-1 text-xs text-text-muted" data-testid="cull-ai-subtitle">
            {t("culling.ai.subtitle", { name: session.name })}
          </p>
        </div>

        <div className="space-y-2 overflow-y-auto p-4" data-testid="cull-ai-rules">
          <SwitchRow
            checked={form.eyesEnabled}
            label={t("culling.ai.eyes")}
            testId="cull-ai-eyes"
            onToggle={(v) => patch({ eyesEnabled: v })}
          >
            <div className="flex items-center justify-between gap-2">
              <span className="text-xs text-text-primary">{t("culling.ai.eyes")}</span>
              <SensitivityPicker
                value={form.eyesSensitivity}
                disabled={!form.eyesEnabled}
                testId="cull-ai-eyes-sens"
                onChange={(v) => patch({ eyesSensitivity: v })}
              />
            </div>
          </SwitchRow>

          <SwitchRow
            checked={form.blurEnabled}
            label={t("culling.ai.blur")}
            testId="cull-ai-blur"
            onToggle={(v) => patch({ blurEnabled: v })}
          >
            <div className="flex items-center justify-between gap-2">
              <span className="text-xs text-text-primary">{t("culling.ai.blur")}</span>
              <SensitivityPicker
                value={form.blurSensitivity}
                disabled={!form.blurEnabled}
                testId="cull-ai-blur-sens"
                onChange={(v) => patch({ blurSensitivity: v })}
              />
            </div>
          </SwitchRow>

          <SwitchRow
            checked={form.burstKeepSharpest}
            label={t("culling.ai.burstKeep")}
            testId="cull-ai-burst"
            onToggle={(v) => patch({ burstKeepSharpest: v })}
          />

          {/* 豁免：合影超 N 人（0=关） */}
          <div
            className="flex items-center gap-3 rounded-lg border border-edge bg-panel/40 px-3 py-2.5"
            data-testid="cull-ai-exempt"
          >
            <span className="min-w-0 flex-1 text-xs text-text-primary">
              {t("culling.ai.groupExempt")}
            </span>
            <input
              type="number"
              min={0}
              step={1}
              value={form.groupExemptFaces}
              onChange={(e) => patch({ groupExemptFaces: e.target.value })}
              aria-label={t("culling.ai.groupExempt")}
              className="h-7 w-16 rounded-md border border-edge bg-bg px-2 text-right font-mono text-xs tabular-nums text-text-primary outline-none transition-colors focus:border-accent"
              data-testid="cull-ai-exempt-input"
            />
          </div>

          {/* 精选张数上限（空=不限） */}
          <div
            className="flex items-center gap-3 rounded-lg border border-edge bg-panel/40 px-3 py-2.5"
            data-testid="cull-ai-max"
          >
            <span className="min-w-0 flex-1 text-xs text-text-primary">
              {t("culling.ai.maxAccepted")}
            </span>
            <input
              type="number"
              min={1}
              step={1}
              placeholder={t("culling.ai.maxPlaceholder")}
              value={form.maxAccepted}
              onChange={(e) => patch({ maxAccepted: e.target.value })}
              aria-label={t("culling.ai.maxAccepted")}
              className="h-7 w-16 rounded-md border border-edge bg-bg px-2 text-right font-mono text-xs tabular-nums text-text-primary outline-none transition-colors placeholder:font-sans placeholder:text-text-muted focus:border-accent"
              data-testid="cull-ai-max-input"
            />
          </div>

          <p className="pt-1 text-[11px] leading-relaxed text-text-muted" data-testid="cull-ai-hint">
            {t("culling.ai.hint")}
          </p>
        </div>

        {/* 预览摘要 / 失败提示 */}
        {preview !== null && (
          <div
            className="mx-4 mb-3 rounded-lg border border-edge bg-panel/40 px-3 py-2.5"
            data-testid="cull-ai-summary"
            data-accepted={preview.suggestedAccepted}
            data-rejected={preview.suggestedRejected}
            data-skipped={preview.skippedManual}
            data-exempted={preview.exemptedGroup}
          >
            <p className="text-xs leading-relaxed text-text-secondary">
              {t("culling.ai.summary", {
                rejected: preview.suggestedRejected,
                accepted: preview.suggestedAccepted,
                exempted: preview.exemptedGroup,
                skipped: preview.skippedManual,
              })}
            </p>
          </div>
        )}
        {failed && (
          <p
            className="mx-4 mb-3 text-xs text-red-400"
            role="alert"
            data-testid="cull-ai-failed"
          >
            {t("culling.ai.failed")}
          </p>
        )}

        <div className="flex items-center justify-end gap-2 border-t border-edge px-4 py-3">
          <button
            type="button"
            onClick={onClose}
            disabled={busy !== "none"}
            className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:bg-panel hover:text-text-primary disabled:opacity-40"
            data-testid="cull-ai-cancel"
          >
            {t("common.cancel")}
          </button>
          <button
            type="button"
            onClick={() => void runPreview()}
            disabled={busy !== "none"}
            className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="cull-ai-preview"
            data-busy={busy === "preview"}
          >
            {busy === "preview" ? t("culling.ai.previewBusy") : t("culling.ai.preview")}
          </button>
          <button
            type="button"
            onClick={() => void runApply()}
            disabled={busy !== "none" || preview === null}
            title={t("culling.ai.applyHint")}
            className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="cull-ai-apply"
            data-busy={busy === "apply"}
            data-ready={preview !== null}
          >
            {busy === "apply" ? t("culling.ai.applyBusy") : t("culling.ai.apply")}
          </button>
        </div>
        {/* 确认提示：应用语义（仅未定项 · 可随时翻转） */}
        {preview !== null && (
          <p
            className="border-t border-edge/60 px-4 py-2 text-center text-[10px] text-text-muted"
            data-testid="cull-ai-apply-hint"
          >
            {t("culling.ai.applyHint")}
          </p>
        )}
      </div>
    </div>
  );
}
