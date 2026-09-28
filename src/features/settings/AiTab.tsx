import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  aiFaceDataClear,
  aiModelCancel,
  aiModelDelete,
  aiModelDownload,
  indexKickNow,
  indexRebuild,
  burstStats,
  type AiFeature,
  type AiModelStatus,
  type AiQualityTier,
  type IndexKind,
  type IndexStatus,
  type RebuildKind,
} from "@/ipc/api";
import {
  QUALITY_TIERS,
  normalizeModelId,
  requiredModelIds,
  tierFaceReady,
  tierModelGaps,
  tierModelsReady,
  tierReadyCount,
  tierSemanticReady,
  type QualityTier,
  type TierModelGap,
} from "@/features/settings/lib/qualityTier";
import { formatBytes } from "@/lib/format";
import { useAiStore } from "@/stores/aiStore";
import { useSettingsStore, type DeepPartial, type Settings } from "@/stores/settingsStore";
import {
  SectionTitle,
  SettingRow,
  Toggle,
  SELECT_CLASS,
} from "@/features/settings/pages/SettingsPage";

/**
 * 设置页 AI tab（三档画质版）：
 * - 画质档位选择器（快速/普通/精准）：档位→所需模型映射（qualityTier.ts 契约常量），
 *   齐备→确认弹窗（提示后端将自动重建受影响通道索引）直切；缺件→弹窗列缺失件，
 *   「下载并切换」在下载全部就绪后自动应用档位（aiModelDownloadFinished 事件驱动
 *   aiStore.refresh，本组件 watch 模型快照），「仅下载」只发起下载；切档本身只走
 *   settings_set(ai.qualityTier)，无新命令。
 * - 模型管理：按 feature 分组（语义/人脸），组内逐模型行——名称、tier 徽章
 *   （快速/普通/精准/共用）、体积、状态与下载进度 + 单模型下载/取消/删除按钮 +
 *   组级「全部下载」。下载沿用既有事件进度 UI（aiStore.downloadProgress）。
 * - 索引状态与操作区：三类索引（缩略图/EXIF/语义）计数 + 立即索引（indexKickNow，
 *   幂等；ai 模型未就绪透传后端 Err 文案）；index_status 进 tab 拉一次 +
 *   indexTaskProgress 事件驱动重拉（aiStore）。切档后的重建进度同样走该通道
 *   进度条/任务抽屉，不新造 UI。
 * - 高级调参区：语义阈值（auto/自定义三态）、调度/资源、索引参数、连拍分组全部收进
 *   「高级」折叠分组，勾选「开发人员配置」才显示（localStorage 持久化）
 * - 功能开关门控（三档化）：语义=当前档语义三件齐备；人脸=当前档检测模型+arcface
 *   齐备（旧「组内全部 done」在多档变体清单下会误伤普通档用户）
 * - 调度/CPU 滑条（仅用于 AI 推理）/GPU（DirectML 自动回退）
 * - 人脸数据一键清除（红色强确认，两步确认防误触）
 */

/** 修改即存（与 SettingsPage 主组件同语义） */
function commit(partial: DeepPartial<Settings>): void {
  const { update, save } = useSettingsStore.getState();
  update(partial);
  void save(useSettingsStore.getState().settings);
}

/** 模型显示名（未知 id 回退原 id；契约 id 与现行清单 id 双收录） */
const MODEL_NAMES: Record<string, string> = {
  "siglip2-visual": "语义 · 图像编码",
  "siglip2-vision": "语义 · 图像编码",
  "siglip2-text": "语义 · 文本编码",
  "siglip2-tokenizer": "语义 · 分词器",
  "siglip2-vision-fp16": "语义 · 图像编码（精准）",
  "siglip2-text-fp16": "语义 · 文本编码（精准）",
  scrfd: "人脸 · 检测",
  "scrfd-10g": "人脸 · 检测（快速）",
  arcface: "人脸 · 识别",
};

function modelDisplayName(id: string): string {
  return MODEL_NAMES[id] ?? id;
}

/** 能力分组展示顺序（分组依据=后端清单 feature 字段，非前端硬编码集合） */
const FEATURE_ORDER: AiFeature[] = ["semantic", "face", "selection"];

/** 按归一 id 建索引（档位表/清单双 id 收敛） */
function modelsById(models: readonly AiModelStatus[]): Map<string, AiModelStatus> {
  const map = new Map<string, AiModelStatus>();
  for (const model of models) map.set(normalizeModelId(model.id), model);
  return map;
}

/** 状态徽标（含色） */
function StateBadge({ model }: { model: AiModelStatus }) {
  const { t } = useTranslation();
  const map: Record<AiModelStatus["state"], string> = {
    idle: "bg-panel text-text-muted",
    downloading: "bg-accent/15 text-accent",
    verifying: "bg-sky-400/15 text-sky-300",
    done: "bg-emerald-400/15 text-emerald-400",
    failed: "bg-red-400/15 text-red-400",
  };
  return (
    <span className={`rounded px-1.5 py-0.5 text-[11px] font-medium ${map[model.state]}`}>
      {t(`settings.ai.model.state.${model.state}`)}
    </span>
  );
}

/** 画质档位徽章：快速/普通/精准/共用（null=各档共用件；未发 tier 字段的旧后端同共用） */
function TierBadge({ tier }: { tier: AiQualityTier | null | undefined }) {
  const { t } = useTranslation();
  const key = tier ?? "shared";
  const map: Record<string, string> = {
    fast: "bg-sky-400/15 text-sky-300",
    normal: "bg-accent/15 text-accent",
    accurate: "bg-violet-400/15 text-violet-300",
    shared: "bg-panel text-text-muted",
  };
  return (
    <span
      className={`rounded px-1.5 py-0.5 text-[10px] font-medium ${map[key]}`}
      data-testid={`ai-tier-badge-${key}`}
    >
      {t(`settings.ai.tier.badge.${key}`)}
    </span>
  );
}

// --- 模型管理：组内逐模型行（单模型下载/取消/删除） -----------------------------------

/** 逐模型行：名称 + tier 徽章 + 状态徽标 + 体积/下载进度 + 单模型操作按钮。
 *  下载进度沿用事件驱动 UI（aiStore.downloadProgress，节流 1s；无事件退快照字段）。 */
