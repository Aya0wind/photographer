import { useState } from "react";
import { useNavigate } from "react-router";
import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import TitleBar from "@/app/shell/TitleBar";
import { useSettingsStore } from "@/stores/settingsStore";
import { useWindowReveal } from "@/lib/windowReveal";
import { isMacPlatform } from "@/lib/platform";
import { useDatabases } from "@/lib/useDatabases";
import {
  databaseCreate,
  photoLibraryCreate,
  type PhotoLibrary,
} from "@/ipc/api";
import { suggestedLibraryName } from "../onboardingConfig";

/**
 * 首次引导（2026-10-09 多数据库修正）：
 * 1. 创建数据库——名称必填 + 位置可选（缺省 = 后端约定路径
 *    `<应用配置目录>/databases/<id>`）；首个库创建即激活，之后一切库内
 *    操作作用于它。已有数据库（引导重入/门禁复检）时只读展示当前库名，
 *    直接进下一步。
 * 2. 引导建立第一个照片库——「新建照片库」（登记空文件夹，之后导入落盘目标
 *    = 该库 root）或「从已有文件夹建立」（reference 登记，只登记不搬文件，
 *    登记即触发后台递归扫描——进度在「存储」页卡片上看）；也可「稍后再建」
 *    直接进入应用（存储页随时可建）。
 * 3. 完成——写 onboardingCompleted 兼容位并进画廊。
 * 门禁在 GatedShell（老语义恢复）：本会话未选数据库或激活数据库无效 →
 * /database-picker（选择页）；首启无数据库由选择页直送本引导。
 */

type OnboardingStep = "welcome" | "library" | "done";
type LibraryMode = "new" | "reference";

const FIELD_CLASS =
  "w-full rounded-md border border-edge bg-bg px-2.5 py-1.5 font-mono text-xs text-text-primary outline-none transition-colors focus:border-accent";

/** 从绝对路径取末段作为缺省库名（Windows/Unix 分隔符都认；取不到回退全路径） */
function baseName(path: string): string {
  const parts = path.split(/[\\/]+/).filter((s) => s.length > 0);
  return parts.length > 0 ? parts[parts.length - 1] : path;
}

