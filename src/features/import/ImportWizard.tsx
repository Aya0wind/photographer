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
import { previewTemplate, importRootOf } from "@/features/onboarding/onboardingConfig";
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

// --- 左栏分区折叠 + 三栏列宽 + tile 尺寸（localStorage 记忆） ---------------------

export const PANEL_COLLAPSE_KEY = "smartphoto.import.panelCollapse";
export const COL_WIDTHS_KEY = "smartphoto.import.colWidths";
export const TILE_SIZE_KEY = "smartphoto.import.tileSize";

const DEFAULT_LEFT_WIDTH = 270;
const DEFAULT_RIGHT_WIDTH = 320;
const LEFT_WIDTH_RANGE: [number, number] = [200, 400];
const RIGHT_WIDTH_RANGE: [number, number] = [260, 440];

function clampNumber(value: number, [min, max]: [number, number]): number {
  return Math.min(max, Math.max(min, value));
}

type PanelSectionKey = "devices" | "fs" | "recent" | "source";
type PanelCollapseState = Record<PanelSectionKey, boolean>;

function loadPanelCollapse(): PanelCollapseState {
  try {
    const raw = localStorage.getItem(PANEL_COLLAPSE_KEY);
    const parsed = raw ? (JSON.parse(raw) as Record<string, unknown>) : {};
    return {
      devices: parsed.devices === true,
      fs: parsed.fs === true,
      recent: parsed.recent === true,
      source: parsed.source === true,
    };
  } catch {
    return { devices: false, fs: false, recent: false, source: false };
  }
}

function savePanelCollapse(state: PanelCollapseState): void {
  try {
    localStorage.setItem(PANEL_COLLAPSE_KEY, JSON.stringify(state));
  } catch {
    // 存储不可用时仅内存态生效
  }
}

function loadColWidths(): { left: number; right: number } {
  try {
    const raw = localStorage.getItem(COL_WIDTHS_KEY);
    const parsed = raw ? (JSON.parse(raw) as { left?: unknown; right?: unknown }) : {};
    const left =
      typeof parsed.left === "number" && Number.isFinite(parsed.left)
        ? parsed.left
        : DEFAULT_LEFT_WIDTH;
    const right =
      typeof parsed.right === "number" && Number.isFinite(parsed.right)
        ? parsed.right
        : DEFAULT_RIGHT_WIDTH;
    return { left: clampNumber(left, LEFT_WIDTH_RANGE), right: clampNumber(right, RIGHT_WIDTH_RANGE) };
  } catch {
    return { left: DEFAULT_LEFT_WIDTH, right: DEFAULT_RIGHT_WIDTH };
  }
}

function saveColWidths(left: number, right: number): void {
  try {
    localStorage.setItem(COL_WIDTHS_KEY, JSON.stringify({ left, right }));
  } catch {
    // 存储不可用时仅内存态生效
  }
}

// --- 缩略图档位：紧凑 100 / 标准 120（默认）/ 大 150；紧凑档信息条只显文件名 -------

type TileSizeKey = "compact" | "standard" | "large";

interface TileSizeSpec {
  /** 块宽（px），缩略区按 4:3 */
  width: number;
  thumbH: number;
  infoH: number;
  showSize: boolean;
}

const TILE_SIZE_SPECS: Record<TileSizeKey, TileSizeSpec> = {
  compact: { width: 100, thumbH: 75, infoH: 26, showSize: false },
  standard: { width: 120, thumbH: 90, infoH: 28, showSize: true },
  large: { width: 150, thumbH: 112, infoH: 30, showSize: true },
};
const TILE_SIZE_ORDER: readonly TileSizeKey[] = ["compact", "standard", "large"];
/** 档位图标：居中方块边长（12 viewBox 内） */
const TILE_SIZE_ICON: Record<TileSizeKey, number> = { compact: 6, standard: 9, large: 12 };

function loadTileSize(): TileSizeKey {
  try {
    const value = localStorage.getItem(TILE_SIZE_KEY);
    return value === "compact" || value === "large" ? value : "standard";
  } catch {
    return "standard";
  }
}

