import { useEffect, useMemo, useState } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import { importStart, isIpcAvailable, type FileKind, type ImportPlan } from "@/ipc/api";
import { formatBytes } from "@/lib/format";
import { previewTemplate, unknownTokens } from "@/features/onboarding/onboardingConfig";
import { useSettingsStore } from "@/stores/settingsStore";
import { useImportStore, type SourceFile } from "@/stores/importStore";

/**
 * 导入向导（A 密度三栏）：左=设备信息+源文件树（按目录分组折叠，全选/反选），
 * 中=可勾选文件表（等宽文件名/大小/类型徽标），右=方案面板（目标根目录/
 * 目录模板（三预设+自定义，实时预览）/查重策略/并发流数（MTP 强制 1））。
 */

interface DirGroup {
  dir: string;
  files: SourceFile[];
}

function groupByDir(files: SourceFile[]): DirGroup[] {
  const map = new Map<string, SourceFile[]>();
  for (const f of files) {
    const list = map.get(f.dir);
    if (list) list.push(f);
    else map.set(f.dir, [f]);
  }
  return [...map.entries()]
    .sort((a, b) => a[0].localeCompare(b[0]))
    .map(([dir, list]) => ({
      dir,
      files: [...list].sort((a, b) => a.name.localeCompare(b.name)),
    }));
}

const KIND_LABEL_COLOR: Record<FileKind, string> = {
  photo: "text-accent",
  raw: "text-sky-400",
  video: "text-violet-400",
  other: "text-text-muted",
};

function KindBadge({ kind }: { kind: FileKind }) {
  const { t } = useTranslation();
  return (
    <span
      className={`rounded bg-bg px-1.5 py-0.5 text-[11px] font-medium ${KIND_LABEL_COLOR[kind]}`}
      data-kind={kind}
    >
      {t(`wizard.fileKind.${kind}`)}
    </span>
  );
}

/** 目录模板预设（值即模板；custom 为自定义输入） */
const TEMPLATE_PRESETS = [
  { key: "ymd", value: "{YYYY}/{MM-DD}" },
  { key: "ym", value: "{YYYY}/{MM}" },
  { key: "orig", value: "{原目录}" },
] as const;

/** 去掉尾部 /{原文件名} 后匹配预设（设置里存的模板常带文件名令牌） */
function matchPreset(template: string): string {
  const dirPart = template.replace(/\/?\{原文件名\}\s*$/, "");
  const hit = TEMPLATE_PRESETS.find((p) => p.value === dirPart);
  return hit ? hit.key : "custom";
}

function presetValue(key: string, custom: string): string {
  const hit = TEMPLATE_PRESETS.find((p) => p.key === key);
  return hit ? hit.value : custom;
}

const inputClass =
  "w-full rounded-md border border-panel bg-bg px-2.5 py-1.5 font-mono text-xs text-text-primary outline-none transition-colors focus:border-accent";

