import { useEffect, useState } from "react";
import { useNavigate } from "react-router";
import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import TitleBar from "@/app/shell/TitleBar";
import { useSettingsStore } from "@/stores/settingsStore";
import { useAiStore } from "@/stores/aiStore";
import {
  QUALITY_TIERS,
  gapsForIds,
  tierFaceIds,
  tierSemanticIds,
  type QualityTier,
} from "@/features/settings/lib/qualityTier";
import { useWindowReveal } from "@/lib/windowReveal";
import { isMacPlatform } from "@/lib/platform";
import { databaseCreate } from "@/ipc/api";
import AiDownloadPanel from "../steps/AiDownloadPanel";

/**
 * 首次引导（2026-10-10 定案）：
 * 1. 创建数据库——名称必填 + 位置可选（缺省 = 后端约定路径
 *    `<应用配置目录>/databases/<id>`）；无论注册表状态恒渲染创建表单
 *    （databaseCreate 多库合法，创建即激活），之后一切库内操作作用于它。
 * 2. AI 功能（老引导回归）：开关两卡 + 模型档位三选；「下载并继续」在本页
 *    下载所选档位的模型（缺口补齐，已就绪复用），可「稍后手动下载」跳过。
 * 3. 完成——写 onboardingCompleted 兼容位并进画廊。照片库不再在引导中
 *    建立，用户之后在「存储」页自建（新建 / 从已有文件夹建立）。
 * 门禁在 GatedShell（老语义恢复）：本会话未选数据库或激活数据库无效 →
 * /database-picker（选择页）；首启无数据库由选择页直送本引导。
 */

type OnboardingStep = "welcome" | "ai" | "done";

const FIELD_CLASS =
  "w-full rounded-md border border-edge bg-bg px-2.5 py-1.5 font-mono text-xs text-text-primary outline-none transition-colors focus:border-accent";

/** 从绝对路径取末段作为缺省库名（Windows/Unix 分隔符都认；取不到回退全路径） */
function baseName(path: string): string {
  const parts = path.split(/[\\/]+/).filter((s) => s.length > 0);
  return parts.length > 0 ? parts[parts.length - 1] : path;
}