function ModelRow({ model }: { model: AiModelStatus }) {
  const { t } = useTranslation();
  const refresh = useAiStore((s) => s.refresh);
  const downloadProgress = useAiStore((s) => s.downloadProgress[model.id]);
  const [confirmDelete, setConfirmDelete] = useState(false);

  // 下载中：事件进度优先，无事件时退模型快照字段
  const progress =
    downloadProgress ??
    (model.state === "downloading" && model.bytesTotal > 0
      ? { doneBytes: model.downloadedBytes, totalBytes: model.bytesTotal }
      : null);
  const pct =
    progress && progress.totalBytes > 0
      ? Math.min(100, (progress.doneBytes / progress.totalBytes) * 100)
      : 0;

  const active = model.state === "downloading" || model.state === "verifying";

  async function download(): Promise<void> {
    try {
      await aiModelDownload(model.id);
    } catch {
      // 发起失败：状态以快照为准，行内可重试
    }
    void refresh();
  }

  async function cancel(): Promise<void> {
    try {
      await aiModelCancel(model.id);
    } catch {
      // 已结束的下载后端会 Err，静默跳过
    }
    void refresh();
  }

  /** 删除已装模型释放磁盘（两步确认；删除无事件回执，结算后统一刷新） */
  async function remove(): Promise<void> {
    setConfirmDelete(false);
    try {
      await aiModelDelete(model.id);
    } catch {
      // 文件被占等失败：状态以快照为准
    }
    void refresh();
  }

  return (
    <div
      className="flex min-h-[36px] items-center justify-between gap-4 border-b border-edge/20 py-1.5 last:border-b-0"
      data-testid={`ai-model-row-${model.id}`}
      data-state={model.state}
      data-tier={model.tier ?? "shared"}
    >
      <div className="flex min-w-0 items-center gap-2 text-[11px] text-text-secondary">
        <span className="truncate text-text-primary">{modelDisplayName(model.id)}</span>
        <TierBadge tier={model.tier} />
        <StateBadge model={model} />
        {model.state === "done" && model.version && (
          <span className="font-mono text-[10px] text-text-muted">{model.version}</span>
        )}
      </div>
      <div className="flex shrink-0 items-center gap-2">
        {progress ? (
          <>
            <span className="font-mono text-[10px] tabular-nums text-text-muted">
              {formatBytes(progress.doneBytes)} / {formatBytes(progress.totalBytes)}
            </span>
            <div className="h-1 w-20 overflow-hidden rounded bg-panel">
              <div
                className="h-full bg-accent transition-[width] duration-300"
                style={{ width: `${pct}%` }}
              />
            </div>
          </>
        ) : (
          <span className="font-mono text-[10px] tabular-nums text-text-muted">
            {formatBytes(model.bytesTotal)}
          </span>
        )}
        {(model.state === "idle" || model.state === "failed") && (
          <button
            type="button"
            onClick={() => void download()}
            className="rounded-md bg-accent px-2 py-1 text-[11px] font-medium text-black transition-colors hover:brightness-110"
            data-testid={`ai-model-download-${model.id}`}
          >
            {model.state === "failed" ? t("settings.ai.model.retry") : t("settings.ai.model.download")}
          </button>
        )}
        {active && (
          <button
            type="button"
            onClick={() => void cancel()}
            className="rounded-md border border-edge px-2 py-1 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
            data-testid={`ai-model-cancel-${model.id}`}
          >
            {t("settings.ai.model.cancel")}
          </button>
        )}
        {model.state === "done" && !active &&
          (confirmDelete ? (
            <span className="flex items-center gap-1.5">
              <button
                type="button"
                onClick={() => void remove()}
                className="rounded-md bg-red-500/90 px-2 py-1 text-[11px] font-medium text-white transition-colors hover:bg-red-500"
                data-testid={`ai-model-delete-confirm-${model.id}`}
              >
                {t("settings.ai.model.confirmDelete")}
              </button>
              <button
                type="button"
                onClick={() => setConfirmDelete(false)}
                className="rounded-md border border-edge px-2 py-1 text-[11px] text-text-secondary"
                data-testid={`ai-model-delete-cancel-${model.id}`}
              >
                {t("common.cancel")}
              </button>
            </span>
          ) : (
            <button
              type="button"
              onClick={() => setConfirmDelete(true)}
              className="rounded-md border border-edge px-2 py-1 text-[11px] text-text-secondary transition-colors hover:border-red-400 hover:text-red-400"
              data-testid={`ai-model-delete-${model.id}`}
            >
              {t("settings.ai.model.delete")}
            </button>
          ))}
      </div>
    </div>
  );
}

// --- 模型管理：按 feature 分组的卡片（组级「全部下载」+ 聚合进度） ---------------------

/** 整组状态（派生自组内模型快照） */
type GroupState = "downloading" | "ready" | "partialFailed" | "partial" | "none";

function groupState(models: AiModelStatus[]): GroupState {
  const installed = models.filter((m) => m.state === "done").length;
  const active = models.some((m) => m.state === "downloading" || m.state === "verifying");
  const failed = models.some((m) => m.state === "failed");
  if (active) return "downloading";
  if (models.length > 0 && installed === models.length) return "ready";
  if (failed) return "partialFailed";
  return installed === 0 ? "none" : "partial";
}

/** 组卡片：语义搜索模型 / 人脸识别模型。操作粒度=逐模型行；组级保留「全部下载」
 *  （只发起组内未装模型，逐模型错误隔离）与聚合进度条（组内在途模型字节聚合）。 */