export default function ImportWizard() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();

  const devices = useImportStore((s) => s.devices);
  const sourceFilesMap = useImportStore((s) => s.sourceFiles);
  const refreshDevice = useImportStore((s) => s.refreshDevice);

  const importSettings = useSettingsStore((s) => s.settings.import);
  const photoRoot = useSettingsStore(
    (s) =>
      s.settings.libraries.find((lib) => lib.id === s.settings.activeLibraryId)?.photoRoot ?? "",
  );

  // 设备选择：URL ?device= 优先，回落第一台
  const urlDevice = searchParams.get("device");
  const selectedId = devices.some((d) => d.id === urlDevice)
    ? (urlDevice as string)
    : (devices[0]?.id ?? null);
  const device = devices.find((d) => d.id === selectedId) ?? null;
  const files = useMemo(
    () => (selectedId ? sourceFilesMap[selectedId] ?? [] : []),
    [selectedId, sourceFilesMap],
  );
  const groups = useMemo(() => groupByDir(files), [files]);

  // 选择状态（路径集合）；设备切换时重置为全选（默认导入全部）
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  useEffect(() => {
    setSelected(new Set(files.map((f) => f.path)));
    setCollapsed(new Set());
  }, [selectedId, files]);

  // 方案状态（初值来自设置与激活库）
  const [targetRoot, setTargetRoot] = useState(photoRoot);
  // 激活库 photoRoot 异步就绪后回填（仅在用户未手动输入时）
  useEffect(() => {
    if (photoRoot && !targetRoot) setTargetRoot(photoRoot);
  }, [photoRoot, targetRoot]);
  const [presetKey, setPresetKey] = useState(() => matchPreset(importSettings.dirTemplate));
  const [customTemplate, setCustomTemplate] = useState(importSettings.dirTemplate);
  const [duplicatePolicy, setDuplicatePolicy] = useState(importSettings.duplicatePolicy);
  const [skipImported, setSkipImported] = useState(importSettings.skipImported);
  const [streams, setStreams] = useState(4);
  const [starting, setStarting] = useState(false);
  const [startError, setStartError] = useState(false);

  const dirTemplate = presetValue(presetKey, customTemplate);
  const isMtp = device?.kind === "mtp";
  const effectiveStreams = isMtp ? 1 : streams;
  const badTokens = presetKey === "custom" ? unknownTokens(customTemplate) : [];
  const canStart = Boolean(device && targetRoot.trim()) && badTokens.length === 0 && !starting;

  const selectedCount = selected.size;
  const selectedBytes = files
    .filter((f) => selected.has(f.path))
    .reduce((sum, f) => sum + f.size, 0);

  function toggleFile(path: string): void {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }

  function toggleGroup(group: DirGroup): void {
    setSelected((prev) => {
      const next = new Set(prev);
      const allIn = group.files.every((f) => next.has(f.path));
      for (const f of group.files) {
        if (allIn) next.delete(f.path);
        else next.add(f.path);
      }
      return next;
    });
  }

  function toggleCollapse(dir: string): void {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(dir)) next.delete(dir);
      else next.add(dir);
      return next;
    });
  }

  function selectAll(): void {
    setSelected(new Set(files.map((f) => f.path)));
  }

  function invertSelection(): void {
    setSelected((prev) => new Set(files.filter((f) => !prev.has(f.path)).map((f) => f.path)));
  }

  async function pickTargetRoot(): Promise<void> {
    try {
      const dir = await openDialog({ directory: true, defaultPath: targetRoot || undefined });
      if (typeof dir === "string" && dir.length > 0) setTargetRoot(dir);
    } catch {
      // 非 Tauri 环境或用户取消：保持现状
    }
  }

  async function startImport(): Promise<void> {
    if (!device || !canStart) return;
    setStarting(true);
    setStartError(false);
    const plan: ImportPlan = {
      sourceId: device.id,
      targetRoot: targetRoot.trim(),
      dirTemplate,
      nameTemplate: "{原文件名}",
      duplicatePolicy,
      skipImported,
      streams: effectiveStreams,
    };
    const jobId = await importStart(plan);
    setStarting(false);
    if (jobId === null) {
      setStartError(true);
      return;
    }
    // 方案回写设置（本地立即生效，持久化失败静默）
    const { update, save } = useSettingsStore.getState();
    const settings = useSettingsStore.getState().settings;
    update({
      import: {
        ...settings.import,
        dirTemplate,
        duplicatePolicy,
        skipImported,
      },
    });
    void save(useSettingsStore.getState().settings);
    navigate("/tasks");
  }

  return (
    <div className="flex h-full flex-col">
      {/* 页头 */}
      <header className="flex h-12 shrink-0 items-center gap-3 border-b border-panel px-4">
        <h1 className="text-sm font-semibold text-text-primary">{t("wizard.title")}</h1>
        {!isIpcAvailable() && (
          <span className="rounded bg-panel px-1.5 py-0.5 text-[11px] text-text-muted">
            {t("wizard.ipcUnavailable")}
          </span>
        )}
      </header>

      <div className="grid min-h-0 flex-1 grid-cols-[270px_minmax(0,1fr)_320px] gap-2 p-2">
        {/* 左栏：设备信息 + 源文件树 */}
        <section className="flex min-h-0 flex-col overflow-hidden rounded-lg border border-panel bg-surface" aria-label={t("wizard.leftPane")}>
          <div className="shrink-0 border-b border-panel p-3">
            {devices.length === 0 ? (
              <p className="py-6 text-center text-xs leading-relaxed text-text-muted">
                {t("wizard.noDevice")}
              </p>
            ) : (
              <>
                <div className="flex items-center gap-2">
                  <select
                    value={selectedId ?? ""}
                    onChange={(e) => setSearchParams(e.target.value ? { device: e.target.value } : {})}
                    className="min-w-0 flex-1 rounded-md border border-panel bg-bg px-2 py-1.5 text-xs text-text-primary outline-none focus:border-accent"
                    aria-label={t("wizard.deviceSelect")}
                  >
                    {devices.map((d) => (
                      <option key={d.id} value={d.id}>
                        {d.name}
                      </option>
                    ))}
                  </select>
                  <button
                    type="button"
                    onClick={() => device && void refreshDevice(device.id)}
                    className="shrink-0 rounded-md border border-panel px-2 py-1.5 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
                    title={t("wizard.rescan")}
                  >
                    {t("wizard.rescan")}
                  </button>
                </div>
                {device && (
                  <dl className="mt-3 space-y-1 text-xs" data-testid="wizard-device-info">
                    <div className="flex justify-between">
                      <dt className="text-text-muted">{t("wizard.deviceKind")}</dt>
                      <dd className="text-text-secondary">
                        {t(device.kind === "mtp" ? "deviceDialog.kind.camera" : "deviceDialog.kind.reader")}
                      </dd>
                    </div>
                    {(Object.keys(device.filesByKind) as FileKind[]).map((kind) => (
                      <div key={kind} className="flex justify-between">
                        <dt className="text-text-muted">{t(`wizard.fileKind.${kind}`)}</dt>
                        <dd className={`font-mono tabular-nums ${KIND_LABEL_COLOR[kind]}`}>
                          {device.filesByKind[kind]}
                        </dd>
                      </div>
                    ))}
                    <div className="flex justify-between">
                      <dt className="text-text-muted">{t("wizard.totalSize")}</dt>
                      <dd className="font-mono text-text-secondary">{formatBytes(device.bytesTotal)}</dd>
                    </div>
                    <div className="flex justify-between">
                      <dt className="text-text-muted">{t("wizard.newFiles")}</dt>
                      <dd className="font-mono text-accent">{device.newFiles}</dd>
                    </div>
                  </dl>
                )}
              </>
            )}
          </div>

          {/* 源文件树 */}
          <div className="flex min-h-0 flex-1 flex-col">
            <div className="flex shrink-0 items-center justify-between px-3 py-2">
              <span className="text-xs font-medium text-text-secondary">{t("wizard.sourceTree")}</span>
              <div className="flex gap-1.5">
                <button type="button" onClick={selectAll} className="text-[11px] text-text-muted transition-colors hover:text-accent">
                  {t("wizard.selectAll")}
                </button>
                <button type="button" onClick={invertSelection} className="text-[11px] text-text-muted transition-colors hover:text-accent">
                  {t("wizard.invert")}
                </button>
              </div>
            </div>
            <div className="min-h-0 flex-1 overflow-y-auto px-1.5 pb-2" data-testid="wizard-tree">
              {groups.length === 0 ? (
                <p className="px-2 py-4 text-xs leading-relaxed text-text-muted">
                  {t("wizard.treeEmpty")}
                </p>
              ) : (
                groups.map((group) => {
                  const allIn = group.files.every((f) => selected.has(f.path));
                  const someIn = group.files.some((f) => selected.has(f.path));
                  const isCollapsed = collapsed.has(group.dir);
                  return (
                    <div key={group.dir} className="mb-0.5">
                      <div className="flex items-center gap-1.5 rounded px-1.5 py-1 hover:bg-panel/40">
                        <input
                          type="checkbox"
                          checked={allIn}
                          ref={(el) => {
                            if (el) el.indeterminate = !allIn && someIn;
                          }}
                          onChange={() => toggleGroup(group)}
                          aria-label={t("wizard.groupToggle", { dir: group.dir })}
                          className="h-3 w-3 shrink-0 accent-[#F0A83C]"
                        />
                        <button
                          type="button"
                          onClick={() => toggleCollapse(group.dir)}
                          className="flex min-w-0 flex-1 items-center gap-1 text-left"
                        >
                          <svg
                            viewBox="0 0 16 16"
                            width="10"
                            height="10"
                            fill="none"
                            stroke="currentColor"
                            strokeWidth="1.6"
                            className={`shrink-0 text-text-muted transition-transform ${isCollapsed ? "" : "rotate-90"}`}
                            aria-hidden="true"
                          >
                            <path d="M5 3l5 5-5 5" />
                          </svg>
                          <span className="truncate font-mono text-[11px] text-text-secondary" title={group.dir}>
                            {group.dir}
                          </span>
                          <span className="ml-auto shrink-0 text-[11px] text-text-muted">
                            {group.files.length}
                          </span>
                        </button>
                      </div>
                      {!isCollapsed &&
                        group.files.map((f) => (
                          <label
                            key={f.path}
                            className="flex cursor-pointer items-center gap-1.5 rounded py-0.5 pl-7 pr-1.5 hover:bg-panel/40"
                          >
                            <input
                              type="checkbox"
                              checked={selected.has(f.path)}
                              onChange={() => toggleFile(f.path)}
                              className="h-3 w-3 shrink-0 accent-[#F0A83C]"
                            />
                            <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-text-secondary" title={f.name}>
                              {f.name}
                            </span>
                          </label>
                        ))}
                    </div>
                  );
                })
              )}
            </div>
          </div>
        </section>

        {/* 中栏：可勾选文件表 */}
        <section className="flex min-h-0 flex-col overflow-hidden rounded-lg border border-panel bg-surface" aria-label={t("wizard.fileTable")}>
          <div
            className="flex h-9 shrink-0 items-center justify-between border-b border-panel px-3 text-xs"
            data-testid="wizard-table-stats"
          >
            <span className="text-text-secondary">
              {t("wizard.selectedStats", {
                selected: selectedCount,
                total: files.length,
                size: formatBytes(selectedBytes),
              })}
            </span>
            <span className="text-text-muted">{t("wizard.fileColumns")}</span>
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto">
            {files.length === 0 ? (
              <p className="px-6 py-10 text-center text-xs leading-relaxed text-text-muted">
                {t("wizard.tableEmpty")}
              </p>
            ) : (
              <table className="w-full table-fixed border-collapse text-xs">
                <thead>
                  <tr className="text-left text-[11px] text-text-muted">
                    <th className="w-8 px-2 py-1.5 font-normal" aria-label={t("wizard.columnPick")} />
                    <th className="px-2 py-1.5 font-normal">{t("wizard.columnName")}</th>
                    <th className="w-20 px-2 py-1.5 text-right font-normal">{t("wizard.columnSize")}</th>
                    <th className="w-16 px-2 py-1.5 font-normal">{t("wizard.columnKind")}</th>
                  </tr>
                </thead>
                <tbody>
                  {files.map((f) => (
                    <tr
                      key={f.path}
                      className={`border-t border-panel/60 ${selected.has(f.path) ? "" : "opacity-40"}`}
                    >
                      <td className="px-2 py-1">
                        <input
                          type="checkbox"
                          checked={selected.has(f.path)}
                          onChange={() => toggleFile(f.path)}
                          aria-label={f.name}
                          className="h-3 w-3 accent-[#F0A83C]"
                        />
                      </td>
                      <td className="truncate px-2 py-1 font-mono text-[11px] text-text-primary" title={f.path}>
                        {f.dir ? `${f.dir}/` : ""}
                        {f.name}
                      </td>
                      <td className="px-2 py-1 text-right font-mono text-[11px] text-text-secondary tabular-nums">
                        {formatBytes(f.size)}
                      </td>
                      <td className="px-2 py-1">
                        <KindBadge kind={f.kind} />
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        </section>

        {/* 右栏：方案面板 */}
        <section className="flex min-h-0 flex-col overflow-y-auto rounded-lg border border-panel bg-surface p-3" aria-label={t("wizard.planPane")}>
          <h2 className="text-xs font-semibold text-text-primary">{t("wizard.plan")}</h2>

          <div className="mt-3 flex flex-col gap-1.5">
            <label htmlFor="wizard.targetRoot" className="text-xs font-medium text-text-secondary">
              {t("wizard.targetRoot")}
            </label>
            <div className="flex gap-1.5">
              <input
                id="wizard.targetRoot"
                type="text"
                value={targetRoot}
                onChange={(e) => setTargetRoot(e.target.value)}
                className={inputClass}
              />
              <button
                type="button"
                onClick={() => void pickTargetRoot()}
                className="shrink-0 rounded-md border border-panel px-2 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
              >
                {t("wizard.browse")}
              </button>
            </div>
          </div>

          <div className="mt-4 flex flex-col gap-1.5">
            <label htmlFor="wizard.dirTemplate" className="text-xs font-medium text-text-secondary">
              {t("wizard.dirTemplate")}
            </label>
            <select
              id="wizard.dirTemplate"
              value={presetKey}
              onChange={(e) => setPresetKey(e.target.value)}
              className="rounded-md border border-panel bg-bg px-2 py-1.5 text-xs text-text-primary outline-none focus:border-accent"
            >
              {TEMPLATE_PRESETS.map((p) => (
                <option key={p.key} value={p.key}>
                  {t(`wizard.template.${p.key}`)}
                </option>
              ))}
              <option value="custom">{t("wizard.template.custom")}</option>
            </select>
            {presetKey === "custom" && (
              <input
                type="text"
                value={customTemplate}
                onChange={(e) => setCustomTemplate(e.target.value)}
                placeholder="{YYYY}/{MM}"
                className={inputClass}
                aria-label={t("wizard.template.custom")}
              />
            )}
            <div
              className="truncate rounded-md border border-panel bg-bg px-2.5 py-1.5 font-mono text-[11px] text-text-secondary"
              title={previewTemplate(dirTemplate, targetRoot)}
              data-testid="wizard-preview"
            >
              {previewTemplate(dirTemplate, targetRoot)}
            </div>
            {badTokens.length > 0 && (
              <p className="text-[11px] text-red-400" role="alert">
                {t("onboarding.scheme.unknownToken", {
                  tokens: badTokens.map((token) => `{${token}}`).join(" "),
                })}
              </p>
            )}
          </div>

          <fieldset className="mt-4 flex flex-col gap-1">
            <legend className="mb-1 text-xs font-medium text-text-secondary">
              {t("wizard.duplicatePolicy")}
            </legend>
            {(["skip", "rename", "ask"] as const).map((option) => (
              <label key={option} className="flex cursor-pointer items-center gap-2 text-xs text-text-secondary">
                <input
                  type="radio"
                  name="wizard.duplicatePolicy"
                  value={option}
                  checked={duplicatePolicy === option}
                  onChange={() => setDuplicatePolicy(option)}
                  className="h-3 w-3 accent-[#F0A83C]"
                />
                {t(`onboarding.scheme.dup.${option}`)}
              </label>
            ))}
          </fieldset>

          <label className="mt-4 flex cursor-pointer items-center gap-2 text-xs text-text-secondary">
            <input
              type="checkbox"
              checked={skipImported}
              onChange={(e) => setSkipImported(e.target.checked)}
              className="h-3 w-3 accent-[#F0A83C]"
            />
            {t("wizard.skipImported")}
          </label>

          <div className="mt-4 flex flex-col gap-1.5">
            <label htmlFor="wizard.streams" className="text-xs font-medium text-text-secondary">
              {t("wizard.streams")}
            </label>
            <div className="flex items-center gap-2">
              <input
                id="wizard.streams"
                type="range"
                min={1}
                max={8}
                step={1}
                value={effectiveStreams}
                disabled={isMtp}
                onChange={(e) => setStreams(Number(e.target.value))}
                className="h-1 flex-1 accent-[#F0A83C] disabled:opacity-40"
              />
              <span className="w-8 text-right font-mono text-xs text-text-primary tabular-nums" data-testid="wizard-streams-value">
                {effectiveStreams}
              </span>
            </div>
            {isMtp && <p className="text-[11px] text-text-muted">{t("wizard.mtpSingleStream")}</p>}
          </div>

          <div className="mt-auto pt-4">
            <button
              type="button"
              disabled={!canStart}
              onClick={() => void startImport()}
              className="w-full rounded-md bg-accent px-4 py-2 text-sm font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
            >
              {starting ? t("wizard.starting") : t("wizard.start")}
            </button>
            {startError && (
              <p className="mt-2 text-[11px] text-red-400" role="alert">
                {t("wizard.startError")}
              </p>
            )}
          </div>
        </section>
      </div>
    </div>
  );
}
