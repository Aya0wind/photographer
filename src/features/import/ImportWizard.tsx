import { useEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { convertFileSrc } from "@tauri-apps/api/core";
import { motion } from "motion/react";
import { useVirtualizer } from "@tanstack/react-virtual";

import {
  deviceFiles,
  folderScan,
  fsListDirs,
  importStart,
  isIpcAvailable,
  kindFromName,
  type DeviceKind,
  type DeviceSnapshot,
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
 * + 源文件树；中=文件区双视图（列表默认 / 缩略图网格，右栏切换并持久化，
 * 两视图均虚拟化、共享勾选语义与统计条）；右=方案面板（查看方式/目标根目录/
 * 目录模板三预设+自定义实时预览/查重策略/并发流数，MTP 强制 1）。
 *
 * 缩略图：photo 走 asset 协议（convertFileSrc，folder=去前缀路径/volume=设备id
 * + relPath，MTP 无文件系统路径恒占位），信号量限 6 张在途解码，onLoad 150ms
 * 淡入，失败/超时静默保持占位；RAW/视频恒占位（M3 缩略图管线前不做内嵌提取）。
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

// --- 查看方式（列表默认 / 缩略图），localStorage 持久化 ---------------------------

export type WizardViewMode = "list" | "grid";

export const VIEW_MODE_STORAGE_KEY = "smartphoto.import.viewMode";

function loadViewMode(): WizardViewMode {
  try {
    return localStorage.getItem(VIEW_MODE_STORAGE_KEY) === "grid" ? "grid" : "list";
  } catch {
    return "list";
  }
}

function saveViewMode(mode: WizardViewMode): void {
  try {
    localStorage.setItem(VIEW_MODE_STORAGE_KEY, mode);
  } catch {
    // 存储不可用时仅内存态生效
  }
}

// --- 缩略图管线：asset 协议 + 解码并发信号量 --------------------------------------

/** 源根的文件系统绝对路径（folder=去 FOLDER: 前缀；volume=设备 id；MTP 无路径） */
function sourceBasePath(device: DeviceSnapshot | null): string | null {
  if (!device) return null;
  if (device.kind === "folder") return device.id.slice("FOLDER:".length);
  if (device.kind === "volume") return device.id;
  return null;
}

/** 源内相对路径（dir + name 统一拼接，与设备枚举的 relPath 语义一致） */
function relPathOf(file: SourceFile): string {
  return file.dir ? `${file.dir}/${file.name}` : file.name;
}

/** absPath → asset 协议 URL；非 Tauri 环境抛错或空结果回退 null（占位） */
function toAssetUrl(base: string | null, relPath: string): string | null {
  if (base === null) return null;
  try {
    return convertFileSrc(`${base}/${relPath}`) || null;
  } catch {
    return null;
  }
}

/** 同一时刻在途解码上限：超出排队，避免大目录一次性打爆 IO/解码 */
const IMAGE_LOAD_CONCURRENCY = 6;
let activeImageLoads = 0;
const imageSlotQueue: Array<() => void> = [];

function acquireImageSlot(): Promise<void> {
  if (activeImageLoads < IMAGE_LOAD_CONCURRENCY) {
    activeImageLoads += 1;
    return Promise.resolve();
  }
  return new Promise((resolve) => {
    imageSlotQueue.push(() => {
      activeImageLoads += 1;
      resolve();
    });
  });
}

function releaseImageSlot(): void {
  activeImageLoads = Math.max(0, activeImageLoads - 1);
  const next = imageSlotQueue.shift();
  if (next) next();
}

function extOf(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot >= 0 ? name.slice(dot + 1).toUpperCase() : "";
}

/** 缩略占位块：surface 底 + kind 色点缀（RAW=扩展名大字+徽标；视频=胶片；photo=图片框） */
function ThumbPlaceholder({ kind, name }: { kind: FileKind; name: string }) {
  const ext = extOf(name);
  if (kind === "raw") {
    return (
      <div className="flex h-full w-full flex-col items-center justify-center gap-1" data-testid="tile-raw">
        <span className="font-mono text-lg font-bold tracking-wide text-sky-400">{ext || "RAW"}</span>
        <span className="rounded bg-bg px-1.5 py-0.5 text-[10px] font-medium text-text-secondary">RAW</span>
      </div>
    );
  }
  if (kind === "video") {
    return (
      <div className="flex h-full w-full flex-col items-center justify-center gap-1.5" data-testid="tile-video">
        <svg
          viewBox="0 0 24 24"
          width="26"
          height="26"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.4"
          strokeLinecap="round"
          strokeLinejoin="round"
          className="text-violet-400"
          aria-hidden="true"
        >
          <rect x="3" y="5" width="18" height="14" rx="2" />
          <path d="M3 9h2M3 15h2M19 9h2M19 15h2" />
          <path d="M10 9.5l5 2.5-5 2.5v-5z" />
        </svg>
        {ext && <span className="font-mono text-[10px] text-text-muted">{ext}</span>}
      </div>
    );
  }
  return (
    <div className="flex h-full w-full flex-col items-center justify-center gap-1.5" data-testid="tile-photo">
      <svg
        viewBox="0 0 24 24"
        width="26"
        height="26"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
        strokeLinejoin="round"
        className="text-accent"
        aria-hidden="true"
      >
        <rect x="3.5" y="4.5" width="17" height="15" rx="2" />
        <circle cx="9" cy="10" r="1.8" />
        <path d="M4.5 17l4.5-4.5 3.5 3.5 3-3 4 4" />
      </svg>
      {ext && <span className="font-mono text-[10px] text-text-muted">{ext}</span>}
    </div>
  );
}

/** 单个缩略图块：4:3 照片区 + 信息条；LR 式左上圆形勾选，点块任意处切换 */
function FileTile({
  file,
  selected,
  onToggle,
  assetUrl,
}: {
  file: SourceFile;
  selected: boolean;
  onToggle: (path: string) => void;
  assetUrl: string | null;
}) {
  // 解码槽位到位后才置 src；onLoad 淡入，onError/15s 超时静默保持占位（不重试）
  const [src, setSrc] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const settledRef = useRef(false);
  const releaseRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    let cancelled = false;
    if (assetUrl === null) return;
    void acquireImageSlot().then(() => {
      if (cancelled) {
        releaseImageSlot();
        return;
      }
      releaseRef.current = releaseImageSlot;
      setSrc(assetUrl);
    });
    return () => {
      cancelled = true;
      releaseRef.current?.();
      releaseRef.current = null;
    };
  }, [assetUrl]);

  useEffect(() => {
    if (src === null) return;
    const timer = setTimeout(() => settle(false), 15_000);
    return () => clearTimeout(timer);
    // settle 为渲染闭包但仅触碰 ref/setState，旧闭包安全
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [src]);

  function settle(ok: boolean): void {
    if (settledRef.current) return;
    settledRef.current = true;
    if (ok) setLoaded(true);
    else setSrc(null);
    releaseRef.current?.();
    releaseRef.current = null;
  }

  const showImg = src !== null;
  return (
    <div
      className={`group relative w-40 shrink-0 cursor-pointer select-none overflow-hidden rounded-md border-2 bg-surface transition-colors ${
        selected ? "border-accent bg-accent/10" : "border-edge hover:border-text-muted"
      }`}
      onClick={() => onToggle(file.path)}
      data-testid="wizard-tile"
      data-path={file.path}
      data-selected={selected}
      data-kind={file.kind}
    >
      <div className="relative h-[120px] w-full overflow-hidden bg-panel/40">
        {showImg ? (
          <img
            src={src ?? undefined}
            alt={file.name}
            loading="lazy"
            decoding="async"
            onLoad={() => settle(true)}
            onError={() => settle(false)}
            className={`h-full w-full object-cover transition-opacity duration-150 ${
              loaded ? "opacity-100" : "opacity-0"
            } ${selected ? "brightness-110" : ""}`}
          />
        ) : (
          <ThumbPlaceholder kind={file.kind} name={file.name} />
        )}
        <button
          type="button"
          role="checkbox"
          aria-checked={selected}
          aria-label={file.name}
          onClick={(e) => {
            e.stopPropagation();
            onToggle(file.path);
          }}
          className={`absolute left-1.5 top-1.5 flex h-5 w-5 items-center justify-center rounded-full border transition-opacity ${
            selected
              ? "border-accent bg-accent opacity-100"
              : "border-white/70 bg-black/50 opacity-0 group-hover:opacity-100"
          }`}
        >
          {selected && (
            <svg
              viewBox="0 0 16 16"
              width="11"
              height="11"
              fill="none"
              stroke="#FFFFFF"
              strokeWidth="2.2"
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden="true"
            >
              <path d="M3.5 8.5l3 3 6-6.5" />
            </svg>
          )}
        </button>
      </div>
      <div className="flex h-[30px] items-center justify-between gap-1 px-1.5">
        <span className="truncate font-mono text-[10px] text-text-secondary" title={file.name}>
          {file.name}
        </span>
        <span className="shrink-0 font-mono text-[10px] text-text-muted tabular-nums">
          {formatBytes(file.size)}
        </span>
      </div>
    </div>
  );
}

/** 分组头（两视图共用）：折叠行，风格从简 */
function GroupHeaderRow({
  label,
  dir,
  count,
  isCollapsed,
  onToggleCollapse,
  testId,
}: {
  label: string;
  dir: string;
  count: number;
  isCollapsed: boolean;
  onToggleCollapse: (dir: string) => void;
  testId: string;
}) {
  return (
    <div className="flex h-[26px] items-center rounded bg-panel/30">
      <button
        type="button"
        onClick={() => onToggleCollapse(dir)}
        className="flex min-w-0 flex-1 items-center gap-1 px-2 py-1 text-left"
        data-testid={testId}
        data-dir={dir}
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
        <span className="truncate font-mono text-[11px] text-text-secondary" title={dir}>
          {label}
        </span>
        <span className="ml-auto shrink-0 pl-2 text-[11px] text-text-muted">{count}</span>
      </button>
    </div>
  );
}

// --- 列表视图（默认）：表格形态 + 虚拟化 -------------------------------------------

type ListRow =
  | { type: "group"; group: DirGroup }
  | { type: "file"; file: SourceFile };

function FileListView({
  groups,
  collapsed,
  selected,
  onToggleFile,
  onToggleCollapse,
  rootDirLabel,
}: {
  groups: DirGroup[];
  collapsed: Set<string>;
  selected: Set<string>;
  onToggleFile: (path: string) => void;
  onToggleCollapse: (dir: string) => void;
  rootDirLabel: string;
}) {
  const { t } = useTranslation();
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const rows = useMemo<ListRow[]>(() => {
    const out: ListRow[] = [];
    for (const g of groups) {
      out.push({ type: "group", group: g });
      if (collapsed.has(g.dir)) continue;
      for (const f of g.files) out.push({ type: "file", file: f });
    }
    return out;
  }, [groups, collapsed]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 26,
    overscan: 10,
  });

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="wizard-file-list">
      <div className="grid shrink-0 grid-cols-[32px_minmax(0,1fr)_80px_72px] items-center border-b border-edge px-2 text-[11px] text-text-muted">
        <span aria-hidden="true" />
        <span className="px-2 py-1.5">{t("wizard.columnName")}</span>
        <span className="py-1.5 text-right">{t("wizard.columnSize")}</span>
        <span className="py-1.5 text-right">{t("wizard.columnKind")}</span>
      </div>
      <div ref={scrollRef} className="min-h-0 flex-1 overflow-y-auto" data-testid="wizard-list-scroll">
        <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {virtualizer.getVirtualItems().map((vi) => {
            const row = rows[vi.index];
            return (
              <div
                key={vi.key}
                data-index={vi.index}
                style={{
                  position: "absolute",
                  top: 0,
                  left: 0,
                  width: "100%",
                  transform: `translateY(${vi.start}px)`,
                }}
              >
                {row.type === "group" ? (
                  <GroupHeaderRow
                    label={row.group.dir || rootDirLabel}
                    dir={row.group.dir}
                    count={row.group.files.length}
                    isCollapsed={collapsed.has(row.group.dir)}
                    onToggleCollapse={onToggleCollapse}
                    testId="wizard-list-group"
                  />
                ) : (
                  <label
                    className={`grid h-[26px] cursor-pointer grid-cols-[32px_minmax(0,1fr)_80px_72px] items-center border-b border-edge/40 px-2 text-xs ${
                      selected.has(row.file.path) ? "" : "opacity-40"
                    }`}
                  >
                    <input
                      type="checkbox"
                      checked={selected.has(row.file.path)}
                      onChange={() => onToggleFile(row.file.path)}
                      aria-label={row.file.name}
                      className="h-3 w-3 accent-[#F0A83C]"
                    />
                    <span
                      className="truncate px-2 font-mono text-[11px] text-text-primary"
                      title={row.file.path}
                    >
                      {row.file.name}
                    </span>
                    <span className="text-right font-mono text-[11px] text-text-secondary tabular-nums">
                      {formatBytes(row.file.size)}
                    </span>
                    <span className="flex justify-end pr-1">
                      <KindBadge kind={row.file.kind} />
                    </span>
                  </label>
                )}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

// --- 缩略图网格视图：响应式列数 + 虚拟化 -------------------------------------------

const TILE_W = 160;
const TILE_H = 150; // 4:3 缩略区 120 + 信息条 30
const GRID_GAP = 8;
const GROUP_HEADER_H = 26;

type GridRow =
  | { type: "header"; dir: string; count: number }
  | { type: "tiles"; files: SourceFile[] };

function FileGridView({
  groups,
  collapsed,
  selected,
  onToggleFile,
  onToggleCollapse,
  basePath,
  rootDirLabel,
}: {
  groups: DirGroup[];
  collapsed: Set<string>;
  selected: Set<string>;
  onToggleFile: (path: string) => void;
  onToggleCollapse: (dir: string) => void;
  basePath: string | null;
  rootDirLabel: string;
}) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(0);

  // 动态列数：容器宽（滚动区）自适应；ResizeObserver 跟踪
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const update = () => setWidth(el.clientWidth);
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const columns = Math.max(1, Math.floor((width - GRID_GAP) / (TILE_W + GRID_GAP)));

  const rows = useMemo<GridRow[]>(() => {
    const out: GridRow[] = [];
    for (const g of groups) {
      out.push({ type: "header", dir: g.dir, count: g.files.length });
      if (collapsed.has(g.dir)) continue;
      for (let i = 0; i < g.files.length; i += columns) {
        out.push({ type: "tiles", files: g.files.slice(i, i + columns) });
      }
    }
    return out;
  }, [groups, collapsed, columns]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => (rows[i].type === "header" ? GROUP_HEADER_H : TILE_H + GRID_GAP),
    overscan: 8,
  });

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="wizard-file-grid">
      <div ref={scrollRef} className="min-h-0 flex-1 overflow-y-auto p-2" data-testid="wizard-grid-scroll">
        <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {virtualizer.getVirtualItems().map((vi) => {
            const row = rows[vi.index];
            return (
              <div
                key={vi.key}
                data-index={vi.index}
                style={{
                  position: "absolute",
                  top: 0,
                  left: 0,
                  width: "100%",
                  transform: `translateY(${vi.start}px)`,
                }}
              >
                {row.type === "header" ? (
                  <GroupHeaderRow
                    label={row.dir || rootDirLabel}
                    dir={row.dir}
                    count={row.count}
                    isCollapsed={collapsed.has(row.dir)}
                    onToggleCollapse={onToggleCollapse}
                    testId="wizard-grid-group"
                  />
                ) : (
                  <div className="flex flex-wrap gap-2 pb-2">
                    {row.files.map((f) => (
                      <FileTile
                        key={f.path}
                        file={f}
                        selected={selected.has(f.path)}
                        onToggle={onToggleFile}
                        assetUrl={f.kind === "photo" ? toAssetUrl(basePath, relPathOf(f)) : null}
                      />
                    ))}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

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
  // 查看方式：列表默认 / 缩略图网格；切换不重置勾选（selected 与视图无关）
  const [viewMode, setViewMode] = useState<WizardViewMode>(loadViewMode);

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
            <div className="flex shrink-0 items-center px-3 py-2">
              <span className="text-xs font-medium text-text-secondary">{t("wizard.sourceTree")}</span>
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

        {/* 中栏：文件区（列表/缩略图双视图，共享勾选与统计；工具栏=统计+全选/反选） */}
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
            <div className="flex gap-1.5">
              <button
                type="button"
                onClick={selectAll}
                className="text-[11px] text-text-muted transition-colors hover:text-accent"
              >
                {t("wizard.selectAll")}
              </button>
              <button
                type="button"
                onClick={invertSelection}
                className="text-[11px] text-text-muted transition-colors hover:text-accent"
              >
                {t("wizard.invert")}
              </button>
            </div>
          </div>
          {files.length === 0 ? (
            <div className="flex min-h-0 flex-1 flex-col items-center justify-center px-6 py-10 text-center">
              <p className="text-xs leading-relaxed text-text-muted">
                {filesLoading && device
                  ? t("wizard.tableLoading")
                  : device
                    ? t("wizard.tableEmpty")
                    : t("wizard.treeEmpty")}
              </p>
            </div>
          ) : viewMode === "list" ? (
            <FileListView
              groups={groups}
              collapsed={collapsed}
              selected={selected}
              onToggleFile={toggleFile}
              onToggleCollapse={toggleCollapse}
              rootDirLabel={t("wizard.rootDir")}
            />
          ) : (
            <FileGridView
              groups={groups}
              collapsed={collapsed}
              selected={selected}
              onToggleFile={toggleFile}
              onToggleCollapse={toggleCollapse}
              basePath={sourceBasePath(device)}
              rootDirLabel={t("wizard.rootDir")}
            />
          )}
        </section>

        {/* 右栏：方案面板 */}
        <section className="flex min-h-0 flex-col overflow-y-auto rounded-lg border border-edge bg-surface p-3" aria-label={t("wizard.planPane")}>
          <h2 className="text-xs font-semibold text-text-primary">{t("wizard.plan")}</h2>

          {/* 查看方式：列表默认 / 缩略图网格，持久化用户偏好 */}
          <div className="mt-3 flex flex-col gap-1.5">
            <span className="text-xs font-medium text-text-secondary">{t("wizard.view.label")}</span>
            <div
              className="flex rounded-md border border-edge bg-bg p-0.5"
              role="radiogroup"
              aria-label={t("wizard.view.label")}
              data-testid="wizard-view"
            >
              {(["list", "grid"] as const).map((option) => (
                <button
                  key={option}
                  type="button"
                  role="radio"
                  aria-checked={viewMode === option}
                  onClick={() => {
                    setViewMode(option);
                    saveViewMode(option);
                  }}
                  className={`flex-1 rounded px-2 py-1 text-xs font-medium transition-colors ${
                    viewMode === option
                      ? "bg-accent text-black"
                      : "text-text-secondary hover:text-text-primary"
                  }`}
                  data-testid={`wizard-view-${option}`}
                >
                  {t(`wizard.view.${option}`)}
                </button>
              ))}
            </div>
          </div>

          <div className="mt-4 flex flex-col gap-1.5">
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
