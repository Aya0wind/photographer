import { useEffect, useMemo, useState } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { motion } from "motion/react";

import {
  deviceFiles,
  folderScan,
  fsListDirs,
  importStart,
  isIpcAvailable,
  kindFromName,
  type DeviceKind,
  type FileKind,
  type FsDirEntry,
  type ImportMode,
  type ImportPlan,
} from "@/ipc/api";
import { formatBytes } from "@/lib/format";
import { previewTemplate, unknownTokens, importRootOf } from "@/features/onboarding/onboardingConfig";
import { useSettingsStore } from "@/stores/settingsStore";
import { useImportStore, type RecentSource, type SourceFile } from "@/stores/importStore";

/**
 * 导入向导（LR 式源面板 + A 密度三栏）：
 * 顶部=复制/移动分段模式条；左=设备卡列表 + 文件系统懒加载目录树
 * （点击文件夹名=选中该文件夹为源，folderScan 成 FOLDER: 源）+ 最近使用源
 * + 源文件树（按目录分组折叠，全选/反选）；中=可勾选文件表；右=方案面板
 * （目标根目录/目录模板三预设+自定义实时预览/查重策略/并发流数，MTP 强制 1）。
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

/** 设备类型 → i18n 键（folder=从本地文件夹导入） */
const KIND_LABEL_KEY: Record<DeviceKind, string> = {
  volume: "deviceDialog.kind.reader",
  mtp: "deviceDialog.kind.camera",
  folder: "deviceDialog.kind.folder",
};

/** 文件夹图标（stroke 风格与现有图标一致，16 viewBox） */
function FolderGlyph({ size = 14, className = "" }: { size?: number; className?: string }) {
  return (
    <svg
      viewBox="0 0 16 16"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth="1.3"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={`shrink-0 ${className}`}
      aria-hidden="true"
    >
      <path d="M2 4.75C2 3.78 2.78 3 3.75 3h2.6l1.5 1.75h4.4c.97 0 1.75.78 1.75 1.75v5.75c0 .97-.78 1.75-1.75 1.75h-8.5C2.78 14 2 13.22 2 12.25v-7.5z" />
    </svg>
  );
}