function ModelGroupCard({ feature, models }: { feature: AiFeature; models: AiModelStatus[] }) {
  const { t } = useTranslation();
  const refresh = useAiStore((s) => s.refresh);
  const downloadProgress = useAiStore((s) => s.downloadProgress);

  const installed = models.filter((m) => m.state === "done").length;
  const allDone = models.length > 0 && installed === models.length;
  const activeModels = models.filter(
    (m) => m.state === "downloading" || m.state === "verifying",
  );
  const anyActive = activeModels.length > 0;
  const anyFailed = models.some((m) => m.state === "failed");
  const badge = groupState(models);

  // 组级进度：组内在途模型字节聚合（事件进度优先，缺事件退快照字段）
  const grpProgress = anyActive
    ? activeModels.reduce(
        (acc, m) => {
          const p =
            downloadProgress[m.id] ?? {
              doneBytes: m.downloadedBytes,
              totalBytes: m.bytesTotal,
            };
          return { doneBytes: acc.doneBytes + p.doneBytes, totalBytes: acc.totalBytes + p.totalBytes };
        },
        { doneBytes: 0, totalBytes: 0 },
      )
    : null;
  const grpPct =
    grpProgress && grpProgress.totalBytes > 0
      ? Math.min(100, (grpProgress.doneBytes / grpProgress.totalBytes) * 100)
      : 0;

  /** 全部下载/重试：只发起组内未装模型（idle/failed），逐模型错误隔离 */
  async function downloadAll(): Promise<void> {
    for (const model of models) {
      if (model.state !== "idle" && model.state !== "failed") continue;
      try {
        await aiModelDownload(model.id);
      } catch {
        // 隔离：继续下一个模型
      }
    }
    void refresh();
  }

  const badgeClass: Record<GroupState, string> = {
    downloading: "bg-accent/15 text-accent",
    ready: "bg-emerald-400/15 text-emerald-400",
    partialFailed: "bg-red-400/15 text-red-400",
    partial: "bg-sky-400/15 text-sky-300",
    none: "bg-panel text-text-muted",
  };

  return (
    <div
      className="mt-2 rounded-lg border border-edge p-3"
      data-testid={`ai-model-group-${feature}`}
      data-installed={installed}
      data-total={models.length}
    >
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <div className="flex items-center gap-2 text-xs text-text-primary">
            <span className="font-medium">{t(`settings.ai.package.${feature}`)}</span>
            <span
              className={`rounded px-1.5 py-0.5 text-[11px] font-medium ${badgeClass[badge]}`}
              data-testid={`ai-group-badge-${feature}`}
              data-state={badge}
            >
              {t(`settings.ai.package.state.${badge}`, {
                installed,
                total: models.length,
              })}
            </span>
          </div>
          <div className="mt-0.5 text-[11px] text-text-muted">
            {t("settings.ai.package.modelCount", { count: models.length })} ·{" "}
            {formatBytes(models.reduce((sum, m) => sum + m.bytesTotal, 0))}
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          {!allDone && (
            <button
              type="button"
              onClick={() => void downloadAll()}
              className="rounded-md bg-accent px-2.5 py-1 text-[11px] font-medium text-black transition-colors hover:brightness-110"
              data-testid={`ai-group-download-${feature}`}
            >
              {anyFailed ? t("settings.ai.package.retry") : t("settings.ai.model.downloadAll")}
            </button>
          )}
        </div>
      </div>

      {grpProgress && (
        <div className="mt-2">
          <div className="h-1 overflow-hidden rounded bg-panel" data-testid={`ai-group-progress-${feature}`}>
            <div
              className="h-full bg-accent transition-[width] duration-300"
              style={{ width: `${grpPct}%` }}
            />
          </div>
          <div className="mt-0.5 font-mono text-[10px] tabular-nums text-text-muted">
            {formatBytes(grpProgress.doneBytes)} / {formatBytes(grpProgress.totalBytes)}
          </div>
        </div>
      )}

      {anyFailed && !anyActive && (
        <p className="mt-1.5 text-[11px] text-red-400" data-testid={`ai-group-failed-${feature}`}>
          {t("settings.ai.package.failedHint")}
        </p>
      )}

      <div className="mt-2 border-t border-edge/40 pt-1" data-testid={`ai-group-models-${feature}`}>
        {models.map((model) => (
          <ModelRow key={model.id} model={model} />
        ))}
      </div>
    </div>
  );
}

// --- 画质档位选择器（三档：快速/普通/精准） ---------------------------------------------

/** 切档请求的待决弹窗：齐备→确认直切；缺件→列缺失件给下载并切换/仅下载 */
type TierDialog =
  | { kind: "confirm"; tier: QualityTier }
  | { kind: "missing"; tier: QualityTier; gaps: TierModelGap[] }
  | null;