/** 第一步（创建数据库）子组件：恒渲染创建表单（多库合法，创建即激活） */
function DatabaseStep({ onCreated, onNext }: { onCreated: () => void; onNext: () => void }) {
  const { t } = useTranslation();
  const [name, setName] = useState("");
  const [dbDir, setDbDir] = useState("");
  /** 名称未被手改过时，选完文件夹自动带出末段作缺省名 */
  const [nameTouched, setNameTouched] = useState(false);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const ready = name.trim().length > 0 && !creating;

  async function pickDirectory(): Promise<void> {
    try {
      const dir = await openDialog({
        directory: true,
        defaultPath: dbDir.trim() || undefined,
      });
      if (typeof dir === "string" && dir.length > 0) {
        setDbDir(dir);
        if (!nameTouched && name.trim().length === 0) setName(baseName(dir));
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function create(): Promise<void> {
    if (!ready) return;
    setCreating(true);
    setError(null);
    const result = await databaseCreate(name.trim(), dbDir.trim() || undefined);
    setCreating(false);
    if (result.ok) {
      onCreated();
      onNext();
      return;
    }
    // null=invoke 不可用（后端未连接）；字符串=后端业务 Err 文案透传
    setError(
      result.error === null ? t("onboarding.dbCreate.unavailable") : result.error,
    );
  }

  return (
    <>
      <h1 className="text-xl font-semibold">{t("onboarding.title")}</h1>
      <p className="text-sm leading-relaxed text-text-secondary">
        {t("onboarding.dbCreate.desc")}
      </p>

      <label className="mt-1 flex flex-col gap-1 text-xs text-text-secondary">
        {t("onboarding.dbCreate.name")}
        <input
          type="text"
          value={name}
          onChange={(e) => {
            setName(e.target.value);
            setNameTouched(true);
          }}
          className={FIELD_CLASS}
          autoFocus
          data-testid="onboarding-db-name"
        />
      </label>

      <label className="mt-2 flex flex-col gap-1 text-xs text-text-secondary">
        {t("onboarding.dbCreate.location")}
        <div className="flex gap-2">
          <input
            type="text"
            value={dbDir}
            onChange={(e) => setDbDir(e.target.value)}
            className={FIELD_CLASS}
            data-testid="onboarding-db-location"
          />
          <button
            type="button"
            onClick={() => void pickDirectory()}
            className="shrink-0 rounded-md border border-edge px-3 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
            data-testid="onboarding-db-browse"
          >
            {t("onboarding.dbCreate.browse")}
          </button>
        </div>
        <span className="text-[11px] leading-relaxed text-text-muted">
          {t("onboarding.dbCreate.locationHint")}
        </span>
      </label>

      {error !== null && (
        <p className="mt-2 text-[11px] text-red-400" role="alert" data-testid="onboarding-db-error">
          {error}
        </p>
      )}

      <div className="mt-2 flex justify-end">
        <button
          type="button"
          disabled={!ready}
          onClick={() => void create()}
          className="rounded-md bg-accent px-5 py-2 text-sm font-medium text-black transition-opacity hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-40"
          data-testid="onboarding-db-create"
        >
          {creating ? t("onboarding.dbCreate.creating") : t("onboarding.dbCreate.create")}
        </button>
      </div>
    </>
  );
}

/**
 * 第二步（AI 功能）子组件：开关两卡 + 档位三选 + 页内下载。
 * - 「下载并继续」：先落设置（三开关全开 + qualityTier），再在本页下载所选
 *   档位的语义/人脸模型（已就绪资源复用），全部就绪自动进下一步；
 * - 「稍后手动下载」：落设置直接进下一步（逃生门——设置页手动下载）；
 * - 「暂不启用」：三开关全关进下一步。
 */
function AiStep({ onNext }: { onNext: () => void }) {
  const { t } = useTranslation();
  const models = useAiStore((s) => s.models);
  const [enabled, setEnabled] = useState(true);
  const [tier, setTier] = useState<QualityTier>(
    () => (useSettingsStore.getState().settings.ai.qualityTier as QualityTier) ?? "normal",
  );
  const [downloading, setDownloading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const requiredIds = enabled ? [...tierSemanticIds(tier), ...tierFaceIds(tier)] : [];
  const ready = requiredIds.length === 0 || gapsForIds(requiredIds, models).length === 0;

  // 下载中就绪 → 自动进下一步（含「本来就全就绪」的瞬时路径）
  useEffect(() => {
    if (downloading && ready) onNext();
  }, [downloading, ready, onNext]);

  async function persist(next: boolean, tierToSave: QualityTier): Promise<void> {
    const current = useSettingsStore.getState().settings;
    await useSettingsStore.getState().save({
      ...current,
      ai: {
        ...current.ai,
        enableClip: next,
        enableFace: next,
        enableSceneTags: next,
        qualityTier: tierToSave,
      },
    });
  }

  async function downloadAndContinue(): Promise<void> {
    if (saving || downloading) return;
    setSaving(true);
    setError(null);
    try {
      await persist(true, tier);
    } catch (e) {
      setError(String(e).replace(/^Error:\s*/, ""));
      return;
    } finally {
      setSaving(false);
    }
    // 设置已落盘；下载中途关应用不丢选择，重进引导补缺口
    setDownloading(true);
  }

  async function manualLater(): Promise<void> {
    if (saving || downloading) return;
    setSaving(true);
    setError(null);
    try {
      await persist(true, tier);
      onNext();
    } catch (e) {
      setError(String(e).replace(/^Error:\s*/, ""));
    } finally {
      setSaving(false);
    }
  }

  async function continueDisabled(): Promise<void> {
    if (saving || downloading) return;
    setSaving(true);
    setError(null);
    try {
      await persist(false, tier);
      onNext();
    } catch (e) {
      setError(String(e).replace(/^Error:\s*/, ""));
    } finally {
      setSaving(false);
    }
  }

  return (
    <>
      <h1 className="text-xl font-semibold">{t("onboarding.ai.title")}</h1>
      <p className="text-sm leading-relaxed text-text-secondary">
        {t("onboarding.ai.desc")}
      </p>

      <div className="mt-1 grid grid-cols-2 gap-2" data-testid="onboarding-ai-modes">
        {([true, false] as const).map((value) => (
          <button
            key={String(value)}
            type="button"
            onClick={() => setEnabled(value)}
            disabled={downloading}
            aria-pressed={enabled === value}
            className={`rounded-lg border p-3 text-left transition-colors ${
              enabled === value
                ? "border-accent bg-accent/10"
                : "border-edge hover:border-text-muted"
            }`}
            data-testid={`onboarding-ai-${value ? "enable" : "disable"}`}
            data-selected={enabled === value}
          >
            <span
              className={`text-sm font-medium ${enabled === value ? "text-accent" : "text-text-primary"}`}
            >
              {t(value ? "onboarding.ai.enable" : "onboarding.ai.disable")}
            </span>
            <p className="mt-1 text-xs leading-relaxed text-text-secondary">
              {t(value ? "onboarding.ai.enableDesc" : "onboarding.ai.disableDesc")}
            </p>
          </button>
        ))}
      </div>

      {enabled && (
        <fieldset className="mt-1 flex flex-col gap-2">
          <legend className="text-xs text-text-secondary">{t("onboarding.ai.qualityTier")}</legend>
          <div className="grid grid-cols-3 gap-2" data-testid="onboarding-ai-tiers">
            {QUALITY_TIERS.map((option) => (
              <label
                key={option}
                className={`cursor-pointer rounded-lg border p-2.5 ${
                  tier === option ? "border-accent bg-accent/5" : "border-edge hover:border-text-muted"
                }`}
                data-testid={`onboarding-ai-tier-${option}`}
                data-selected={tier === option}
              >
                <span className="flex items-center gap-2 text-xs font-medium text-text-primary">
                  <input
                    type="radio"
                    name="onboarding-ai-tier"
                    value={option}
                    checked={tier === option}
                    onChange={() => setTier(option)}
                    disabled={downloading}
                    className="h-3.5 w-3.5 accent-[#F0A83C]"
                  />
                  {t(`settings.ai.tier.${option}`)}
                </span>
                <p className="mt-1 text-[11px] leading-relaxed text-text-secondary">
                  {t(`settings.ai.tier.${option}.desc`)}
                </p>
              </label>
            ))}
          </div>
          <p className="text-[11px] leading-relaxed text-text-muted">
            {t("onboarding.ai.qualityNote")}
          </p>
        </fieldset>
      )}

      {enabled && <AiDownloadPanel qualityTier={tier} preparing={downloading} />}

      {error !== null && (
        <p className="mt-2 text-[11px] text-red-400" role="alert" data-testid="onboarding-ai-error">
          {error}
        </p>
      )}

      <div className="mt-2 flex items-center justify-between gap-3">
        <span className="text-[11px] text-text-muted">{t("onboarding.ai.note")}</span>
        <div className="flex items-center gap-2">
          {enabled && !downloading && (
            <button
              type="button"
              onClick={() => void manualLater()}
              disabled={saving}
              className="text-xs text-text-muted underline-offset-2 transition-colors hover:text-text-primary hover:underline disabled:opacity-40"
              data-testid="onboarding-ai-later"
            >
              {t("onboarding.ai.downloadLater")}
            </button>
          )}
          <button
            type="button"
            disabled={saving || downloading}
            onClick={() => void (enabled ? downloadAndContinue() : continueDisabled())}
            className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="onboarding-ai-continue"
          >
            {downloading
              ? t("onboarding.ai.downloading")
              : enabled
                ? t("onboarding.ai.downloadAndContinue")
                : t("common.next")}
          </button>
        </div>
      </div>
    </>
  );
}

export default function OnboardingPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const loaded = useSettingsStore((s) => s.loaded);
  useWindowReveal(loaded);

  const [step, setStep] = useState<OnboardingStep>("welcome");
  const [finishing, setFinishing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!loaded) return null;

  /** 完成引导：写兼容位 + 会话选库标志 → 画廊 */
  async function finish(): Promise<void> {
    if (finishing) return;
    setFinishing(true);
    try {
      const current = useSettingsStore.getState().settings;
      await useSettingsStore.getState().save({ ...current, onboardingCompleted: true });
      // 引导内必已创建/拥有数据库（首个库创建即激活）：置会话选库标志
      // 直进主壳，否则 GatedShell 会弹回 /database-picker
      useSettingsStore.getState().setDatabaseChosen(true);
      navigate("/gallery", { replace: true });
    } catch (e) {
      setError(String(e).replace(/^Error:\s*/, ""));
      setFinishing(false);
    }
  }

  return (
    <div className={`flex h-full w-full flex-col bg-bg font-sans text-text-primary ${isMacPlatform() ? "mac-window-content" : ""}`}>
      {/* 无边框窗口：主壳外全屏页也要有自绘标题栏（拖动/最大化/关闭） */}
      <TitleBar />
      <div className="sp-scroll flex min-h-0 flex-1 items-center justify-center overflow-y-auto px-8 py-6">
        <div
          className="flex w-full max-w-xl flex-col gap-4 rounded-xl border border-edge bg-surface p-6"
          data-testid="onboarding-card"
          data-step={step}
        >
          {/* 步骤指示（创建数据库 → AI 功能 → 完成） */}
          <div className="flex items-center gap-2 text-[11px] text-text-muted" data-testid="onboarding-steps">
            {(["welcome", "ai", "done"] as const).map((key, index) => (
              <span key={key} className="flex items-center gap-2">
                {index > 0 && <span aria-hidden="true">·</span>}
                <span
                  className={
                    key === step ? "font-medium text-accent" : undefined
                  }
                  data-testid="onboarding-step-label"
                  data-step={key}
                  data-active={key === step}
                >
                  {t(`onboarding.steps.${key}`)}
                </span>
              </span>
            ))}
          </div>

          {step === "welcome" && (
            <DatabaseStep
              onCreated={() => {
                // 创建成功（创建即激活，多库合法）：无额外态，事件驱动注册表刷新
              }}
              onNext={() => setStep("ai")}
            />
          )}

          {step === "ai" && <AiStep onNext={() => setStep("done")} />}

          {step === "done" && (
            <>
              <h1 className="text-xl font-semibold">{t("onboarding.finished.title")}</h1>
              <p className="text-sm leading-relaxed text-text-secondary">
                {t("onboarding.finished.desc")}
              </p>
              <p className="text-xs leading-relaxed text-text-muted">
                {t("onboarding.finished.hint")}
              </p>
              {error !== null && (
                <p className="text-[11px] text-red-400" role="alert" data-testid="onboarding-finish-error">
                  {error}
                </p>
              )}
              <div className="mt-2 flex justify-end">
                <button
                  type="button"
                  onClick={() => void finish()}
                  disabled={finishing}
                  className="rounded-md bg-accent px-5 py-2 text-sm font-medium text-black transition-opacity hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-40"
                  data-testid="onboarding-start"
                >
                  {t("onboarding.finished.start")}
                </button>
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