/** Windows 路径宽松比较（大小写/分隔符/尾斜杠归一）——树节点选中高亮用 */
function normalizeFsPath(path: string): string {
  return path.replace(/\//g, "\\").replace(/[\\/]+$/, "").toLowerCase();
}

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
  "w-full rounded-md border border-edge bg-bg px-2.5 py-1.5 font-mono text-xs text-text-primary outline-none transition-colors focus:border-accent";

export default function ImportWizard() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();

  const devices = useImportStore((s) => s.devices);
  const sourceFilesMap = useImportStore((s) => s.sourceFiles);
  const refreshDevice = useImportStore((s) => s.refreshDevice);
  const addDevice = useImportStore((s) => s.addDevice);
  const setSourceFiles = useImportStore((s) => s.setSourceFiles);
  const recentSources = useImportStore((s) => s.recentSources);
  const recordRecentSource = useImportStore((s) => s.recordRecentSource);

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

  // 选中源变化时拉取文件清单（device_files）；已有缓存的源不重复拉取。
  // 大目录（数千文件）枚举在后端完成后一次性返回，此处仅等待并填充。
  const [filesLoading, setFilesLoading] = useState(false);
  useEffect(() => {
    if (!selectedId) return;
    if (sourceFilesMap[selectedId]) return;
    let cancelled = false;
    setFilesLoading(true);
    void deviceFiles(selectedId).then((entries) => {
      if (cancelled) return;
      setFilesLoading(false);
      if (!entries) return; // IPC 失败/不可用：保持空态（预览模式）
      setSourceFiles(
        selectedId,
        entries.map((e) => {
          const slash = e.relPath.lastIndexOf("/");
          const dir = slash >= 0 ? e.relPath.slice(0, slash) : "";
          const name = slash >= 0 ? e.relPath.slice(slash + 1) : e.relPath;
          return { path: e.relPath, dir, name, size: e.size, kind: kindFromName(name) };
        }),
      );
    });
    return () => {
      cancelled = true;
    };
    // sourceFilesMap[selectedId] 变为存在即触发跳过分支，无需进依赖
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedId]);

  const files = useMemo(
    () => (selectedId ? sourceFilesMap[selectedId] ?? [] : []),
    [selectedId, sourceFilesMap],
  );
  const groups = useMemo(() => groupByDir(files), [files]);

  // 当前选中源对应的文件夹路径（树高亮）；设备源为 null
  const selectedFolderPath =
    device?.kind === "folder" ? normalizeFsPath(device.id.slice("FOLDER:".length)) : null;

  // 选择状态（路径集合）；设备切换时重置为全选（默认导入全部）
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  useEffect(() => {
    setSelected(new Set(files.map((f) => f.path)));
    setCollapsed(new Set());
  }, [selectedId, files]);

  // 方案状态（初值来自设置与激活库）：目标根默认 = photoRoot + 导入子目录（应用写入区）
  const [targetRoot, setTargetRoot] = useState("");
  // 激活库 photoRoot 异步就绪后回填（仅在用户未手动输入时）
  useEffect(() => {
    if (photoRoot && !targetRoot) setTargetRoot(importRootOf(photoRoot, importSettings.importSubdir));
  }, [photoRoot, importSettings.importSubdir, targetRoot]);
  const [presetKey, setPresetKey] = useState(() => matchPreset(importSettings.dirTemplate));
  const [customTemplate, setCustomTemplate] = useState(importSettings.dirTemplate);
  const [duplicatePolicy, setDuplicatePolicy] = useState(importSettings.duplicatePolicy);
  const [skipImported, setSkipImported] = useState(importSettings.skipImported);
  const [streams, setStreams] = useState(4);
  const [starting, setStarting] = useState(false);
  const [startError, setStartError] = useState(false);
  // LR 式导入模式（顶部分段条）：复制保留原文件 / 移动纳管
  const [mode, setMode] = useState<ImportMode>("copy");

  // 文件系统懒加载树：根（盘符）+ 每目录子级缓存 + 展开集合
  const [fsRoots, setFsRoots] = useState<FsDirEntry[] | null>(null);
  const [fsChildren, setFsChildren] = useState<Record<string, FsDirEntry[]>>({});
  const [fsExpanded, setFsExpanded] = useState<Set<string>>(() => new Set());
  useEffect(() => {
    let cancelled = false;
    void fsListDirs().then((roots) => {
      if (!cancelled) setFsRoots(roots);
    });
    return () => {
      cancelled = true;
    };
  }, []);

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

  // --- 源选择 -------------------------------------------------------------------

  /** 手动从设备下拉选择：切 URL 并记入最近使用 */
  function selectDevice(id: string): void {
    const snapshot = devices.find((d) => d.id === id);
    if (snapshot) recordRecentSource({ id: snapshot.id, name: snapshot.name, kind: snapshot.kind });
    setSearchParams(id ? { device: id } : {});
  }

  /** 选中文件夹为源：folderScan 成快照则入库并选中；失败（IPC 不可用）静默保持原源 */
  async function selectFolder(path: string): Promise<boolean> {
    const snapshot = await folderScan(path);
    if (!snapshot) return false;
    addDevice(snapshot);
    recordRecentSource({ id: snapshot.id, name: snapshot.name, kind: snapshot.kind });
    setSearchParams({ device: snapshot.id });
    return true;
  }

  /** 树区「浏览…」：系统目录选择器选目录，等价于在树中直接选中该目录 */
  async function browseFolder(): Promise<void> {
    try {
      const dir = await openDialog({ directory: true });
      if (typeof dir === "string" && dir.length > 0) await selectFolder(dir);
    } catch {
      // 非 Tauri 环境或用户取消：静默
    }
  }

  /** 最近使用：文件夹→重扫选中；设备→仍在线则重选 */
  function selectRecent(entry: RecentSource): void {
    if (entry.kind === "folder") {
      void selectFolder(entry.id.slice("FOLDER:".length));
      return;
    }
    if (devices.some((d) => d.id === entry.id)) {
      recordRecentSource(entry);
      setSearchParams({ device: entry.id });
    }
  }

  /** 树节点展开/折叠；首次展开懒加载子级（失败/空显示（空）） */
  async function toggleFsNode(node: FsDirEntry): Promise<void> {
    const path = node.path;
    const isExpanded = fsExpanded.has(path);
    setFsExpanded((prev) => {
      const next = new Set(prev);
      if (isExpanded) next.delete(path);
      else next.add(path);
      return next;
    });
    if (isExpanded) return;
    if (!(path in fsChildren)) {
      const children = await fsListDirs(path);
      setFsChildren((prev) => ({ ...prev, [path]: children }));
    }
  }

  function renderFsNode(node: FsDirEntry, depth: number) {
    const isExpanded = fsExpanded.has(node.path);
    const children = fsChildren[node.path];
    const isSelected =
      selectedFolderPath !== null && normalizeFsPath(node.path) === selectedFolderPath;
    return (
      <div key={node.path}>
        <div
          className={`flex items-center gap-1 border-l-2 py-0.5 pr-1.5 ${
            isSelected ? "border-accent bg-accent/10" : "border-transparent hover:bg-panel/40"
          }`}
          style={{ paddingLeft: 4 + depth * 12 }}
        >
          {node.hasSubdirs ? (
            <button
              type="button"
              onClick={() => void toggleFsNode(node)}
              className="shrink-0 rounded p-0.5 text-text-muted transition-colors hover:text-accent"
              aria-label={t(isExpanded ? "wizard.fs.collapse" : "wizard.fs.expand", {
                dir: node.path,
              })}
              data-testid="wizard-fs-toggle"
            >
              <svg
                viewBox="0 0 16 16"
                width="10"
                height="10"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.6"
                className={`transition-transform ${isExpanded ? "rotate-90" : ""}`}
                aria-hidden="true"
              >
                <path d="M5 3l5 5-5 5" />
              </svg>
            </button>
          ) : (
            <span className="w-3.5 shrink-0" aria-hidden="true" />
          )}
          <button
            type="button"
            onClick={() => void selectFolder(node.path)}
            className={`flex min-w-0 flex-1 items-center gap-1 rounded py-0.5 text-left ${
              isSelected ? "text-accent" : "text-text-secondary"
            }`}
            data-testid="wizard-fs-node"
            data-path={node.path}
            data-selected={isSelected}
            title={node.path}
          >
            <FolderGlyph size={12} className={isSelected ? "text-accent" : "text-text-muted"} />
            <span className="truncate font-mono text-[11px]">{node.name}</span>
          </button>
        </div>
        {node.hasSubdirs && isExpanded && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            transition={{ duration: 0.15, ease: "easeOut" }}
            className="overflow-hidden"
          >
            {children === undefined ? (
              <p className="py-0.5 pl-9 text-[11px] text-text-muted">…</p>
            ) : children.length === 0 ? (
              <p className="py-0.5 pl-9 text-[11px] text-text-muted">{t("wizard.fs.empty")}</p>
            ) : (
              children.map((child) => renderFsNode(child, depth + 1))
            )}
          </motion.div>
        )}
      </div>
    );
  }

  // --- 方案与启动 ----------------------------------------------------------------

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
      mode,
    };
    const jobId = await importStart(plan);
    setStarting(false);
    if (jobId === null) {
      setStartError(true);
      return;
    }
    // 模式随任务记录（事件不含 mode，总结弹窗文案用）
    useImportStore.getState().recordJobMode(jobId, mode);
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
      {/* 页头 + LR 式导入模式分段条 */}
      <header className="flex h-12 shrink-0 items-center gap-3 border-b border-edge px-4">
        <h1 className="text-sm font-semibold text-text-primary">{t("wizard.title")}</h1>
        {!isIpcAvailable() && (
          <span className="rounded bg-panel px-1.5 py-0.5 text-[11px] text-text-muted">
            {t("wizard.ipcUnavailable")}
          </span>
        )}
        <div
          className="ml-auto flex items-center rounded-md border border-edge bg-bg p-0.5"
          role="radiogroup"
          aria-label={t("wizard.mode.label")}
          data-testid="wizard-mode"
        >
          {(["copy", "move"] as const).map((option) => (
            <button
              key={option}
              type="button"
              role="radio"
              aria-checked={mode === option}
              onClick={() => setMode(option)}
              className={`rounded px-3 py-1 text-xs font-medium transition-colors ${
                mode === option
                  ? "bg-accent text-black"
                  : "text-text-secondary hover:text-text-primary"
              }`}
              data-testid={`wizard-mode-${option}`}
            >
              {t(`wizard.mode.${option}`)}
            </button>
          ))}
        </div>
      </header>
      <div className="flex h-7 shrink-0 items-center border-b border-edge px-4">
        <p className="truncate text-[11px] text-text-muted">
          {t(mode === "copy" ? "wizard.mode.copyDesc" : "wizard.mode.moveDesc")}
        </p>
      </div>

      <div className="grid min-h-0 flex-1 grid-cols-[270px_minmax(0,1fr)_320px] gap-2 p-2">
        {/* 左栏：源面板（设备 / 文件系统树 / 最近使用）+ 源文件树 */}
        <section className="flex min-h-0 flex-col overflow-hidden rounded-lg border border-edge bg-surface" aria-label={t("wizard.leftPane")}>
          {/* 设备区 */}
          <div className="shrink-0 border-b border-edge p-3">
            {devices.length === 0 ? (
              <p className="py-4 text-center text-xs leading-relaxed text-text-muted">
                {t("wizard.noDevice")}
              </p>
            ) : (
              <>
                <div className="flex items-center gap-2">
                  <select
                    value={selectedId ?? ""}
                    onChange={(e) => selectDevice(e.target.value)}
                    className="min-w-0 flex-1 rounded-md border border-edge bg-bg px-2 py-1.5 text-xs text-text-primary outline-none focus:border-accent"
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
                    className="shrink-0 rounded-md border border-edge px-2 py-1.5 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
                    title={t("wizard.rescan")}
                  >
                    {t("wizard.rescan")}
                  </button>
                </div>
                {device && (
                  <dl className="mt-3 space-y-1 text-xs" data-testid="wizard-device-info">
                    <div className="flex justify-between">
                      <dt className="text-text-muted">{t("wizard.deviceKind")}</dt>
                      <dd className="flex items-center gap-1 text-text-secondary">
                        {device.kind === "folder" && <FolderGlyph className="text-text-secondary" />}
                        {t(KIND_LABEL_KEY[device.kind])}
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

          {/* 文件系统区：懒加载目录树 */}
          <div className="shrink-0 border-b border-edge py-2" data-testid="wizard-fs">
            <div className="flex items-center justify-between px-3 pb-1">
              <span className="text-xs font-medium text-text-secondary">{t("wizard.fs.title")}</span>
              <button
                type="button"
                onClick={() => void browseFolder()}
                className="flex shrink-0 items-center gap-1 rounded-md border border-edge px-2 py-0.5 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
                data-testid="wizard-fs-browse"
              >
                <FolderGlyph size={12} />
                {t("wizard.fs.browse")}
              </button>
            </div>
            <div className="max-h-44 overflow-y-auto px-1.5 pb-1">
              {fsRoots === null ? null : fsRoots.length === 0 ? (
                <p className="px-2 py-1 text-[11px] leading-relaxed text-text-muted">
                  {t("wizard.fs.unavailable")}
                </p>
              ) : (
                fsRoots.map((node) => renderFsNode(node, 0))
              )}
            </div>
          </div>

          {/* 最近使用区：有记录才显示 */}
          {recentSources.length > 0 && (
            <div className="shrink-0 border-b border-edge py-2" data-testid="wizard-recent">
              <div className="px-3 pb-1 text-xs font-medium text-text-secondary">
                {t("wizard.recent.title")}
              </div>
              <div className="px-1.5">
                {recentSources.map((entry) => (
                  <button
                    key={entry.id}
                    type="button"
                    onClick={() => selectRecent(entry)}
                    className={`flex w-full items-center gap-1.5 rounded px-1.5 py-1 text-left transition-colors hover:bg-panel/40 ${
                      entry.id === selectedId ? "text-accent" : "text-text-secondary"
                    }`}
                    data-testid="wizard-recent-item"
                    title={entry.id}
                  >
                    {entry.kind === "folder" ? (
                      <FolderGlyph size={12} className="text-text-muted" />
                    ) : (
                      <svg
                        viewBox="0 0 16 16"
                        width="12"
                        height="12"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="1.3"
                        className="shrink-0 text-text-muted"
                        aria-hidden="true"
                      >
                        <rect x="2.5" y="3" width="11" height="10" rx="1.5" />
                        <path d="M2.5 10.5l3-3 2.5 2.5" />
                      </svg>
                    )}
                    <span className="truncate text-[11px]">{entry.name}</span>
                  </button>
                ))}
              </div>
            </div>
          )}

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
        <section className="flex min-h-0 flex-col overflow-hidden rounded-lg border border-edge bg-surface" aria-label={t("wizard.fileTable")}>
          <div
            className="flex h-9 shrink-0 items-center justify-between border-b border-edge px-3 text-xs"
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
                {filesLoading && device
                  ? t("wizard.tableLoading")
                  : device
                    ? t("wizard.tableEmpty")
                    : t("wizard.treeEmpty")}
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
                      className={`border-t border-edge/60 ${selected.has(f.path) ? "" : "opacity-40"}`}
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
        <section className="flex min-h-0 flex-col overflow-y-auto rounded-lg border border-edge bg-surface p-3" aria-label={t("wizard.planPane")}>
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
                className="shrink-0 rounded-md border border-edge px-2 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
              >
                {t("wizard.browse")}
              </button>
            </div>
            <p className="text-xs text-text-muted">{t("wizard.targetRootDesc")}</p>
          </div>

          <div className="mt-4 flex flex-col gap-1.5">
            <label htmlFor="wizard.dirTemplate" className="text-xs font-medium text-text-secondary">
              {t("wizard.dirTemplate")}
            </label>
            <select
              id="wizard.dirTemplate"
              value={presetKey}
              onChange={(e) => setPresetKey(e.target.value)}
              className="rounded-md border border-edge bg-bg px-2 py-1.5 text-xs text-text-primary outline-none focus:border-accent"
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
              className="truncate rounded-md border border-edge bg-bg px-2.5 py-1.5 font-mono text-[11px] text-text-secondary"
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
