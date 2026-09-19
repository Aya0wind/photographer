import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  aiFaceDataClear,
  aiModelCancel,
  aiModelDelete,
  aiModelDownload,
  type AiModelStatus,
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
