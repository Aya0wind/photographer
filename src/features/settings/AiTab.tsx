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
  type AiModelStatus,
  type IndexKind,
  type IndexStatus,
  type RebuildKind,
} from "@/ipc/api";
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
 * 设置页 AI tab（M4 实化）：
 * - 模型状态区：4 行（siglip2-visual/text、scrfd、arcface）名称/体积/状态/操作
 *   （下载/取消/重试/删除释放磁盘），下载进度条走 aiModelDownloadProgress 事件
 * - 索引状态与操作区：三类索引（缩略图/EXIF/语义）计数 + 立即索引（indexKickNow，
 *   幂等；ai 模型未就绪透传后端 Err 文案）；index_status 进 tab 拉一次 +
 *   indexTaskProgress 事件驱动重拉（aiStore）。运行态从持久化 indexStatus
 *   派生（pending/running>0）——切页重挂载/重启后状态保留，不丢「进行中」
 * - 功能开关门控：语义=两个 siglip2 都 done；人脸=scrfd+arcface 都 done
 * - 调度/CPU 滑条（仅用于 AI 推理）/GPU（DirectML 自动回退）
 * - 人脸数据一键清除（红色强确认，两步确认防误触）
 */

/** 修改即存（与 SettingsPage 主组件同语义） */
function commit(partial: DeepPartial<Settings>): void {
  const { update, save } = useSettingsStore.getState();
  update(partial);
  void save(useSettingsStore.getState().settings);
}

/** 模型显示名（未知 id 回退原 id） */
const MODEL_NAMES: Record<string, string> = {
  "siglip2-visual": "语义 · 图像编码",
  "siglip2-text": "语义 · 文本编码",
  scrfd: "人脸 · 检测",
  arcface: "人脸 · 识别",
};

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