function QualityTierSection() {
  const { t } = useTranslation();
  const settings = useSettingsStore((s) => s.settings);
  const models = useAiStore((s) => s.models) ?? [];
  const modelsLoaded = useAiStore((s) => s.modelsLoaded);
  const refresh = useAiStore((s) => s.refresh);
  const [dialog, setDialog] = useState<TierDialog>(null);
  /** 「下载并切换」等待中的目标档：模型齐备后自动应用 */
  const [pendingTier, setPendingTier] = useState<QualityTier | null>(null);

  const currentTier: QualityTier = settings.ai.qualityTier ?? "normal";
  const catalogAvailable = modelsLoaded && models.length > 0;
  const byId = modelsById(models);

  // 下载全部就绪后自动应用档位（快照经 aiModelDownloadFinished → refresh 驱动更新）
  useEffect(() => {
    if (pendingTier && catalogAvailable && tierModelsReady(pendingTier, models)) {
      const tier = pendingTier;
      setPendingTier(null);
      commit({ ai: { qualityTier: tier } });
    }
  }, [pendingTier, models, catalogAvailable]);

  /** 发起点击档位：齐备→确认弹窗；缺件→缺件弹窗；后端未连接→直切（无门控信息） */
  function requestTier(tier: QualityTier): void {
    if (tier === currentTier) return;
    if (!catalogAvailable) {
      commit({ ai: { qualityTier: tier } });
      return;
    }
    const gaps = tierModelGaps(tier, models);
    if (gaps.length === 0) setDialog({ kind: "confirm", tier });
    else setDialog({ kind: "missing", tier, gaps });
  }

  /** 发起缺件下载（逐模型错误隔离；清单未收录的件无法发起，跳过） */
  async function downloadGaps(gaps: TierModelGap[]): Promise<void> {
    for (const gap of gaps) {
      if (!gap.status) continue;
      try {
        await aiModelDownload(gap.status.id);
      } catch {
        // 隔离：继续下一个模型
      }
    }
    void refresh();
  }

  const pendingReady = pendingTier ? tierReadyCount(pendingTier, models) : null;
  const pendingFailed =
    pendingTier && catalogAvailable
      ? tierModelGaps(pendingTier, models).some((g) => g.status?.state === "failed")
      : false;

  return (
    <div data-testid="ai-quality-tier" data-tier={currentTier}>
      <SectionTitle>{t("settings.section.aiQuality")}</SectionTitle>

      {/* 档位选择（segmented：每档一行说明 + 就绪状态） */}
      <div
        role="radiogroup"
        aria-label={t("settings.ai.qualityTier")}
        className="mt-1 flex flex-col gap-1.5"
      >
        {QUALITY_TIERS.map((tier) => {
          const selected = tier === currentTier;
          const count = catalogAvailable ? tierReadyCount(tier, models) : null;
          return (
            <button
              key={tier}
              type="button"
              role="radio"
              aria-checked={selected}
              onClick={() => requestTier(tier)}
              className={`rounded-lg border px-3 py-2 text-left transition-colors ${
                selected
                  ? "border-accent bg-accent/10"
                  : "border-edge hover:border-accent/60"
              }`}
              data-testid={`ai-tier-option-${tier}`}
            >
              <div className="flex items-center gap-2">
                <span
                  className={`text-xs font-medium ${selected ? "text-accent" : "text-text-primary"}`}
                >
                  {t(`settings.ai.tier.${tier}`)}
                </span>
                {tier === "normal" && (
                  <span className="rounded bg-panel px-1.5 py-0.5 text-[10px] text-text-muted">
                    {t("settings.ai.tier.defaultTag")}
                  </span>
                )}
                {count !== null && (
                  <span
                    className={`ml-auto text-[11px] ${
                      count.ready === count.total ? "text-emerald-400" : "text-text-muted"
                    }`}
                    data-testid={`ai-tier-option-status-${tier}`}
                  >
                    {count.ready === count.total
                      ? t("settings.ai.tier.state.ready")
                      : t("settings.ai.tier.state.missing", { count: count.total - count.ready })}
                  </span>
                )}
              </div>
              <div className="mt-0.5 text-[11px] text-text-muted">
                {t(`settings.ai.tier.${tier}.desc`)}
              </div>
            </button>
          );
        })}
      </div>

      {/* 当前档所需模型清单及安装状态 */}
      {catalogAvailable && (
        <div className="mt-2 rounded-lg border border-edge/60 p-2" data-testid="ai-tier-models">
          <div className="text-[10px] font-semibold uppercase tracking-wider text-text-muted">
            {t("settings.ai.tier.modelsTitle", { tier: t(`settings.ai.tier.${currentTier}`) })}
          </div>
          {requiredModelIds(currentTier).map((id) => {
            const status = byId.get(id) ?? null;
            return (
              <div
                key={id}
                className="flex min-h-[28px] items-center justify-between gap-4 py-1"
                data-testid={`ai-tier-model-${id}`}
                data-state={status?.state ?? "missing"}
              >
                <div className="flex min-w-0 items-center gap-2 text-[11px] text-text-secondary">
                  <span className="truncate text-text-primary">{modelDisplayName(id)}</span>
                  <TierBadge tier={status?.tier ?? null} />
                </div>
                <span className="shrink-0 font-mono text-[10px] tabular-nums text-text-muted">
                  {status
                    ? `${t(`settings.ai.model.state.${status.state}`)} · ${formatBytes(status.bytesTotal)}`
                    : t("settings.ai.tier.notInCatalog")}
                </span>
              </div>
            );
          })}
        </div>
      )}

      {/* 「下载并切换」等待横幅：就绪后自动应用；可取消等待 */}
      {pendingTier && (
        <div
          className="mt-2 rounded-lg border border-accent/40 bg-accent/5 px-3 py-2"
          data-testid="ai-tier-pending"
          data-tier={pendingTier}
        >
          <div className="flex items-center justify-between gap-4">
            <span className="text-[11px] text-text-secondary">
              {catalogAvailable && pendingReady
                ? t("settings.ai.tier.pending", {
                    tier: t(`settings.ai.tier.${pendingTier}`),
                    ready: pendingReady.ready,
                    total: pendingReady.total,
                  })
                : t("settings.ai.tier.pendingNoCount", {
                    tier: t(`settings.ai.tier.${pendingTier}`),
                  })}
            </span>
            <button
              type="button"
              onClick={() => setPendingTier(null)}
              className="shrink-0 rounded-md border border-edge px-2 py-1 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
              data-testid="ai-tier-pending-cancel"
            >
              {t("settings.ai.tier.pendingCancel")}
            </button>
          </div>
          {pendingFailed && (
            <p className="mt-1 text-[11px] text-red-400" data-testid="ai-tier-pending-failed">
              {t("settings.ai.tier.pendingFailed")}
            </p>
          )}
        </div>
      )}

      {/* 齐备直切确认弹窗：切换即生效，后端自动重建受影响通道索引 */}
      {dialog?.kind === "confirm" && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
          role="dialog"
          aria-modal="true"
          aria-label={t("settings.ai.tier.switchTitle")}
          data-testid="ai-tier-switch-confirm"
        >
          <div className="w-[380px] rounded-xl border border-edge bg-surface p-4 shadow-2xl">
            <h2 className="text-sm font-semibold text-text-primary">
              {t("settings.ai.tier.switchTitle")}
            </h2>
            <p className="mt-2 text-xs leading-relaxed text-text-secondary">
              {t("settings.ai.tier.switchTo", { tier: t(`settings.ai.tier.${dialog.tier}`) })}
              {t("settings.ai.tier.switchRebuildNote")}
            </p>
            <div className="mt-4 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setDialog(null)}
                className="rounded-md border border-edge px-2.5 py-1 text-[11px] text-text-secondary"
                data-testid="ai-tier-switch-cancel"
              >
                {t("common.cancel")}
              </button>
              <button
                type="button"
                onClick={() => {
                  commit({ ai: { qualityTier: dialog.tier } });
                  setDialog(null);
                }}
                className="rounded-md bg-accent px-2.5 py-1 text-[11px] font-medium text-black transition-colors hover:brightness-110"
                data-testid="ai-tier-switch-confirm-yes"
              >
                {t("settings.ai.tier.switchYes")}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* 缺件弹窗：列缺失件 + 下载并切换 / 仅下载 */}
      {dialog?.kind === "missing" && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
          role="dialog"
          aria-modal="true"
          aria-label={t("settings.ai.tier.missingTitle")}
          data-testid="ai-tier-missing-dialog"
        >
          <div className="w-[420px] rounded-xl border border-edge bg-surface p-4 shadow-2xl">
            <h2 className="text-sm font-semibold text-text-primary">
              {t("settings.ai.tier.missingTitle")}
            </h2>
            <p className="mt-1 text-[11px] text-text-secondary">
              {t("settings.ai.tier.missingDesc", {
                tier: t(`settings.ai.tier.${dialog.tier}`),
                count: dialog.gaps.length,
              })}
            </p>
            <div className="mt-2 flex flex-col" data-testid="ai-tier-missing-list">
              {dialog.gaps.map((gap) => (
                <div
                  key={gap.id}
                  className="flex items-center justify-between gap-4 border-b border-edge/30 py-1.5 last:border-b-0"
                  data-testid={`ai-tier-missing-item-${gap.id}`}
                >
                  <span className="min-w-0 truncate text-[11px] text-text-primary">
                    {modelDisplayName(gap.id)}
                  </span>
                  <span className="shrink-0 font-mono text-[10px] tabular-nums text-text-muted">
                    {gap.status ? formatBytes(gap.status.bytesTotal) : t("settings.ai.tier.notInCatalog")}
                  </span>
                </div>
              ))}
            </div>
            <div className="mt-4 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setDialog(null)}
                className="rounded-md border border-edge px-2.5 py-1 text-[11px] text-text-secondary"
                data-testid="ai-tier-missing-cancel"
              >
                {t("common.cancel")}
              </button>
              <button
                type="button"
                onClick={() => {
                  void downloadGaps(dialog.gaps);
                  setDialog(null);
                }}
                className="rounded-md border border-edge px-2.5 py-1 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
                data-testid="ai-tier-download-only"
              >
                {t("settings.ai.tier.downloadOnly")}
              </button>
              <button
                type="button"
                onClick={() => {
                  setPendingTier(dialog.tier);
                  void downloadGaps(dialog.gaps);
                  setDialog(null);
                }}
                className="rounded-md bg-accent px-2.5 py-1 text-[11px] font-medium text-black transition-colors hover:brightness-110"
                data-testid="ai-tier-download-switch"
              >
                {t("settings.ai.tier.downloadSwitch")}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

// --- 索引参数与重建区（M4.5 wave-3 第 7 项） -------------------------------------------

/** 数值参数行：改即存（钳制到合法区间）；偏离默认时给「恢复默认」 */
function AiParamRow({
  label,
  desc,
  value,
  defaultValue,
  min,
  max,
  step,
  testId,
  onCommit,
}: {
  label: string;
  desc: string;
  value: number;
  defaultValue: number;
  min: number;
  max: number;
  step: number;
  testId: string;
  onCommit: (next: number) => void;
}) {
  const { t } = useTranslation();
  // 编辑期本地草稿（受控值不逐键回写——钳制会造成光标跳动/吞字）；失焦/回车提交
  const [draft, setDraft] = useState<string | null>(null);
  const shown = draft ?? String(value);
  function commitDraft(): void {
    if (draft === null) return;
    const n = Number(draft);
    if (Number.isFinite(n)) onCommit(Math.min(max, Math.max(min, n)));
    setDraft(null);
  }
  return (
    <SettingRow label={label} desc={desc} testId={`settings-row-${testId}`}>
      <div className="flex items-center gap-2">
        <input
          type="number"
          value={shown}
          min={min}
          max={max}
          step={step}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commitDraft}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              commitDraft();
            }
          }}
          aria-label={label}
          className="w-24 rounded-md border border-edge bg-bg px-2 py-1 text-right font-mono text-xs tabular-nums text-text-primary outline-none transition-colors focus:border-accent"
          data-testid={testId}
        />
        {value !== defaultValue && (
          <button
            type="button"
            onClick={() => onCommit(defaultValue)}
            className="rounded-md border border-edge px-2 py-1 text-[11px] text-text-muted transition-colors hover:border-accent hover:text-accent"
            data-testid={`${testId}-reset`}
          >
            {t("settings.ai.param.resetDefault")}
          </button>
        )}
      </div>
    </SettingRow>
  );
}