/** 第一步（创建数据库）子组件：无数据库 = 创建表单；已有 = 只读展示直进 */
function DatabaseStep({ onCreated, onNext }: { onCreated: () => void; onNext: () => void }) {
  const { t } = useTranslation();
  const databases = useDatabases();
  const [name, setName] = useState("");
  const [dbDir, setDbDir] = useState("");
  /** 名称未被手改过时，选完文件夹自动带出末段作缺省名 */
  const [nameTouched, setNameTouched] = useState(false);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (databases !== null && databases.databases.length > 0) {
    // 已有数据库（引导重入）：只读展示激活库名，直接进下一步
    const active =
      databases.databases.find((db) => db.id === databases.activeId) ??
      databases.databases[0];
    return (
      <>
        <h1 className="text-xl font-semibold">{t("onboarding.title")}</h1>
        <p className="text-sm leading-relaxed text-text-secondary">
          {t("onboarding.dbCreate.existingDesc")}
        </p>
        <div
          className="rounded-lg border border-edge bg-panel/30 p-3 text-xs"
          data-testid="onboarding-db-current"
        >
          <p className="text-text-secondary">{t("onboarding.dbCreate.existingLabel")}</p>
          <p className="mt-1 break-all font-mono text-[11px] text-text-muted">
            {active.name}
          </p>
        </div>
        <div className="mt-2 flex justify-end">
          <button
            type="button"
            onClick={onNext}
            className="rounded-md bg-accent px-5 py-2 text-sm font-medium text-black transition-opacity hover:opacity-90"
            data-testid="onboarding-next"
          >
            {t("onboarding.dbCreate.next")}
          </button>
        </div>
      </>
    );
  }

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

export default function OnboardingPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const loaded = useSettingsStore((s) => s.loaded);
  useWindowReveal(loaded);

  const [step, setStep] = useState<OnboardingStep>("welcome");
  const [mode, setMode] = useState<LibraryMode>("new");
  const [name, setName] = useState(() => suggestedLibraryName());
  const [rootPath, setRootPath] = useState("");
  /** 名称未被手改过时，选完文件夹自动带出末段作缺省名 */
  const [nameTouched, setNameTouched] = useState(false);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [created, setCreated] = useState<PhotoLibrary | null>(null);
  const [finishing, setFinishing] = useState(false);

  if (!loaded) return null;

  const ready = name.trim().length > 0 && rootPath.trim().length > 0 && !creating;

  async function pickDirectory(): Promise<void> {
    try {
      const dir = await openDialog({
        directory: true,
        defaultPath: rootPath.trim() || undefined,
      });
      if (typeof dir === "string" && dir.length > 0) {
        setRootPath(dir);
        if (!nameTouched) setName(baseName(dir));
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function createLibrary(): Promise<void> {
    if (!ready) return;
    setCreating(true);
    setError(null);
    const result = await photoLibraryCreate(
      name.trim(),
      rootPath.trim(),
      mode === "reference",
    );
    setCreating(false);
    if (result.ok) {
      setCreated(result.library);
      setStep("done");
      return;
    }
    // null=invoke 不可用（后端未连接）；字符串=后端业务 Err 文案透传
    setError(
      result.error === null ? t("onboarding.library.unavailable") : result.error,
    );
  }

  /** 完成引导（建库或稍后再建共用）：写兼容位 + 会话选库标志 → 画廊 */
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

  function patchMode(next: LibraryMode): void {
    if (next === mode) return;
    setMode(next);
    // 切模式清路径；名称未手改时回缺省名（新建=「主库」，从文件夹=选完带出）
    setRootPath("");
    if (!nameTouched) setName(next === "new" ? suggestedLibraryName() : "");
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
          {/* 步骤指示（创建数据库 → 建立照片库 → 完成） */}
          <div className="flex items-center gap-2 text-[11px] text-text-muted" data-testid="onboarding-steps">
            {(["welcome", "library", "done"] as const).map((key, index) => (
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
                // 创建成功（首个库自动激活）：无额外态，事件驱动注册表刷新
              }}
              onNext={() => setStep("library")}
            />
          )}

          {step === "library" && (
            <>
              <h1 className="text-xl font-semibold">{t("onboarding.library.title")}</h1>
              <p className="text-sm leading-relaxed text-text-secondary">
                {t("onboarding.library.desc")}
              </p>

              {/* 模式二选一：新建空文件夹 / 从已有文件夹建立（reference） */}
              <div className="grid grid-cols-2 gap-2" data-testid="onboarding-library-modes">
                {(["new", "reference"] as const).map((key) => (
                  <button
                    key={key}
                    type="button"
                    onClick={() => patchMode(key)}
                    aria-pressed={mode === key}
                    className={`rounded-lg border p-3 text-left transition-colors ${
                      mode === key
                        ? "border-accent bg-accent/10"
                        : "border-edge hover:border-text-muted"
                    }`}
                    data-testid={`onboarding-mode-${key}`}
                    data-selected={mode === key}
                  >
                    <span
                      className={`text-sm font-medium ${mode === key ? "text-accent" : "text-text-primary"}`}
                    >
                      {t(`onboarding.library.mode.${key}`)}
                    </span>
                    <p className="mt-1 text-xs leading-relaxed text-text-secondary">
                      {t(`onboarding.library.mode.${key}Desc`)}
                    </p>
                  </button>
                ))}
              </div>

              <label className="mt-1 flex flex-col gap-1 text-xs text-text-secondary">
                {t("onboarding.library.name")}
                <input
                  type="text"
                  value={name}
                  onChange={(e) => {
                    setName(e.target.value);
                    setNameTouched(true);
                  }}
                  className={FIELD_CLASS}
                  data-testid="onboarding-library-name"
                />
              </label>

              <label className="mt-2 flex flex-col gap-1 text-xs text-text-secondary">
                {mode === "new"
                  ? t("onboarding.library.rootNew")
                  : t("onboarding.library.rootReference")}
                <div className="flex gap-2">
                  <input
                    type="text"
                    value={rootPath}
                    onChange={(e) => setRootPath(e.target.value)}
                    className={FIELD_CLASS}
                    data-testid="onboarding-library-root"
                  />
                  <button
                    type="button"
                    onClick={() => void pickDirectory()}
                    className="shrink-0 rounded-md border border-edge px-3 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
                    data-testid="onboarding-library-browse"
                  >
                    {t("onboarding.library.browse")}
                  </button>
                </div>
                <span className="text-[11px] leading-relaxed text-text-muted">
                  {mode === "new"
                    ? t("onboarding.library.rootNewHint")
                    : t("onboarding.library.rootReferenceHint")}
                </span>
              </label>

              {error !== null && (
                <p className="mt-2 text-[11px] text-red-400" role="alert" data-testid="onboarding-library-error">
                  {error}
                </p>
              )}

              <div className="mt-2 flex items-center justify-between">
                <button
                  type="button"
                  onClick={() => void finish()}
                  disabled={finishing}
                  className="text-xs text-text-muted underline-offset-2 transition-colors hover:text-text-primary hover:underline disabled:opacity-40"
                  data-testid="onboarding-later"
                >
                  {t("onboarding.library.later")}
                </button>
                <div className="flex gap-2">
                  <button
                    type="button"
                    onClick={() => setStep("welcome")}
                    className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
                    data-testid="onboarding-back"
                  >
                    {t("common.back")}
                  </button>
                  <button
                    type="button"
                    disabled={!ready}
                    onClick={() => void createLibrary()}
                    className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
                    data-testid="onboarding-library-create"
                  >
                    {creating ? t("onboarding.library.creating") : t("onboarding.library.create")}
                  </button>
                </div>
              </div>
            </>
          )}

          {step === "done" && (
            <>
              <h1 className="text-xl font-semibold">{t("onboarding.finished.title")}</h1>
              <p className="text-sm leading-relaxed text-text-secondary">
                {created !== null
                  ? t("onboarding.finished.created", { name: created.name })
                  : t("onboarding.finished.desc")}
              </p>
              {created !== null && (
                <p
                  className="break-all rounded-lg border border-edge bg-panel/30 p-3 font-mono text-[11px] text-text-muted"
                  data-testid="onboarding-finished-root"
                >
                  {created.rootPath}
                </p>
              )}
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