function ModelRow({ model }: { model: AiModelStatus }) {
  const { t } = useTranslation();
  const downloadProgress = useAiStore((s) => s.downloadProgress[model.id]);
  const refresh = useAiStore((s) => s.refresh);
  const [confirmDelete, setConfirmDelete] = useState(false);

  // 下载中：事件进度优先（节流 1s），无事件时退模型快照字段
  const progress =
    downloadProgress ??
    (model.state === "downloading" && model.bytesTotal > 0
      ? { doneBytes: model.downloadedBytes, totalBytes: model.bytesTotal }
      : null);
  const pct =
    progress && progress.totalBytes > 0
      ? Math.min(100, (progress.doneBytes / progress.totalBytes) * 100)
      : 0;

  return (
    <div
      className="flex min-h-[44px] items-center justify-between gap-4 border-b border-edge/40 py-2"
      data-testid={`ai-model-row`}
      data-model-id={model.id}
      data-state={model.state}
    >
      <div className="min-w-0">
        <div className="flex items-center gap-2 text-xs text-text-primary">
          <span className="truncate">{MODEL_NAMES[model.id] ?? model.id}</span>
          <StateBadge model={model} />
          {model.state === "done" && model.version && (
            <span className="font-mono text-[10px] text-text-muted">{model.version}</span>
          )}
        </div>
        <div className="mt-0.5 text-[11px] text-text-muted">
          {formatBytes(model.bytesTotal)}
          {progress && (
            <span className="ml-2 font-mono tabular-nums">
              {formatBytes(progress.doneBytes)} / {formatBytes(progress.totalBytes)}
            </span>
          )}
        </div>
        {progress && model.state === "downloading" && (
          <div className="mt-1 h-1 w-48 overflow-hidden rounded bg-panel" data-testid={`ai-model-progress-${model.id}`}>
            <div className="h-full bg-accent transition-[width] duration-300" style={{ width: `${pct}%` }} />
          </div>
        )}
      </div>
      <div className="flex shrink-0 items-center gap-2">
        {(model.state === "idle" || model.state === "failed") && (
          <button
            type="button"
            onClick={() => {
              void aiModelDownload(model.id);
              void refresh();
            }}
            className="rounded-md bg-accent px-2.5 py-1 text-[11px] font-medium text-black transition-colors hover:brightness-110"
            data-testid={`ai-model-download-${model.id}`}
          >
            {model.state === "failed"
              ? t("settings.ai.model.retry")
              : t("settings.ai.model.download")}
          </button>
        )}
        {model.state === "downloading" && (
          <button
            type="button"
            onClick={() => {
              void aiModelCancel(model.id);
              void refresh();
            }}
            className="rounded-md border border-edge px-2.5 py-1 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
            data-testid={`ai-model-cancel-${model.id}`}
          >
            {t("settings.ai.model.cancel")}
          </button>
        )}
        {model.state === "done" &&
          (confirmDelete ? (
            <span className="flex items-center gap-1.5">
              <button
                type="button"
                onClick={() => {
                  setConfirmDelete(false);
                  void aiModelDelete(model.id);
                  void refresh();
                }}
                className="rounded-md bg-red-500/90 px-2.5 py-1 text-[11px] font-medium text-white transition-colors hover:bg-red-500"
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
              className="rounded-md border border-edge px-2.5 py-1 text-[11px] text-text-secondary transition-colors hover:border-red-400 hover:text-red-400"
              data-testid={`ai-model-delete-${model.id}`}
            >
              {t("settings.ai.model.delete")}
            </button>
          ))}
      </div>
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
          const complete = c.total === 0 || (c.done >= c.total && c.failed === 0);
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
      <SectionTitle>{t("settings.ai.paramsSection")}</SectionTitle>
      <AiParamsSection />
      <SectionTitle>{t("settings.ai.burstSection")}</SectionTitle>
      <BurstSection />
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

  const byId = new Map(models.map((m) => [m.id, m]));
  const semanticReady =
    byId.get("siglip2-visual")?.state === "done" && byId.get("siglip2-text")?.state === "done";
  const faceReady = byId.get("scrfd")?.state === "done" && byId.get("arcface")?.state === "done";

  // 人脸数据清除：两步强确认
  const [faceConfirm, setFaceConfirm] = useState(false);
  const [faceClearing, setFaceClearing] = useState(false);

  return (
    <>
      <SectionTitle>{t("settings.section.ai")}</SectionTitle>

      {/* 模型状态区 */}
      <div data-testid="ai-model-list">
        {!modelsLoaded ? (
          <p className="py-3 text-[11px] text-text-muted">{t("settings.ai.model.loading")}</p>
        ) : models.length === 0 ? (
          <p className="py-3 text-[11px] text-text-muted">{t("gallery.ipcUnavailable")}</p>
        ) : (
          models.map((model) => <ModelRow key={model.id} model={model} />)
        )}
      </div>

      {/* 单一区块：状态、立即索引、重建与索引参数 */}
      <IndexStatusSection />

      {/* 功能开关（模型门控） */}
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
      {/* 语义相似度阈值：低于该分的结果过滤（0 = 不过滤）。
          实测 SigLIP2 cos 区间压缩：无关内容 top≈0.087、相关簇≈0.099+，
          阈值过高全灭、过低任何查询返回 top-N≈全库（进哪个智能相册都是
          全部照片的真机复现根因） */}
      <SettingRow
        label={t("settings.ai.semanticThreshold")}
        desc={t("settings.ai.semanticThresholdDesc")}
      >
        <span className="flex items-center gap-2">
          <input
            type="number"
            min={0}
            max={1}
            step={0.01}
            value={settings.ai.semanticMinScore}
            onChange={(e) => {
              const v = Number(e.target.value);
              if (Number.isFinite(v)) {
                commit({ ai: { semanticMinScore: Math.min(1, Math.max(0, v)) } });
              }
            }}
            aria-label={t("settings.ai.semanticThreshold")}
            className="w-20 rounded border border-edge bg-bg px-2 py-1 font-mono text-xs text-text-primary outline-none focus:border-accent"
            data-testid="ai-semantic-threshold"
          />
          <button
            type="button"
            onClick={() => commit({ ai: { semanticMinScore: 0.09 } })}
            className="rounded border border-edge px-2 py-1 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
            data-testid="ai-semantic-threshold-reset"
          >
            {t("settings.ai.semanticThresholdReset")}
          </button>
        </span>
      </SettingRow>

      {/* 调度 / 资源 */}
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