/** 与每个索引状态行并列的重建按钮：二次红色确认后重跑同一后台管线。 */
function RebuildButton({ kind }: { kind: RebuildKind }) {
  const { t } = useTranslation();
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const refreshIndexStatus = useAiStore((s) => s.refreshIndexStatus);

  async function rebuild(): Promise<void> {
    setConfirming(false);
    setError(null);
    try {
      await indexRebuild(kind);
    } catch (err) {
      // 后端 Err 文案透传（如「请先在设置中下载模型」）
      setError(err instanceof Error ? err.message : typeof err === "string" ? err : null);
      return;
    }
    // 进行中态由任务抽屉（IndexTaskResumed/Progress 事件）反映；此处刷新计数快照
    void refreshIndexStatus();
  }

  return (
    <span className="flex items-center gap-1.5" data-testid={`settings-rebuild-${kind}`}>
      {confirming ? (
        <span className="flex items-center gap-1.5">
          <span className="text-[11px] text-red-400">{t("settings.ai.rebuild.confirm")}</span>
          <button
            type="button"
            onClick={() => void rebuild()}
            className="rounded-md bg-red-500/90 px-2 py-1 text-[11px] font-medium text-white transition-colors hover:bg-red-500"
            data-testid={`ai-rebuild-confirm-${kind}`}
          >
            {t("settings.ai.rebuild.confirmYes")}
          </button>
          <button
            type="button"
            onClick={() => setConfirming(false)}
            className="rounded-md border border-edge px-2 py-1 text-[11px] text-text-secondary"
            data-testid={`ai-rebuild-cancel-${kind}`}
          >
            {t("common.cancel")}
          </button>
        </span>
      ) : (
        <button
          type="button"
          onClick={() => setConfirming(true)}
          className="rounded-md border border-edge px-2.5 py-1 text-[11px] text-text-secondary transition-colors hover:border-red-400 hover:text-red-400"
          data-testid={`ai-rebuild-${kind}`}
        >
          {t("settings.ai.rebuild.run")}
        </button>
      )}
      {error !== null && (
        <span className="max-w-[160px] truncate text-[11px] text-red-400" title={error} data-testid="ai-rebuild-error">
          {error}
        </span>
      )}
    </span>
  );
}

function AiParamsSection() {
  const { t } = useTranslation();
  const settings = useSettingsStore((s) => s.settings);
  const rebuildHint = t("settings.ai.param.rebuildHint");

  return (
    <div className="mt-2" data-testid="ai-index-params">
      <AiParamRow
        label={t("settings.ai.param.embedInputSize")}
        desc={rebuildHint}
        value={settings.ai.embedInputSize ?? 256}
        defaultValue={256}
        min={128}
        max={512}
        step={1}
        testId="ai-param-embed-input-size"
        onCommit={(embedInputSize) => commit({ ai: { embedInputSize } })}
      />
      <AiParamRow
        label={t("settings.ai.param.faceDetectThreshold")}
        desc={rebuildHint}
        value={settings.ai.faceDetectThreshold ?? 0.5}
        defaultValue={0.5}
        min={0.1}
        max={0.9}
        step={0.05}
        testId="ai-param-face-detect-threshold"
        onCommit={(faceDetectThreshold) => commit({ ai: { faceDetectThreshold } })}
      />
      <AiParamRow
        label={t("settings.ai.param.faceClusterThreshold")}
        desc={rebuildHint}
        value={settings.ai.faceClusterThreshold ?? 0.4}
        defaultValue={0.4}
        min={0.1}
        max={0.9}
        step={0.05}
        testId="ai-param-face-cluster-threshold"
        onCommit={(faceClusterThreshold) => commit({ ai: { faceClusterThreshold } })}
      />

    </div>
  );
}

// --- 连拍分组区（M6） -----------------------------------------------------------------