function saveTileSize(size: TileSizeKey): void {
  try {
    localStorage.setItem(TILE_SIZE_KEY, size);
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
  size,
}: {
  file: SourceFile;
  selected: boolean;
  onToggle: (path: string) => void;
  assetUrl: string | null;
  size: TileSizeSpec;
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
      className={`group relative shrink-0 cursor-pointer select-none overflow-hidden rounded-md border-2 bg-surface transition-colors ${
        selected ? "border-accent bg-accent/10" : "border-edge hover:border-text-muted"
      }`}
      style={{ width: size.width }}
      onClick={() => onToggle(file.path)}
      data-testid="wizard-tile"
      data-path={file.path}
      data-selected={selected}
      data-kind={file.kind}
    >
      <div
        className="relative w-full overflow-hidden bg-panel/40"
        style={{ height: size.thumbH }}
      >
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
      <div
        className="flex items-center justify-between gap-1 px-1.5"
        style={{ height: size.infoH }}
      >
        <span className="truncate font-mono text-[10px] text-text-secondary" title={file.name}>
          {file.name}
        </span>
        {size.showSize && (
          <span className="shrink-0 font-mono text-[10px] text-text-muted tabular-nums">
            {formatBytes(file.size)}
          </span>
        )}
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
      <div
        ref={scrollRef}
        className="sp-scroll min-h-0 flex-1 overflow-y-auto"
        data-testid="wizard-list-scroll"
      >
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
  tile,
}: {
  groups: DirGroup[];
  collapsed: Set<string>;
  selected: Set<string>;
  onToggleFile: (path: string) => void;
  onToggleCollapse: (dir: string) => void;
  basePath: string | null;
  rootDirLabel: string;
  tile: TileSizeSpec;
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

  const tileRowH = tile.thumbH + tile.infoH + GRID_GAP;
  const columns = Math.max(1, Math.floor((width - GRID_GAP) / (tile.width + GRID_GAP)));

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
    estimateSize: (i) => (rows[i].type === "header" ? GROUP_HEADER_H : tileRowH),
    overscan: 8,
  });

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="wizard-file-grid">
      <div
        ref={scrollRef}
        className="sp-scroll min-h-0 flex-1 overflow-y-auto p-2"
        data-testid="wizard-grid-scroll"
      >
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
                        size={tile}
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

// --- 左栏可折叠分区（LR 式：箭头 150ms 旋转，点击标题整行切换） -------------------

function PanelSection({
  sectionKey,
  title,
  collapsed,
  onToggle,
  actions = null,
  testId,
  fill = false,
  children,
}: {
  sectionKey: PanelSectionKey;
  title: string;
  collapsed: boolean;
  onToggle: () => void;
  actions?: React.ReactNode;
  testId?: string;
  /** 填满剩余高度（源文件树用）：内容区随之外伸，内部滚动 */
  fill?: boolean;
  children: React.ReactNode;
}) {
  return (
    <div
      className={fill ? "flex min-h-0 flex-1 flex-col" : "shrink-0"}
      data-testid={testId ?? `wizard-section-${sectionKey}`}
    >
      <div className="flex shrink-0 items-center">
        <button
          type="button"
          onClick={onToggle}
          aria-expanded={!collapsed}
          className="flex min-w-0 flex-1 items-center gap-1.5 py-1.5 pl-3 pr-1.5 text-left transition-colors hover:bg-panel/40"
          data-testid={`wizard-section-toggle-${sectionKey}`}
        >
          <svg
            viewBox="0 0 16 16"
            width="10"
            height="10"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.6"
            className={`shrink-0 text-text-muted transition-transform duration-150 ${
              collapsed ? "" : "rotate-90"
            }`}
            aria-hidden="true"
          >
            <path d="M5 3l5 5-5 5" />
          </svg>
          <span className="truncate text-xs font-medium text-text-secondary">{title}</span>
        </button>
        {actions}
      </div>
      {!collapsed && (
        <div className={fill ? "flex min-h-0 flex-1 flex-col" : undefined}>{children}</div>
      )}
    </div>
  );
}

// --- 三栏列宽拖动条：拖动只改相邻边栏宽，中列 minmax(0,1fr) 自动补偿 ---------------