/** 连拍分组参数 + burstStats 展示行（与索引参数同款：草稿失焦提交 + 恢复默认） */
function BurstSection() {
  const { t } = useTranslation();
  const settings = useSettingsStore((s) => s.settings);
  const [stats, setStats] = useState<{ groups: number; photosInBursts: number } | null>(null);
  const rebuildHint = t("settings.ai.param.burstRebuildHint");

  // 进 tab 拉一次统计（后端不可用为 null——展示行隐藏）
  useEffect(() => {
    let cancelled = false;
    void burstStats().then((result) => {
      if (!cancelled) setStats(result);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div className="mt-2" data-testid="ai-burst-params">
      <AiParamRow
        label={t("settings.ai.param.burstGapMs")}
        desc={rebuildHint}
        value={settings.ai.burstGapMs ?? 2000}
        defaultValue={2000}
        min={500}
        max={10000}
        step={100}
        testId="ai-param-burst-gap-ms"
        onCommit={(burstGapMs) => commit({ ai: { burstGapMs } })}
      />
      <AiParamRow
        label={t("settings.ai.param.burstHammingMax")}
        desc={rebuildHint}
        value={settings.ai.burstHammingMax ?? 10}
        defaultValue={10}
        min={4}
        max={24}
        step={1}
        testId="ai-param-burst-hamming-max"
        onCommit={(burstHammingMax) => commit({ ai: { burstHammingMax } })}
      />
      <AiParamRow
        label={t("settings.ai.param.burstMinSize")}
        desc={rebuildHint}
        value={settings.ai.burstMinSize ?? 2}
        defaultValue={2}
        min={2}
        max={10}
        step={1}
        testId="ai-param-burst-min-size"
        onCommit={(burstMinSize) => commit({ ai: { burstMinSize } })}
      />
      {stats !== null && (
        <SettingRow
          label={t("settings.ai.burstStats")}
          testId="settings-row-burst-stats"
        >
          <span
            className="font-mono text-xs tabular-nums text-text-secondary"
            data-testid="ai-burst-stats"
          >
            {t("settings.ai.burstStatsValue", {
              groups: stats.groups,
              photos: stats.photosInBursts,
            })}
          </span>
        </SettingRow>
      )}
    </div>
  );
}

// --- 索引状态与操作区 ---------------------------------------------------------------

type T = ReturnType<typeof useTranslation>["t"];

/** 行计数文案（thumb/exif：待处理/已完成/失败；ai：待处理 + 已索引 N/M + 失败） */
function countersText(kind: IndexKind, status: IndexStatus, t: T): string {
  if (kind === "ai" || kind === "face") {
    const c = kind === "ai" ? status.ai : status.face;
    if (!c) return "—";
    return [
      t("settings.ai.index.pending", { count: c.pending }),
      t("settings.ai.index.aiProgress", { done: c.done, total: c.total }),
      c.failed > 0 ? t("settings.ai.index.failed", { count: c.failed }) : null,
    ]
      .filter((part): part is string => part !== null)
      .join(" · ");
  }
  const c = kind === "thumb" ? status.thumb : status.exif;
  return [
    t("settings.ai.index.pending", { count: c.pending }),
    t("settings.ai.index.done", { count: c.done }),
    c.failed > 0 ? t("settings.ai.index.failed", { count: c.failed }) : null,
  ]
    .filter((part): part is string => part !== null)
    .join(" · ");
}

function IndexStatusSection() {
  const { t } = useTranslation();
  const status = useAiStore((s) => s.indexStatus);
  const refreshIndexStatus = useAiStore((s) => s.refreshIndexStatus);
  const [error, setError] = useState<string | null>(null);
  const [kicking, setKicking] = useState<Set<IndexKind>>(() => new Set());

  // 进 tab 拉一次；此后 indexTaskProgress/indexTaskResumed 事件经 aiStore 驱动重拉
  useEffect(() => {
    void refreshIndexStatus();
  }, [refreshIndexStatus]);

  const kick = useCallback(
    async (kind: IndexKind) => {
      if (kicking.has(kind)) return;
      setKicking((current) => new Set(current).add(kind));
      setError(null);
      try {
        await indexKickNow(kind);
      } catch (err) {
        // 后端 Err 文案透传（如 ai 模型未就绪「请先在设置中下载模型」）
        setError(err instanceof Error ? err.message : typeof err === "string" ? err : null);
        return;
      } finally {
        setKicking((current) => {
          const next = new Set(current);
          next.delete(kind);
          return next;
        });
      }
      void refreshIndexStatus();
    },
    [kicking, refreshIndexStatus],
  );

  const rows: IndexKind[] = status?.face ? ["thumb", "exif", "ai", "face"] : ["thumb", "exif", "ai"];

  return (
    <>
      <SectionTitle>{t("settings.ai.indexSection")}</SectionTitle>
      {status === null ? (
        <>
          <p className="py-2 text-[11px] text-text-muted" data-testid="index-status-unavailable">
            {t("settings.ai.index.unavailable")}
          </p>
          {(["thumb", "exif", "ai", "face"] as const).map((kind) => (
            <div
              key={kind}
              className="flex min-h-[36px] items-center justify-between gap-8 border-b border-edge/40 py-2"
              data-testid={`index-rebuild-row-${kind}`}
            >
              <div className="text-xs text-text-primary">{t(`settings.ai.index.${kind}`)}</div>
              <RebuildButton kind={kind === "ai" ? "semantic" : kind} />
            </div>
          ))}
        </>
      ) : (
        rows.map((kind) => {
          const label = t(`settings.ai.index.${kind}`);
          // 运行态从持久化任务账派生（index_tasks 表是唯一真值）：
          // 切页重挂载/应用重启后快照重拉，按钮状态随之恢复——
          // 本地 state 派生会在重挂载时丢失（真机修复 2026-09-19）
          const c = status[kind];
          if (!c) return null;
          const running = kicking.has(kind) || c.pending > 0 || c.running > 0;
          const complete = c.total === 0 || c.done >= c.total;
          return (
            <div
              key={kind}
              className="flex min-h-[36px] items-center justify-between gap-8 border-b border-edge/40 py-2"
              data-testid={`index-status-${kind}`}
              data-running={running}
            >
              <div className="min-w-0">
                <div className="text-xs text-text-primary">{label}</div>
                <div className="mt-0.5 font-mono text-[11px] tabular-nums text-text-muted" data-testid={`index-count-${kind}`}>
                  {countersText(kind, status, t)}
                </div>
              </div>
              <div className="flex shrink-0 items-center gap-2">
                <button
                  type="button"
                  disabled={running || complete}
                  onClick={() => void kick(kind)}
                  className={`shrink-0 rounded-md px-2.5 py-1 text-[11px] font-medium transition-colors ${
                    running || complete
                      ? "cursor-default bg-panel text-text-muted"
                      : "bg-accent text-black hover:brightness-110"
                  }`}
                  data-testid={`index-kick-${kind}`}
                >
                  {running
                    ? t("settings.ai.index.running")
                    : complete
                      ? t("settings.ai.index.complete")
                      : t("settings.ai.index.kick")}
                </button>
                <RebuildButton kind={kind === "ai" ? "semantic" : kind} />
              </div>
            </div>
          );
        })
      )}
      {error !== null && (
        <p className="text-[11px] text-red-400" role="alert" data-testid="index-kick-error">
          {t("settings.ai.index.kickError", { error })}
        </p>
      )}
    </>
  );
}

// --- 高级调参折叠分组（④） -------------------------------------------------------------

/** 「开发人员配置」开关持久化 key（localStorage，与智能相册标签同款简化存储） */
const AI_ADVANCED_KEY = "smartphoto.settings.ai.advanced";

function loadAiAdvanced(): boolean {
  try {
    return localStorage.getItem(AI_ADVANCED_KEY) === "1";
  } catch {
    return false;
  }
}

function saveAiAdvanced(on: boolean): void {
  try {
    localStorage.setItem(AI_ADVANCED_KEY, on ? "1" : "0");
  } catch {
    // 隐私模式等：仅本次会话生效
  }
}

/**
 * 高级分组：语义阈值、调度/资源、连拍阈值/间隔、人脸检测与聚类阈值等调参
 * 输入收进可折叠区，勾选「开发人员配置」才显示（localStorage 持久化）。
 */
function AdvancedSection() {
  const { t } = useTranslation();
  const [advanced, setAdvanced] = useState(loadAiAdvanced);

  function toggleAdvanced(next: boolean): void {
    setAdvanced(next);
    saveAiAdvanced(next);
  }

  return (
    <>
      <SectionTitle>{t("settings.ai.advancedSection")}</SectionTitle>
      <SettingRow
        label={t("settings.ai.advancedToggle")}
        desc={t("settings.ai.advancedToggleDesc")}
        testId="settings-row-ai-advanced"
      >
        <Toggle
          checked={advanced}
          label={t("settings.ai.advancedToggle")}
          testId="ai-advanced-toggle"
          onChange={toggleAdvanced}
        />
      </SettingRow>
      {advanced && (
        <div data-testid="ai-advanced-params">
          <SemanticThresholdRow />
          <ScheduleSection />
          <AiParamsSection />
          <BurstSection />
        </div>
      )}
    </>
  );
}

/** 语义相似度阈值三态：auto（默认，跟随模型：普通档 0.09 / 精准档 fp16 标定值）
 *  / 自定义数值（0-1 钳制）。settings.ai.semanticMinScore=null 即 auto 语义。 */
function SemanticThresholdRow() {
  const { t } = useTranslation();
  const settings = useSettingsStore((s) => s.settings);
  const value = settings.ai.semanticMinScore;
  const auto = value === null || value === undefined;

  return (
    <SettingRow
      label={t("settings.ai.semanticThreshold")}
      desc={t("settings.ai.semanticThresholdDesc")}
      testId="settings-row-semantic-threshold"
    >
      <div className="flex flex-col items-end gap-1">
        <div className="flex items-center gap-2">
          <div
            role="radiogroup"
            aria-label={t("settings.ai.semanticThresholdMode")}
            className="flex rounded-md border border-edge bg-bg p-0.5"
            data-testid="ai-semantic-threshold-mode"
            data-mode={auto ? "auto" : "custom"}
          >
            <button
              type="button"
              role="radio"
              aria-checked={auto}
              onClick={() => {
                if (!auto) commit({ ai: { semanticMinScore: null } });
              }}
              className={`rounded px-2 py-1 text-[11px] transition-colors ${
                auto ? "bg-accent text-black" : "text-text-secondary hover:text-text-primary"
              }`}
              data-testid="ai-semantic-threshold-auto"
            >
              {t("settings.ai.semanticThresholdAuto")}
            </button>
            <button
              type="button"
              role="radio"
              aria-checked={!auto}
              onClick={() => {
                if (auto) commit({ ai: { semanticMinScore: 0.09 } });
              }}
              className={`rounded px-2 py-1 text-[11px] transition-colors ${
                !auto ? "bg-accent text-black" : "text-text-secondary hover:text-text-primary"
              }`}
              data-testid="ai-semantic-threshold-custom"
            >
              {t("settings.ai.semanticThresholdCustom")}
            </button>
          </div>
          <input
            type="number"
            min={0}
            max={1}
            step={0.01}
            disabled={auto}
            value={auto ? "" : String(value)}
            onChange={(e) => {
              const v = Number(e.target.value);
              if (Number.isFinite(v)) {
                commit({ ai: { semanticMinScore: Math.min(1, Math.max(0, v)) } });
              }
            }}
            aria-label={t("settings.ai.semanticThreshold")}
            className="w-20 rounded border border-edge bg-bg px-2 py-1 font-mono text-xs text-text-primary outline-none focus:border-accent disabled:opacity-40"
            data-testid="ai-semantic-threshold"
          />
          {!auto && (
            <button
              type="button"
              onClick={() => commit({ ai: { semanticMinScore: null } })}
              className="rounded border border-edge px-2 py-1 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
              data-testid="ai-semantic-threshold-reset"
            >
              {t("settings.ai.semanticThresholdResetAuto")}
            </button>
          )}
        </div>
        {auto && (
          <span
            className="text-[10px] text-text-muted"
            data-testid="ai-semantic-threshold-auto-hint"
          >
            {t("settings.ai.semanticThresholdAutoHint")}
          </span>
        )}
      </div>
    </SettingRow>
  );
}

/** 调度 / 资源组（④ 起收进「高级」折叠分组） */
function ScheduleSection() {
  const { t } = useTranslation();
  const settings = useSettingsStore((s) => s.settings);
  return (
    <>
      <SectionTitle>{t("settings.section.aiSchedule")}</SectionTitle>
      <SettingRow label={t("settings.ai.schedule")}>
        <select
          value={settings.ai.indexSchedule}
          onChange={(e) =>
            commit({
              ai: {
                indexSchedule: e.target.value as Settings["ai"]["indexSchedule"],
              },
            })
          }
          aria-label={t("settings.ai.schedule")}
          className={SELECT_CLASS}
          data-testid="ai-schedule"
        >
          <option value="idleOnly">{t("settings.ai.schedule.idleOnly")}</option>
          <option value="afterImport">{t("settings.ai.schedule.afterImport")}</option>
          <option value="manual">{t("settings.ai.schedule.manual")}</option>
        </select>
      </SettingRow>
      <SettingRow label={t("settings.ai.cpu")} desc={t("settings.ai.cpuDesc")}>
        <input
          type="range"
          min={10}
          max={100}
          step={10}
          value={settings.ai.cpuLimitPercent}
          onChange={(e) => commit({ ai: { cpuLimitPercent: Number(e.target.value) } })}
          aria-label={t("settings.ai.cpu")}
          className="w-40 accent-[#F0A83C]"
          data-testid="ai-cpu-slider"
        />
        <span className="ml-2 w-10 font-mono text-xs tabular-nums text-text-secondary">
          {settings.ai.cpuLimitPercent}%
        </span>
      </SettingRow>
      <SettingRow label={t("settings.ai.gpu")} desc={t("settings.ai.gpuDesc")}>
        <Toggle
          checked={settings.ai.useGpu}
          label={t("settings.ai.gpu")}
          testId="ai-toggle-gpu"
          onChange={(next) => commit({ ai: { useGpu: next } })}
        />
      </SettingRow>
    </>
  );
}

export default function AiTab() {
  const { t } = useTranslation();
  const settings = useSettingsStore((s) => s.settings);
  const models = useAiStore((s) => s.models) ?? [];
  const modelsLoaded = useAiStore((s) => s.modelsLoaded);
  const refresh = useAiStore((s) => s.refresh);

  // 事件订阅在应用启动时已挂（initAi）；进入 tab 拉一次快照
  useEffect(() => {
    void refresh();
  }, [refresh]);

  // 按能力分组（分组依据=后端清单 feature 字段；空组不渲染卡片）
  const byFeature = new Map<AiFeature, AiModelStatus[]>(
    FEATURE_ORDER.map((feature) => [feature, [] as AiModelStatus[]]),
  );
  for (const model of models) {
    byFeature.get(model.feature)?.push(model);
  }
  /** 功能门控（三档化）：语义=当前档语义三件；人脸=当前档检测件+arcface */
  const currentTier: QualityTier = settings.ai.qualityTier ?? "normal";
  const semanticReady = tierSemanticReady(currentTier, models);
  const faceReady = tierFaceReady(currentTier, models);

  // 人脸数据清除：两步强确认
  const [faceConfirm, setFaceConfirm] = useState(false);
  const [faceClearing, setFaceClearing] = useState(false);

  return (
    <>
      {/* 画质档位选择器（快速/普通/精准；显眼置顶） */}
      <QualityTierSection />

      {/* 模型状态区（按 feature 分组，组内逐模型行） */}
      <SectionTitle>{t("settings.section.ai")}</SectionTitle>
      <div data-testid="ai-model-list">
        {!modelsLoaded ? (
          <p className="py-3 text-[11px] text-text-muted">{t("settings.ai.model.loading")}</p>
        ) : models.length === 0 ? (
          <p className="py-3 text-[11px] text-text-muted">{t("gallery.ipcUnavailable")}</p>
        ) : (
          FEATURE_ORDER.map((feature) => {
            const groupModels = byFeature.get(feature) ?? [];
            return groupModels.length > 0 ? (
              <ModelGroupCard key={feature} feature={feature} models={groupModels} />
            ) : null;
          })
        )}
      </div>

      {/* 区块：状态、立即索引、重建（切档后的重建进度同样走此通道/任务抽屉） */}
      <IndexStatusSection />

      {/* 高级调参（折叠分组，开发人员配置开关门控） */}
      <AdvancedSection />

      {/* 功能开关（模型门控：按当前档所需件） */}
      <SectionTitle>{t("settings.section.aiFeatures")}</SectionTitle>
      <SettingRow
        label={t("settings.ai.semantic")}
        desc={semanticReady ? t("settings.ai.semanticDesc") : t("settings.ai.gateHint")}
      >
        <Toggle
          checked={settings.ai.enableClip}
          disabled={!semanticReady}
          label={t("settings.ai.semantic")}
          testId="ai-toggle-semantic"
          onChange={(next) => commit({ ai: { enableClip: next } })}
        />
      </SettingRow>
      <SettingRow
        label={t("settings.ai.face")}
        desc={faceReady ? undefined : t("settings.ai.gateHint")}
      >
        <Toggle
          checked={settings.ai.enableFace}
          disabled={!faceReady}
          label={t("settings.ai.face")}
          testId="ai-toggle-face"
          onChange={(next) => commit({ ai: { enableFace: next } })}
        />
      </SettingRow>
      <SettingRow label={t("settings.ai.scene")}>
        <Toggle
          checked={settings.ai.enableSceneTags}
          disabled
          label={t("settings.ai.scene")}
          onChange={() => {}}
        />
      </SettingRow>

      {/* 人脸数据一键清除（红色强确认） */}
      <SectionTitle>{t("settings.section.aiData")}</SectionTitle>
      <div className="flex items-center justify-between gap-8 py-2">
        <span className="text-xs text-text-primary">{t("settings.ai.faceClear")}</span>
        {faceConfirm ? (
          <span className="flex items-center gap-1.5">
            <span className="text-[11px] text-red-400">{t("settings.ai.faceClearConfirm")}</span>
            <button
              type="button"
              onClick={async () => {
                setFaceClearing(true);
                await aiFaceDataClear();
                setFaceClearing(false);
                setFaceConfirm(false);
              }}
              disabled={faceClearing}
              className="rounded-md bg-red-500/90 px-2.5 py-1 text-[11px] font-medium text-white transition-colors hover:bg-red-500 disabled:opacity-50"
              data-testid="ai-face-clear-confirm"
            >
              {t("settings.ai.faceClearGo")}
            </button>
            <button
              type="button"
              onClick={() => setFaceConfirm(false)}
              className="rounded-md border border-edge px-2 py-1 text-[11px] text-text-secondary"
              data-testid="ai-face-clear-cancel"
            >
              {t("common.cancel")}
            </button>
          </span>
        ) : (
          <button
            type="button"
            onClick={() => setFaceConfirm(true)}
            className="rounded-md border border-red-400/60 px-2.5 py-1 text-[11px] text-red-400 transition-colors hover:bg-red-400/10"
            data-testid="ai-face-clear"
          >
            {t("settings.ai.faceClear")}
          </button>
        )}
      </div>

      <p className="mt-2 text-[11px] leading-relaxed text-text-muted">{t("settings.ai.note")}</p>
    </>
  );
}