function ColumnResizeHandle({
  side,
  onDelta,
  onReset,
}: {
  side: "left" | "right";
  onDelta: (dx: number) => void;
  onReset: () => void;
}) {
  const [dragging, setDragging] = useState(false);
  const lastX = useRef(0);

  // 拖动中全局 col-resize + 禁止文本选择
  useEffect(() => {
    if (!dragging) return;
    const prevCursor = document.body.style.cursor;
    document.body.classList.add("select-none");
    document.body.style.cursor = "col-resize";
    return () => {
      document.body.classList.remove("select-none");
      document.body.style.cursor = prevCursor;
    };
  }, [dragging]);

  function handlePointerDown(e: React.PointerEvent<HTMLDivElement>): void {
    lastX.current = e.clientX;
    setDragging(true);
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      // jsdom/老 WebView 无指针捕获时退化为全局监听语义（本组件内 move/up 仍生效）
    }
  }

  function handlePointerMove(e: React.PointerEvent<HTMLDivElement>): void {
    if (!dragging) return;
    onDelta(e.clientX - lastX.current);
    lastX.current = e.clientX;
  }

  function handlePointerUp(e: React.PointerEvent<HTMLDivElement>): void {
    if (!dragging) return;
    setDragging(false);
    try {
      e.currentTarget.releasePointerCapture(e.pointerId);
    } catch {
      // 指针捕获不可用时静默
    }
  }

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerUp}
      onDoubleClick={onReset}
      className={`w-1.5 shrink-0 cursor-col-resize self-stretch rounded transition-colors ${
        dragging ? "bg-accent" : "bg-transparent hover:bg-edge"
      }`}
      data-testid={`wizard-col-handle-${side}`}
      data-dragging={dragging}
    />
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
  // 激活库（达芬奇式：导入整理规则是库属性，向导只读展示）
  const activeLibrary = useSettingsStore((s) =>
    s.settings.activeLibraryId
      ? s.settings.libraries.find((lib) => lib.id === s.settings.activeLibraryId) ?? null
      : null,
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

  // 方案状态：目标根/模板从激活库合成（只读；旧库缺字段时以全局设置兜底）
  const libraryDirTemplateFull = activeLibrary?.dirTemplate ?? importSettings.dirTemplate;
  const libraryImportSubdir = activeLibrary?.importSubdir ?? importSettings.importSubdir;
  const libraryPhotoRoot = activeLibrary?.photoRoot ?? "";
  const targetRoot = libraryPhotoRoot
    ? importRootOf(libraryPhotoRoot, libraryImportSubdir)
    : "";
  // plan 的目录段 = 库模板去掉尾部 {原文件名}；nameTemplate 恒为 {原文件名}
  const dirTemplate = libraryDirTemplateFull.replace(/\/?\{原文件名\}\s*$/, "");
  const locationPreview = previewTemplate(dirTemplate, targetRoot);
  const [duplicatePolicy, setDuplicatePolicy] = useState(importSettings.duplicatePolicy);
  const [skipImported, setSkipImported] = useState(importSettings.skipImported);
  // 双目的地（M2）：默认关；移动模式互斥（后端拒 move+secondTarget）
  const [secondEnabled, setSecondEnabled] = useState(false);
  const [secondRoot, setSecondRoot] = useState("");
  const [starting, setStarting] = useState(false);
  // 启动失败文案：优先透出后端 Err；invoke 不可用时为通用文案（null → 用 i18n 兜底）
  const [startError, setStartError] = useState<string | null>(null);
  // LR 式导入模式（顶部分段条）：复制保留原文件 / 移动纳管
  const [mode, setMode] = useState<ImportMode>("copy");
  // 查看方式：列表默认 / 缩略图网格；切换不重置勾选（selected 与视图无关）
  const [viewMode, setViewMode] = useState<WizardViewMode>(loadViewMode);
  // 左栏分区折叠（localStorage 记忆，默认全展开）
  const [panelCollapse, setPanelCollapse] = useState<PanelCollapseState>(loadPanelCollapse);
  // 三栏列宽：拖动只改边栏宽，中列 minmax(0,1fr) 自动补偿
  const [colWidths, setColWidths] = useState(loadColWidths);
  // 缩略图档位（默认标准 120px；列数与信息条随档位缩放）
  const [tileSize, setTileSize] = useState<TileSizeKey>(loadTileSize);

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

  const isMtp = device?.kind === "mtp";
  // 并发流数是库属性（设置页/新建库改）：向导只读合成；MTP 受协议限制恒 1
  const effectiveStreams = isMtp ? 1 : activeLibrary?.streams ?? 4;
  // 双目的地开启且第二目标根目录为空 → 必填校验拦住开始
  const secondReady = !secondEnabled || secondRoot.trim().length > 0;
  const canStart =
    Boolean(device && activeLibrary && targetRoot) && secondReady && !starting;

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

  /** 左栏分区折叠切换（写 localStorage 记忆） */
  function togglePanelSection(key: PanelSectionKey): void {
    setPanelCollapse((prev) => {
      const next = { ...prev, [key]: !prev[key] };
      savePanelCollapse(next);
      return next;
    });
  }

  /** 拖动分隔条：left=左栏宽 +dx；right=右栏宽 -dx（向右拖右栏变窄、中列变宽） */
  function resizeColumn(side: "left" | "right", dx: number): void {
    setColWidths((prev) => {
      const next =
        side === "left"
          ? { ...prev, left: clampNumber(Math.round(prev.left + dx), LEFT_WIDTH_RANGE) }
          : { ...prev, right: clampNumber(Math.round(prev.right - dx), RIGHT_WIDTH_RANGE) };
      if (next.left !== prev.left || next.right !== prev.right) {
        saveColWidths(next.left, next.right);
      }
      return next;
    });
  }

  /** 双击分隔条恢复默认列宽 */
  function resetColumns(): void {
    setColWidths({ left: DEFAULT_LEFT_WIDTH, right: DEFAULT_RIGHT_WIDTH });
    saveColWidths(DEFAULT_LEFT_WIDTH, DEFAULT_RIGHT_WIDTH);
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

  /** 双目的地「浏览…」：系统目录选择器选第二目标根目录 */
  async function browseSecondRoot(): Promise<void> {
    try {
      const dir = await openDialog({ directory: true });
      if (typeof dir === "string" && dir.length > 0) setSecondRoot(dir);
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

  async function startImport(): Promise<void> {
    if (!device || !canStart) return;
    setStarting(true);
    setStartError(null);
    const plan: ImportPlan = {
      sourceId: device.id,
      targetRoot,
      dirTemplate,
      nameTemplate: "{原文件名}",
      duplicatePolicy,
      skipImported,
      streams: effectiveStreams,
      mode,
      secondTarget:
        secondEnabled && secondRoot.trim() ? { targetRoot: secondRoot.trim(), dirTemplate } : undefined,
    };
    // 竞态防护：sessionStarted 事件可能先于 import_start 返回到达，先挂待归位模式/源类型
    useImportStore.getState().setPendingJobMode(mode);
    useImportStore.getState().setPendingJobSource(device.kind);
    const result = await importStart(plan);
    setStarting(false);
    if (!result.ok) {
      // error=null 表示 invoke 不可用：用通用文案；否则透出后端 Err 原文
      setStartError(result.error ?? t("wizard.startError"));
      return;
    }
    // 双保险：事件先到时 sessionStarted 已用 pending 归位，这里幂等覆盖
    useImportStore.getState().recordJobMode(result.jobId, mode);
    useImportStore.getState().recordJobSource(result.jobId, device.kind);
    // 每次导入可调项回写全局设置（作为后续新建库的默认值；库属性不再回写）
    const { update, save } = useSettingsStore.getState();
    const settings = useSettingsStore.getState().settings;
    update({
      import: {
        ...settings.import,
        duplicatePolicy,
        skipImported,
      },
    });
    void save(useSettingsStore.getState().settings);
    // LR 式后台导入：点了导入立即回画廊继续浏览，进度由全局右下角进度卡常驻呈现
    navigate("/gallery");
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
              onClick={() => {
                setMode(option);
                // 互斥：切到移动时自动关掉双目的地（后端拒 move+secondTarget）
                if (option === "move") setSecondEnabled(false);
              }}
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

      <div
        className="grid min-h-0 flex-1 p-2"
        style={{
          gridTemplateColumns: `${colWidths.left}px 6px minmax(0, 1fr) 6px ${colWidths.right}px`,
        }}
        data-testid="wizard-columns"
      >
        {/* 左栏：源面板（设备 / 文件系统树 / 最近使用，三区可折叠）+ 源文件树 */}
        <section className="flex min-h-0 flex-col overflow-hidden rounded-lg border border-edge bg-surface" aria-label={t("wizard.leftPane")}>
          {/* 设备区（可折叠） */}
          <PanelSection
            sectionKey="devices"
            title={t("wizard.section.devices")}
            collapsed={panelCollapse.devices}
            onToggle={() => togglePanelSection("devices")}
          >
            <div className="px-3 pb-3">
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
                      {device.kind === "mtp" && (
                        <div className="flex justify-between" data-testid="wizard-mtp-streams">
                          <dt className="text-text-muted">{t("wizard.streams")}</dt>
                          <dd className="font-mono text-text-secondary" title={t("wizard.mtpSingleStream")}>
                            1
                          </dd>
                        </div>
                      )}
                    </dl>
                  )}
                </>
              )}
            </div>
          </PanelSection>

          {/* 文件系统区：懒加载目录树（可折叠；浏览按钮常驻标题行） */}
          <PanelSection
            sectionKey="fs"
            testId="wizard-fs"
            title={t("wizard.fs.title")}
            collapsed={panelCollapse.fs}
            onToggle={() => togglePanelSection("fs")}
            actions={
              <button
                type="button"
                onClick={() => void browseFolder()}
                className="mr-2 flex shrink-0 items-center gap-1 rounded-md border border-edge px-2 py-0.5 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
                data-testid="wizard-fs-browse"
              >
                <FolderGlyph size={12} />
                {t("wizard.fs.browse")}
              </button>
            }
          >
            <div className="sp-scroll max-h-44 overflow-y-auto px-3 pb-2">
              {fsRoots === null ? null : fsRoots.length === 0 ? (
                <p className="px-2 py-1 text-[11px] leading-relaxed text-text-muted">
                  {t("wizard.fs.unavailable")}
                </p>
              ) : (
                fsRoots.map((node) => renderFsNode(node, 0))
              )}
            </div>
          </PanelSection>

          {/* 最近使用区（可折叠；有记录才显示） */}
          {recentSources.length > 0 && (
            <PanelSection
              sectionKey="recent"
              testId="wizard-recent"
              title={t("wizard.recent.title")}
              collapsed={panelCollapse.recent}
              onToggle={() => togglePanelSection("recent")}
            >
              <div className="px-3 pb-2">
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
            </PanelSection>
          )}

          {/* 源文件树（可折叠，与其他分区一致） */}
          <PanelSection
            sectionKey="source"
            title={t("wizard.sourceTree")}
            collapsed={panelCollapse.source}
            onToggle={() => togglePanelSection("source")}
            fill
          >
            <div className="sp-scroll min-h-0 flex-1 overflow-y-auto px-1.5 pb-2" data-testid="wizard-tree">
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
          </PanelSection>
        </section>

        {/* 左|中 列宽拖动条 */}
        <ColumnResizeHandle
          side="left"
          onDelta={(dx) => resizeColumn("left", dx)}
          onReset={resetColumns}
        />

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
              tile={TILE_SIZE_SPECS[tileSize]}
            />
          )}
        </section>

        {/* 中|右 列宽拖动条 */}
        <ColumnResizeHandle
          side="right"
          onDelta={(dx) => resizeColumn("right", dx)}
          onReset={resetColumns}
        />

        {/* 右栏：方案面板 */}
        <section className="flex min-h-0 flex-col overflow-y-auto rounded-lg border border-edge bg-surface p-3" aria-label={t("wizard.planPane")}>
          <h2 className="text-xs font-semibold text-text-primary">{t("wizard.plan")}</h2>

          {/* 查看方式 + 缩略图档位（均 localStorage 记忆） */}
          <div className="mt-3 flex flex-col gap-1.5">
            <span className="text-xs font-medium text-text-secondary">{t("wizard.view.label")}</span>
            <div className="flex items-stretch gap-1.5">
              <div
                className="flex flex-1 rounded-md border border-edge bg-bg p-0.5"
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
              <div
                className="flex rounded-md border border-edge bg-bg p-0.5"
                role="radiogroup"
                aria-label={t("wizard.tileSize.label")}
                data-testid="wizard-tile-size"
              >
                {TILE_SIZE_ORDER.map((option) => (
                  <button
                    key={option}
                    type="button"
                    role="radio"
                    aria-checked={tileSize === option}
                    aria-label={t(`wizard.tileSize.${option}`)}
                    title={t(`wizard.tileSize.${option}`)}
                    onClick={() => {
                      setTileSize(option);
                      saveTileSize(option);
                    }}
                    className={`flex w-7 items-center justify-center rounded transition-colors ${
                      tileSize === option
                        ? "bg-accent text-black"
                        : "text-text-secondary hover:text-text-primary"
                    }`}
                    data-testid={`wizard-tile-size-${option}`}
                  >
                    <svg
                      viewBox="0 0 16 16"
                      width="12"
                      height="12"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="1.5"
                      aria-hidden="true"
                    >
                      <rect
                        x={(16 - TILE_SIZE_ICON[option]) / 2}
                        y={(16 - TILE_SIZE_ICON[option]) / 2}
                        width={TILE_SIZE_ICON[option]}
                        height={TILE_SIZE_ICON[option]}
                        rx="1"
                      />
                    </svg>
                  </button>
                ))}
              </div>
            </div>
          </div>

          {/* 导入位置（库属性，只读）：目标根/模板随库走，在设置中修改 */}
          <div className="mt-4 flex flex-col gap-1.5">
            <div className="flex items-center justify-between gap-2">
              <span className="text-xs font-medium text-text-secondary">
                {t("wizard.location.title")}
              </span>
              <button
                type="button"
                onClick={() => navigate("/settings")}
                className="shrink-0 rounded bg-panel px-1.5 py-0.5 text-[11px] text-text-muted transition-colors hover:text-accent"
                title={t("wizard.location.badge")}
                data-testid="wizard-location-edit"
              >
                {t("wizard.location.badge")}
              </button>
            </div>
            <div
              className="flex flex-col gap-1.5 rounded-lg border border-edge bg-bg p-2.5"
              data-testid="wizard-location-card"
            >
              {activeLibrary ? (
                <>
                  <div className="flex items-baseline justify-between gap-2">
                    <span className="shrink-0 text-[11px] text-text-muted">
                      {t("wizard.location.root")}
                    </span>
                    <span
                      className="truncate font-mono text-[11px] text-text-primary"
                      title={targetRoot}
                    >
                      {targetRoot}
                    </span>
                  </div>
                  <div className="flex items-baseline justify-between gap-2">
                    <span className="shrink-0 text-[11px] text-text-muted">
                      {t("wizard.location.template")}
                    </span>
                    <span
                      className="truncate font-mono text-[11px] text-text-secondary"
                      title={libraryDirTemplateFull}
                    >
                      {libraryDirTemplateFull}
                    </span>
                  </div>
                  <p
                    className="mt-1 truncate rounded border border-edge bg-surface px-2 py-1 font-mono text-[11px] text-text-muted"
                    title={locationPreview}
                    data-testid="wizard-preview"
                  >
                    {locationPreview}
                  </p>
                </>
              ) : (
                <p className="text-[11px] leading-relaxed text-text-muted">
                  {t("wizard.location.noLibrary")}
                </p>
              )}
            </div>
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

          {/* 双目的地（M2）：默认关；移动模式互斥（后端拒 move+secondTarget） */}
          <div className="mt-4 flex flex-col gap-1.5">
            <label
              className={`flex items-center gap-2 text-xs ${
                mode === "move" ? "cursor-not-allowed text-text-muted" : "cursor-pointer text-text-secondary"
              }`}
              title={mode === "move" ? t("wizard.second.moveUnsupported") : undefined}
            >
              <input
                type="checkbox"
                checked={secondEnabled}
                disabled={mode === "move"}
                onChange={(e) => setSecondEnabled(e.target.checked)}
                className="h-3 w-3 accent-[#F0A83C] disabled:opacity-40"
                data-testid="wizard-second-toggle"
              />
              {t("wizard.second.label")}
            </label>
            <p className="pl-5 text-[11px] leading-relaxed text-text-muted">
              {mode === "move"
                ? t("wizard.second.moveUnsupported")
                : t("wizard.second.desc")}
            </p>
            {secondEnabled && mode !== "move" && (
              <div className="mt-1 flex flex-col gap-1.5" data-testid="wizard-second-panel">
                <div className="flex items-center gap-1.5">
                  <input
                    type="text"
                    value={secondRoot}
                    onChange={(e) => setSecondRoot(e.target.value)}
                    placeholder={t("wizard.second.rootPlaceholder")}
                    aria-label={t("wizard.second.root")}
                    className="min-w-0 flex-1 rounded-md border border-edge bg-bg px-2 py-1.5 font-mono text-[11px] text-text-primary outline-none transition-colors focus:border-accent"
                    data-testid="wizard-second-root"
                  />
                  <button
                    type="button"
                    onClick={() => void browseSecondRoot()}
                    className="shrink-0 rounded-md border border-edge px-2 py-1.5 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
                    data-testid="wizard-second-browse"
                  >
                    {t("wizard.browse")}
                  </button>
                </div>
                {secondRoot.trim() === "" && (
                  <p className="pl-0.5 text-[11px] text-yellow-300">{t("wizard.second.required")}</p>
                )}
                <p className="text-[11px] leading-relaxed text-text-muted">
                  {t("wizard.second.sameTemplate")}
                </p>
              </div>
            )}
          </div>

          {/* 并发流数已下沉库属性（设置页/新建库对话框修改），向导不再展示控件 */}

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
                {startError}
              </p>
            )}
          </div>
        </section>
      </div>
    </div>
  );
}
