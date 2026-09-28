import { useEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { convertFileSrc } from "@tauri-apps/api/core";
import { useVirtualizer } from "@tanstack/react-virtual";

import {
  albumCreate,
  albumList,
  albumSubgroups,
  deviceFiles,
  FIXED_PLAN_DIR_TEMPLATE,
  folderScan,
  fsListDirs,
  importStart,
  isIpcAvailable,
  kindFromName,
  thumbGet,
  type AlbumDto,
  type DeviceSnapshot,
  type FileKind,
  type FsDirEntry,
  type ImportMode,
  type ImportPlan,
} from "@/ipc/api";
import { formatBytes } from "@/lib/format";
import { FIXED_ALBUM_LAYOUT, importRootOf } from "@/features/onboarding/onboardingConfig";
import {
  isUngroupedAlbum,
  subgroupSuggestions,
  UNGROUPED_ALBUM_NAME,
} from "@/features/albums/lib/ungroupedAlbum";
import { useSettingsStore } from "@/stores/settingsStore";
import { seedDevicesFromBackend, useImportStore, type RecentSource, type SourceFile } from "@/stores/importStore";
import { deviceKindLabelKey, devicePresentationKind, type DevicePresentationKind } from "./devicePresentation";

/**
 * 导入向导（LR 式源面板 + A 密度三栏）：
 * 顶部=复制/移动分段模式条；左=设备卡列表 + 文件系统懒加载目录树
 * （点击文件夹名=选中该文件夹为源，folderScan 成 FOLDER: 源）+ 最近使用源
 * + 源文件树；中=文件区双视图（列表默认 / 缩略图网格，右栏切换并持久化，
 * 两视图均虚拟化、共享勾选语义与统计条）；右=方案面板（查看方式/目标根与固定
 * 目录布局展示/查重策略/存入相册与实时路径预览，MTP 强制 1）。
 *
 * 缩略图：photo 走 asset 协议（convertFileSrc，folder=去前缀路径/volume=设备id
 * + relPath，MTP 无文件系统路径恒占位），信号量限 6 张在途解码，onLoad 150ms
 * 淡入，失败/超时静默保持占位；RAW 无内嵌预览时保持占位。
 */

interface DirGroup {
  dir: string;
  files: SourceFile[];
}

function groupByDir(files: SourceFile[], scanning = false): DirGroup[] {
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
      files: scanning ? list : [...list].sort((a, b) => a.name.localeCompare(b.name)),
    }));
}

const KIND_LABEL_COLOR: Record<FileKind, string> = {
  photo: "text-accent",
  raw: "text-sky-400",
  other: "text-text-muted",
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

/** 设备类型图标（读卡器=存储卡 / 相机 / 文件夹），16 viewBox stroke 手写 */
function DeviceGlyph({
  kind,
  size = 14,
  className = "",
}: {
  kind: DevicePresentationKind;
  size?: number;
  className?: string;
}) {
  const paths: Record<DevicePresentationKind, React.ReactNode> = {
    reader: (
      <>
        <path d="M4.6 2.5h4.5L12.5 6v6.2c0 .8-.6 1.3-1.4 1.3H4.6c-.9 0-1.6-.7-1.6-1.5V4c0-.8.7-1.5 1.6-1.5z" />
        <path d="M9.1 2.5V6h3.4" />
      </>
    ),
    camera: (
      <>
        <rect x="2" y="4.6" width="12" height="8.4" rx="1.5" />
        <path d="M5.7 4.6l.9-1.7h2.8l.9 1.7" />
        <circle cx="8" cy="8.7" r="2.4" />
      </>
    ),
    folder: (
      <path d="M2 4.75C2 3.78 2.78 3 3.75 3h2.6l1.5 1.75h4.4c.97 0 1.75.78 1.75 1.75v5.75c0 .97-.78 1.75-1.75 1.75h-8.5C2.78 14 2 13.22 2 12.25v-7.5z" />
    ),
  };
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
      {paths[kind]}
    </svg>
  );
}

/** 设备列表行的文件数徽标：总文件数千分位；无文件时退回类型中文标签 */
function deviceBadge(d: DeviceSnapshot, t: (key: string) => string): string {
  if (d.scanStatus === "scanning") return t("wizard.deviceScanning");
  if (d.scanStatus === "failed") return t("wizard.deviceScanFailed");
  const total = Object.values(d.filesByKind).reduce((sum, n) => sum + n, 0);
  return total > 0 ? `${total.toLocaleString("zh-CN")} ${t("wizard.deviceFiles")}` : t(deviceKindLabelKey(d));
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

// --- 缩略图档位：标准 120（默认）/ 大 150，与其他照片网格统一为两档 -------

type TileSizeKey = "standard" | "large";

interface TileSizeSpec {
  /** 块宽（px），缩略区按 4:3 */
  width: number;
  thumbH: number;
  infoH: number;
  showSize: boolean;
}

const TILE_SIZE_SPECS: Record<TileSizeKey, TileSizeSpec> = {
  standard: { width: 120, thumbH: 90, infoH: 28, showSize: true },
  large: { width: 150, thumbH: 112, infoH: 30, showSize: true },
};
const TILE_SIZE_ORDER: readonly TileSizeKey[] = ["standard", "large"];
/** 档位图标：居中方块边长（12 viewBox 内） */
const TILE_SIZE_ICON: Record<TileSizeKey, number> = { standard: 7, large: 12 };

function loadTileSize(): TileSizeKey {
  try {
    const value = localStorage.getItem(TILE_SIZE_KEY);
    return value === "large" ? value : "standard";
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

/** 缩略图请求边长（px）：后端缓存档位就近 */
const THUMB_SIZE = 256;

/** thumb_get 在途并发上限：后端已提速（turbojpeg 缩放解码+动态并发），前端拉高铺屏速度 */
const IMAGE_LOAD_CONCURRENCY = 8;
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

/** 会话内缩略图缓存：absPath → asset URL（null=该文件无缩略图或提取失败） */
const thumbUrlCache = new Map<string, string | null>();
/** in-flight 去重：同 absPath 的并发请求共享同一 Promise（滚动复用不重复 IPC） */
const thumbInflight = new Map<string, Promise<string | null>>();

/** 缩略图文件路径 → asset 协议 URL；非 Tauri 环境抛错/空结果回退 null（占位） */
function thumbAssetUrl(thumbPath: string): string | null {
  try {
    return convertFileSrc(thumbPath) || null;
  } catch {
    return null;
  }
}

/**
 * 取某源文件的缩略图 asset URL（M2 起 img 一律读后端小图，不再解码原图）：
 * thumb_get(absPath, 256) → 缓存文件路径 → convertFileSrc；null → 调用方保持占位。
 * 命中缓存直接返回；同 path 并发共享 in-flight Promise；thumb_get 失败静默记 null。
 */
export function fetchThumbUrl(absPath: string): Promise<string | null> {
  const cached = thumbUrlCache.get(absPath);
  if (cached !== undefined) return Promise.resolve(cached);
  const inflight = thumbInflight.get(absPath);
  if (inflight) return inflight;
  const promise = (async () => {
    await acquireImageSlot();
    try {
      const thumbPath = await thumbGet(absPath, THUMB_SIZE);
      const url = thumbPath !== null ? thumbAssetUrl(thumbPath) : null;
      thumbUrlCache.set(absPath, url);
      return url;
    } catch {
      thumbUrlCache.set(absPath, null);
      return null;
    } finally {
      releaseImageSlot();
      thumbInflight.delete(absPath);
    }
  })();
  thumbInflight.set(absPath, promise);
  return promise;
}

/** 仅测试用：清空缩略图会话缓存 */
export function resetThumbCacheForTests(): void {
  thumbUrlCache.clear();
  thumbInflight.clear();
}

function extOf(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot >= 0 ? name.slice(dot + 1).toUpperCase() : "";
}

/** 缩略占位块：surface 底 + kind 色点缀（RAW=扩展名大字+徽标；photo=图片框） */
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
  absPath,
  size,
}: {
  file: SourceFile;
  selected: boolean;
  onToggle: (path: string) => void;
  /** 源文件绝对路径（photo 且源有文件系统路径）；thumb_get 据此取后端小图 */
  absPath: string | null;
  size: TileSizeSpec;
}) {
  // M2 起 img 一律读后端小图（~30KB），不再解码原图；onLoad 淡入，onError/15s 超时静默保持占位
  const [src, setSrc] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [pending, setPending] = useState(false);
  const settledRef = useRef(false);

  useEffect(() => {
    let cancelled = false;
    if (absPath === null) return;
    setPending(true);
    void fetchThumbUrl(absPath).then((url) => {
      // null=无缩略图或提取失败：保持占位
      if (cancelled) return;
      setPending(false);
      if (url === null) return;
      setSrc(url);
    });
    return () => {
      cancelled = true;
    };
  }, [absPath]);

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
  }

  const showImg = src !== null;
  // 加载中（请求在途 / 小图在解码）= 骨架动画；永久无图或已展示 = 静态底
  const thumbLoading = pending || (showImg && !loaded);
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
        className={`relative w-full overflow-hidden ${thumbLoading ? "sp-skeleton" : "bg-panel/40"}`}
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

function SourceTree({ groups, collapsed, selected, onToggleGroup, onToggleCollapse, onToggleFile }: {
  groups: DirGroup[];
  collapsed: Set<string>;
  selected: Set<string>;
  onToggleGroup: (group: DirGroup) => void;
  onToggleCollapse: (dir: string) => void;
  onToggleFile: (path: string) => void;
}) {
  const { t } = useTranslation();
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const rows = useMemo<ListRow[]>(() => groups.flatMap((group): ListRow[] => [
    { type: "group", group },
    ...(collapsed.has(group.dir) ? [] : group.files.map((file): ListRow => ({ type: "file", file }))),
  ]), [groups, collapsed]);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 26,
    overscan: 10,
    getItemKey: (index) => {
      const row = rows[index];
      return row.type === "group" ? `group:${row.group.dir}` : `file:${row.file.path}`;
    },
  });
  return (
    <div ref={scrollRef} className="sp-scroll min-h-0 flex-1 overflow-y-auto px-1.5 pb-2" data-testid="wizard-tree">
      {rows.length === 0 ? <p className="px-2 py-4 text-xs leading-relaxed text-text-muted">{t("wizard.treeEmpty")}</p> : (
        <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {virtualizer.getVirtualItems().map((item) => {
            const row = rows[item.index];
            let content;
            if (row.type === "group") {
              const group = row.group;
              const allIn = group.files.every((f) => selected.has(f.path));
              const someIn = group.files.some((f) => selected.has(f.path));
              const isCollapsed = collapsed.has(group.dir);
              content = (
                <div className="flex items-center gap-1.5 rounded px-1.5 py-1 hover:bg-panel/40">
                  <input
                    type="checkbox"
                    checked={allIn}
                    ref={(el) => {
                      if (el) el.indeterminate = !allIn && someIn;
                    }}
                    onChange={() => onToggleGroup(group)}
                    aria-label={t("wizard.groupToggle", { dir: group.dir })}
                    className="h-3 w-3 shrink-0 accent-[#F0A83C]"
                  />
                  <button
                    type="button"
                    onClick={() => onToggleCollapse(group.dir)}
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
              );
            } else {
              const f = row.file;
              content = (
                <label
                  key={f.path}
                  className="flex cursor-pointer items-center gap-1.5 rounded h-[26px] pl-7 pr-1.5 hover:bg-panel/40"
                >
                  <input
                    type="checkbox"
                    checked={selected.has(f.path)}
                    onChange={() => onToggleFile(f.path)}
                    className="h-3 w-3 shrink-0 accent-[#F0A83C]"
                  />
                  <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-text-secondary" title={f.name}>
                    {f.name}
                  </span>
                </label>
              );
            }
            return <div key={item.key} data-tree-row style={{ position: "absolute", top: 0, left: 0, width: "100%", height: 26, transform: `translateY(${item.start}px)` }}>{content}</div>;
          })}
        </div>
      )}
    </div>
  );
}

type FsTreeRow =
  | { type: "folder"; node: FsDirEntry; depth: number }
  | { type: "status"; key: string; depth: number; loading: boolean };

/** 文件夹导航也使用虚拟列表：展开数千个子目录时只创建视口附近的行。 */
function FileSystemTree({
  roots,
  expanded,
  children,
  loading,
  selectedPath,
  onToggle,
  onSelect,
}: {
  roots: FsDirEntry[];
  expanded: Set<string>;
  children: Record<string, FsDirEntry[]>;
  loading: Set<string>;
  selectedPath: string | null;
  onToggle: (node: FsDirEntry) => void;
  onSelect: (path: string) => void;
}) {
  const { t } = useTranslation();
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const rows = useMemo<FsTreeRow[]>(() => {
    const result: FsTreeRow[] = [];
    const append = (nodes: FsDirEntry[], depth: number) => {
      for (const node of nodes) {
        result.push({ type: "folder", node, depth });
        if (!expanded.has(node.path)) continue;
        const loaded = children[node.path];
        if (loading.has(node.path) || loaded === undefined) {
          result.push({ type: "status", key: `${node.path}:loading`, depth: depth + 1, loading: true });
        } else if (loaded.length === 0) {
          result.push({ type: "status", key: `${node.path}:empty`, depth: depth + 1, loading: false });
        } else {
          append(loaded, depth + 1);
        }
      }
    };
    append(roots, 0);
    return result;
  }, [roots, expanded, children, loading]);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 30,
    overscan: 12,
    getItemKey: (index) => {
      const row = rows[index];
      return row.type === "folder" ? `folder:${row.node.path}` : row.key;
    },
  });

  return (
    <div ref={scrollRef} className="sp-scroll h-full min-h-0 overflow-y-auto px-2 pb-2" data-testid="wizard-fs-tree">
      <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
        {virtualizer.getVirtualItems().map((item) => {
          const row = rows[item.index];
          const content = row.type === "status" ? (
            <div
              className="flex h-[30px] items-center gap-2 text-[11px] text-text-muted"
              style={{ paddingLeft: 26 + row.depth * 14 }}
            >
              {row.loading && <span className="h-3 w-3 animate-spin rounded-full border border-text-muted border-t-accent" />}
              {row.loading ? t("wizard.fs.loading") : t("wizard.fs.empty")}
            </div>
          ) : (() => {
            const node = row.node;
            const loaded = children[node.path];
            const isExpanded = expanded.has(node.path);
            const isLoading = loading.has(node.path);
            const canExpand = node.hasSubdirs && (loaded === undefined || loaded.length > 0);
            const isSelected = selectedPath !== null && normalizeFsPath(node.path) === selectedPath;
            return (
              <div
                className={`flex h-[30px] items-center gap-1 rounded-md border border-transparent pr-1.5 transition-colors ${
                  isSelected ? "border-accent/30 bg-accent/10" : "hover:bg-panel/60"
                }`}
                style={{ paddingLeft: 4 + row.depth * 14 }}
              >
                {canExpand ? (
                  <button
                    type="button"
                    onClick={() => onToggle(node)}
                    className="flex h-6 w-6 shrink-0 items-center justify-center rounded text-text-muted hover:bg-bg hover:text-accent"
                    aria-label={t(isExpanded ? "wizard.fs.collapse" : "wizard.fs.expand", { dir: node.path })}
                    data-testid="wizard-fs-toggle"
                  >
                    {isLoading ? (
                      <span className="h-3 w-3 animate-spin rounded-full border border-text-muted border-t-accent" />
                    ) : (
                      <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" strokeWidth="1.8"
                        className={`transition-transform ${isExpanded ? "rotate-90" : ""}`} aria-hidden="true">
                        <path d="M5 3l5 5-5 5" />
                      </svg>
                    )}
                  </button>
                ) : <span className="h-6 w-6 shrink-0" aria-hidden="true" />}
                <button
                  type="button"
                  onClick={() => onSelect(node.path)}
                  className={`flex min-w-0 flex-1 items-center gap-2 py-1 text-left ${isSelected ? "text-accent" : "text-text-secondary"}`}
                  data-testid="wizard-fs-node"
                  data-path={node.path}
                  data-selected={isSelected}
                  title={node.path}
                >
                  <FolderGlyph size={14} className={isSelected ? "text-accent" : "text-text-muted"} />
                  <span className="truncate text-xs">{node.name}</span>
                </button>
              </div>
            );
          })();
          return (
            <div key={item.key} style={{ position: "absolute", top: 0, left: 0, width: "100%", height: 30, transform: `translateY(${item.start}px)` }}>
              {content}
            </div>
          );
        })}
      </div>
    </div>
  );
}


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
                        absPath={
                          f.kind === "photo" && basePath !== null ? `${basePath}/${relPathOf(f)}` : null
                        }
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

function ViewControls({
  viewMode,
  tileSize,
  onViewMode,
  onTileSize,
}: {
  viewMode: WizardViewMode;
  tileSize: TileSizeKey;
  onViewMode: (mode: WizardViewMode) => void;
  onTileSize: (size: TileSizeKey) => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex items-center gap-1.5">
      <div className="flex rounded-lg border border-edge bg-bg/70 p-0.5" role="radiogroup"
        aria-label={t("wizard.view.label")} data-testid="wizard-view">
        {(["list", "grid"] as const).map((option) => (
          <button key={option} type="button" role="radio" aria-checked={viewMode === option}
            onClick={() => onViewMode(option)}
            className={`rounded-md px-2.5 py-1 text-[11px] font-medium transition-colors ${
              viewMode === option ? "bg-panel text-text-primary shadow-sm" : "text-text-muted hover:text-text-primary"
            }`} data-testid={`wizard-view-${option}`}>
            {t(`wizard.view.${option}`)}
          </button>
        ))}
      </div>
      {viewMode === "grid" && (
        <div className="flex rounded-lg border border-edge bg-bg/70 p-0.5" role="radiogroup"
          aria-label={t("wizard.tileSize.label")} data-testid="wizard-tile-size">
          {TILE_SIZE_ORDER.map((option) => (
            <button key={option} type="button" role="radio" aria-checked={tileSize === option}
              aria-label={t(`wizard.tileSize.${option}`)} title={t(`wizard.tileSize.${option}`)}
              onClick={() => onTileSize(option)}
              className={`flex h-6 w-7 items-center justify-center rounded-md transition-colors ${
                tileSize === option ? "bg-panel text-accent shadow-sm" : "text-text-muted hover:text-text-primary"
              }`} data-testid={`wizard-tile-size-${option}`}>
              <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
                <rect x={(16 - TILE_SIZE_ICON[option]) / 2} y={(16 - TILE_SIZE_ICON[option]) / 2}
                  width={TILE_SIZE_ICON[option]} height={TILE_SIZE_ICON[option]} rx="1" />
              </svg>
            </button>
          ))}
        </div>
      )}
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
  // 激活库（达芬奇式：导入整理规则是库属性，向导只读展示）
  const activeLibrary = useSettingsStore((s) =>
    s.settings.activeLibraryId
      ? s.settings.libraries.find((lib) => lib.id === s.settings.activeLibraryId) ?? null
      : null,
  );

  // 设备选择：显式选中（会话内）优先 → URL ?device=（深链/设备弹窗入口）→ 首台回落。
  // 修复「设备在场时点文件系统文件夹无反应」：此前 selectedId 只认 URL——URL 往返
  // （URLSearchParams 序列化不编码反斜杠，真机 browser history 下读回值可能变形）
  // 或 addDevice/setSearchParams 跨渲染批次的中间态都会让 some() 匹配失败，回落
  // devices[0]（在场旧设备），表现为点了文件夹毫无反应。显式选中不再依赖 URL 往返与时序。
  const [explicitId, setExplicitId] = useState<string | null>(null);
  const urlDevice = searchParams.get("device");
  const explicitValid = explicitId !== null && devices.some((d) => d.id === explicitId);
  const urlValid = urlDevice !== null && devices.some((d) => d.id === urlDevice);
  const selectedId = explicitValid
    ? (explicitId as string)
    : urlValid
      ? (urlDevice as string)
      : (devices[0]?.id ?? null);
  const device = devices.find((d) => d.id === selectedId) ?? null;

  // 选中源变化时拉取文件清单（device_files）；已有缓存的源不重复拉取。
  // 扫描中由文件增量填充；仅已完成且没有缓存的源使用兼容读取。
  const [filesLoading, setFilesLoading] = useState(false);
  useEffect(() => {
    if (!selectedId || device?.scanStatus === "scanning" || device?.scanStatus === "failed") {
      setFilesLoading(false);
      return;
    }
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
  }, [selectedId, device?.scanStatus, sourceFilesMap[selectedId ?? ""]]);

  const files = useMemo(
    () => (selectedId ? sourceFilesMap[selectedId] ?? [] : []).filter((file) => file.kind === "photo" || file.kind === "raw"),
    [selectedId, sourceFilesMap],
  );
  // 扫描中按到达顺序追加，结束后再排序，避免每批重排所有文件。
  const groups = useMemo(() => groupByDir(files, device?.scanStatus === "scanning"), [files, device?.scanStatus]);

  // 当前选中源对应的文件夹路径（树高亮）；设备源为 null
  const selectedFolderPath =
    device?.kind === "folder" ? normalizeFsPath(device.id.slice("FOLDER:".length)) : null;

  // 设备区只渲染真实设备（volume/mtp）：folder 是「导入源」不是设备——
  // 其统计在中栏统计条、选中态由文件系统树高亮表达，设备区不重复呈现。
  // devices 数组机制不动（选中链路/jobSources 仍依赖 folder 快照在表中）。
  const visibleDevices = useMemo(
    () => devices.filter((d) => d.kind !== "folder"),
    [devices],
  );

  // 选择状态（路径集合）；设备切换时重置为全选（默认导入全部）
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  const seenFiles = useRef<{ id: string | null; paths: Set<string> }>({ id: null, paths: new Set() });
  useEffect(() => {
    const next = new Set(files.map((f) => f.path));
    const previous = seenFiles.current;
    if (previous.id !== selectedId) {
      setSelected(next);
      setCollapsed(new Set());
    } else {
      setSelected((current) => new Set([...next].filter((path) => current.has(path) || !previous.paths.has(path))));
    }
    seenFiles.current = { id: selectedId, paths: next };
  }, [selectedId, files]);

  // 方案状态：目标根从激活库合成（只读；旧库缺字段时以全局设置兜底）。
  // 目录布局已固定（时间/相册+平铺，dirTemplate 配置退役 2026-09-28）：
  // 具体落位见相册区实时预览，此处不再展示库级模板。
  const libraryPhotoRoot = activeLibrary?.photoRoot ?? "";
  const targetRoot = libraryPhotoRoot ? importRootOf(libraryPhotoRoot) : "";
  const [duplicatePolicy, setDuplicatePolicy] = useState(importSettings.duplicatePolicy);
  const [skipImported, setSkipImported] = useState(importSettings.skipImported);
  // 存入相册（规格修订后必选）：无「不添加」分支；默认预选系统保底相册「未分组」，
  // 清单到位后按名匹配回填；新建分支照旧。
  const [albumChoice, setAlbumChoice] = useState<"existing" | "new">("existing");
  const [albumId, setAlbumId] = useState<number | null>(null);
  const [newAlbumName, setNewAlbumName] = useState("");
  const [albums, setAlbums] = useState<AlbumDto[]>([]);
  const [albumError, setAlbumError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    void albumList().then((list) => {
      if (!cancelled) setAlbums(list);
    });
    return () => {
      cancelled = true;
    };
  }, []);
  // 默认预选「未分组」（后端按名幂等自动创建；仅在未手选时回填）
  useEffect(() => {
    if (albums.length === 0) return;
    const ungrouped = albums.find((a) => isUngroupedAlbum(a));
    if (ungrouped) setAlbumId((prev) => (prev === null ? ungrouped.id : prev));
  }, [albums]);
  // 子分组（B4 定案，可选）：所选相册的现有子分组名（datalist 提示）；换相册即清空
  const [albumSubgroup, setAlbumSubgroup] = useState("");
  const [albumSubgroupNames, setAlbumSubgroupNames] = useState<string[]>([]);
  useEffect(() => {
    setAlbumSubgroup("");
    setAlbumSubgroupNames([]);
    if (albumChoice !== "existing" || albumId === null) return;
    let cancelled = false;
    void albumSubgroups(albumId).then((list) => {
      if (!cancelled) setAlbumSubgroupNames(list.map((g) => g.name));
    });
    return () => {
      cancelled = true;
    };
  }, [albumChoice, albumId]);
  // 相册主组织（必选，时间/相册布局 2026-09-28 定案）：导入落
  // `照片根/{相册创建YYYY}/{相册创建MM}/{相册目录名}/`，相册内平铺不按日期分层
  //（应用内按拍摄日分组；相册导入下后端整体覆写 dir_template，预览不再拼库级模板）。
  // 实时预览目标路径（相册目录名 dir_name 缺省回退显示名；未选出时按「未分组」兜底；
  // 新建分支用输入名）。
  const selectedAlbum = albumChoice === "existing" ? albums.find((a) => a.id === albumId) : undefined;
  const ungroupedAlbum = albums.find((a) => isUngroupedAlbum(a));
  const albumDirForPreview =
    albumChoice === "existing"
      ? (selectedAlbum?.dirName ?? selectedAlbum?.name ?? UNGROUPED_ALBUM_NAME)
      : newAlbumName.trim() === ""
        ? UNGROUPED_ALBUM_NAME
        : newAlbumName.trim();
  // 外层年月 = 相册创建时间（只到月）：existing 分支取所选相册 createdAt（未选出回退
  // 「未分组」）；新建分支 = 导入当刻 YYYY/MM，空名回退「未分组」时若列表中已存在
  // 同名保底相册则用其真实 createdAt（后端按名幂等复用），没有再用当前日期。
  const nowForPreview = new Date();
  const currentDateForPreview = `${nowForPreview.getFullYear()}-${String(nowForPreview.getMonth() + 1).padStart(2, "0")}-01`;
  const albumCreatedAtForPreview =
    albumChoice === "existing"
      ? (selectedAlbum?.createdAt ?? ungroupedAlbum?.createdAt ?? currentDateForPreview)
      : newAlbumName.trim() === ""
        ? (ungroupedAlbum?.createdAt ?? currentDateForPreview)
        : currentDateForPreview;
  const albumYearForPreview = albumCreatedAtForPreview.slice(0, 4);
  const albumMonthForPreview = albumCreatedAtForPreview.slice(5, 7);
  const importTargetPreview = `${targetRoot}\\${albumYearForPreview}\\${albumMonthForPreview}\\${albumDirForPreview}`;
  // 双目的地（M2）：默认关；移动模式互斥（后端拒 move+secondTarget）
  const [secondEnabled, setSecondEnabled] = useState(false);
  const [secondRoot, setSecondRoot] = useState("");
  // 双目的地第二份预览：第二根目录 + 同公式（后端 engine 覆写 dir_template 后随之对齐）
  const secondImportTargetPreview = `${secondRoot.trim().replace(/[\\/]+$/, "")}\\${albumYearForPreview}\\${albumMonthForPreview}\\${albumDirForPreview}`;
  const [starting, setStarting] = useState(false);
  // 启动失败文案：优先透出后端 Err；invoke 不可用时为通用文案（null → 用 i18n 兜底）
  const [startError, setStartError] = useState<string | null>(null);
  // 源选择失败文案（folderScan 失败等）：中栏一行红字，选中其他源时清除
  const [sourceError, setSourceError] = useState<string | null>(null);
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
  const [fsLoading, setFsLoading] = useState<Set<string>>(() => new Set());
  const fsLoadingRef = useRef<Set<string>>(new Set());
  useEffect(() => {
    let cancelled = false;
    void fsListDirs().then((roots) => {
      if (!cancelled) setFsRoots(roots);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  // 设备初值补拉：后端启动枚举的 deviceScanned 事件早于 webview 订阅，
  // 错过事件的在位设备（如已连接的相机）从这里直接出现
  useEffect(() => {
    void seedDevicesFromBackend();
  }, []);

  const isMtp = device?.kind === "mtp";
  // 并发流数是库属性（设置页/新建库改）：向导只读合成；MTP 受协议限制恒 1
  const effectiveStreams = isMtp ? 1 : activeLibrary?.streams ?? 4;
  // 双目的地开启且第二目标根目录为空 → 必填校验拦住开始
  const secondReady = !secondEnabled || secondRoot.trim().length > 0;
  const canStart =
    Boolean(device && activeLibrary && targetRoot) && device?.scanStatus !== "scanning" && device?.scanStatus !== "failed" && secondReady && !starting;

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

  /** 手动从设备列表选择：显式选中 + 切 URL + 记入最近使用 */
  function selectDevice(id: string): void {
    const snapshot = devices.find((d) => d.id === id);
    if (snapshot) recordRecentSource({ id: snapshot.id, name: snapshot.name, kind: snapshot.kind });
    setExplicitId(id);
    setSourceError(null);
    setSearchParams(id ? { device: id } : {});
  }

  /** 选中文件夹为源：folderScan 成快照则入库并显式选中；失败（IPC 不可用等）提示并保持原源 */
  async function selectFolder(path: string): Promise<boolean> {
    let snapshot: DeviceSnapshot | null;
    try { snapshot = await folderScan(path, true); }
    catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setSourceError(`${t("wizard.fs.scanFailed", { dir: path })}：${message}`);
      return false;
    }
    if (!snapshot) {
      // 失败不再静默：中栏一行红字提示（此前「点了没反应」难排查）
      setSourceError(t("wizard.fs.scanFailed", { dir: path }));
      return false;
    }
    addDevice(snapshot);
    recordRecentSource({ id: snapshot.id, name: snapshot.name, kind: snapshot.kind });
    setExplicitId(snapshot.id);
    setSourceError(null);
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
      setExplicitId(entry.id);
      setSourceError(null);
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
    if (path in fsChildren || fsLoadingRef.current.has(path)) return;
    fsLoadingRef.current.add(path);
    setFsLoading(new Set(fsLoadingRef.current));
    try {
      const children = await fsListDirs(path);
      setFsChildren((prev) => ({ ...prev, [path]: children }));
    } finally {
      fsLoadingRef.current.delete(path);
      setFsLoading(new Set(fsLoadingRef.current));
    }
  }

  // --- 方案与启动 ----------------------------------------------------------------

  async function startImport(): Promise<void> {
    if (!device || !canStart) return;
    // 相册必选（规格修订）：存入已有相册需选中；新建分支先建相册拿 id（重名等错误透出行内提示并中止启动）
    let resolvedAlbumId: number | undefined;
    if (albumChoice === "existing") {
      if (albumId === null) {
        setAlbumError(t("wizard.album.required"));
        return;
      }
      resolvedAlbumId = albumId;
    } else {
      const name = newAlbumName.trim();
      if (name === "") {
        setAlbumError(t("wizard.album.nameRequired"));
        return;
      }
      const created = await albumCreate(name);
      if (!created.ok) {
        setAlbumError(created.error ?? t("albums.createFailed"));
        return;
      }
      resolvedAlbumId = created.album.id;
      setAlbums((prev) => [created.album, ...prev]);
      setAlbumId(created.album.id);
    }
    setAlbumError(null);
    setStarting(true);
    setStartError(null);
    const plan: ImportPlan = {
      sourceId: device.id,
      targetRoot,
      // 过渡期兼容占位（后端 ImportPlan.dir_template 必填）：相册导入下后端
      // begin 阶段整体覆写为 {相册创建YYYY}/{MM}/{dir_name}，此值不影响落位
      dirTemplate: FIXED_PLAN_DIR_TEMPLATE,
      nameTemplate: "{原文件名}",
      duplicatePolicy,
      skipImported,
      streams: effectiveStreams,
      mode,
      secondTarget:
        secondEnabled && secondRoot.trim()
          ? { targetRoot: secondRoot.trim(), dirTemplate: FIXED_PLAN_DIR_TEMPLATE }
          : undefined,
      // 勾选即范围：只导入选中的文件（rel_path 集合），引擎按此过滤
      include: files.filter((f) => selected.has(f.path)).map((f) => f.path),
      // 添加到相册（可选）：导入完成后新入库照片加入该相册
      albumId: resolvedAlbumId,
      // 子分组（B4 定案，可选）：留空 = 相册根
      albumSubgroup:
        albumChoice === "existing" && albumSubgroup.trim() !== ""
          ? albumSubgroup.trim()
          : undefined,
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
    <div className="flex h-full flex-col bg-bg">
      <header className="flex h-[68px] shrink-0 items-center gap-4 border-b border-edge bg-surface/70 px-5">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <h1 className="text-base font-semibold tracking-tight text-text-primary">{t("wizard.title")}</h1>
          </div>
          <p className="mt-0.5 truncate text-[11px] text-text-muted">
            {t(mode === "copy" ? "wizard.mode.copyDesc" : "wizard.mode.moveDesc")}
          </p>
        </div>
        {!isIpcAvailable() && (
          <span className="rounded bg-panel px-1.5 py-0.5 text-[11px] text-text-muted">
            {t("wizard.ipcUnavailable")}
          </span>
        )}
        <div
          className="ml-auto flex items-center rounded-lg border border-edge bg-bg/80 p-1 shadow-sm"
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
              className={`rounded-md px-4 py-1.5 text-xs font-medium transition-colors ${
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

      <div
        className="grid min-h-0 flex-1 gap-0 p-3"
        style={{
          gridTemplateColumns: `${colWidths.left}px 6px minmax(0, 1fr) 6px ${colWidths.right}px`,
        }}
        data-testid="wizard-columns"
      >
        {/* 左栏：源面板（设备 / 文件系统树 / 最近使用，三区可折叠）+ 源文件树 */}
        <section className="flex min-h-0 flex-col overflow-hidden rounded-xl border border-edge bg-surface shadow-sm" aria-label={t("wizard.leftPane")}>
          {/* 设备区（可折叠）：设备列表（可点切换，与树选中态同样式）+ 选中设备信息卡 */}
          <PanelSection
            sectionKey="devices"
            title={t("wizard.section.devices")}
            collapsed={panelCollapse.devices}
            onToggle={() => togglePanelSection("devices")}
          >
            <div className="px-3 pb-3">
              {visibleDevices.length === 0 ? (
                <p className="py-4 text-center text-xs leading-relaxed text-text-muted">
                  {t("wizard.noDevice")}
                </p>
              ) : (
                <>
                  {/* 设备列表：行=图标+名称+文件数徽标；点击=选中该设备（中栏立即切换清单） */}
                  <div className="flex flex-col gap-0.5" data-testid="wizard-device-list">
                    {visibleDevices.map((d) => {
                      const isSelected = d.id === selectedId;
                      return (
                        <button
                          key={d.id}
                          type="button"
                          onClick={() => selectDevice(d.id)}
                          className={`flex w-full items-center gap-1.5 rounded border-l-2 py-1 pl-2 pr-1.5 text-left transition-colors ${
                            isSelected
                              ? "border-accent bg-accent/10"
                              : "border-transparent hover:bg-panel/40"
                          }`}
                          data-testid="wizard-device-item"
                          data-device-id={d.id}
                          data-selected={isSelected}
                          title={d.id}
                        >
                          <DeviceGlyph
                            kind={devicePresentationKind(d)}
                            size={13}
                            className={isSelected ? "text-accent" : "text-text-muted"}
                          />
                          <span
                            className={`min-w-0 flex-1 truncate text-xs ${
                              isSelected ? "font-medium text-accent" : "text-text-secondary"
                            }`}
                          >
                            {d.name}
                          </span>
                          <span className="ml-auto shrink-0 rounded bg-panel px-1.5 py-0.5 text-[10px] text-text-muted tabular-nums">
                            {deviceBadge(d, t)}
                          </span>
                        </button>
                      );
                    })}
                  </div>

                  {/* 选中设备详细信息（仅当前选中的真实设备；选中文件夹时隐藏——
                      其选中态由树高亮表达，统计在中栏，设备区不重复呈现）+ 重新扫描 */}
                  {device && device.kind !== "folder" && (
                    <>
                      <div className="mt-2.5 flex justify-end">
                        <button
                          type="button"
                          onClick={() => device && void refreshDevice(device.id)}
                          className="rounded-md border border-edge px-2 py-0.5 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
                          title={t("wizard.rescan")}
                        >
                          {t("wizard.rescan")}
                        </button>
                      </div>
                      {device.scanError && <p role="status" className="mt-2 text-xs text-red-400">{t("wizard.deviceScanFailed")}: {device.scanError}</p>}
                      <dl className="mt-1.5 space-y-1 text-xs" data-testid="wizard-device-info">
                        <div className="flex justify-between">
                          <dt className="text-text-muted">{t("wizard.deviceKind")}</dt>
                          <dd className="flex items-center gap-1 text-text-secondary">
                            <DeviceGlyph kind={devicePresentationKind(device)} size={12} className="text-text-secondary" />
                            {t(deviceKindLabelKey(device))}
                          </dd>
                        </div>
                        {(["photo", "raw"] as const).map((kind) => (
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
                    </>
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
            <div className="h-52 min-h-0">
              {fsRoots === null ? (
                <div className="flex h-full items-center justify-center gap-2 text-[11px] text-text-muted">
                  <span className="h-3 w-3 animate-spin rounded-full border border-text-muted border-t-accent" />
                  {t("wizard.fs.loading")}
                </div>
              ) : fsRoots.length === 0 ? (
                <p className="px-2 py-1 text-[11px] leading-relaxed text-text-muted">
                  {t("wizard.fs.unavailable")}
                </p>
              ) : (
                <FileSystemTree
                  roots={fsRoots}
                  expanded={fsExpanded}
                  children={fsChildren}
                  loading={fsLoading}
                  selectedPath={selectedFolderPath}
                  onToggle={(node) => void toggleFsNode(node)}
                  onSelect={(path) => void selectFolder(path)}
                />
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
            <SourceTree groups={groups} collapsed={collapsed} selected={selected}
              onToggleGroup={toggleGroup} onToggleCollapse={toggleCollapse} onToggleFile={toggleFile} />
          </PanelSection>
        </section>

        {/* 左|中 列宽拖动条 */}
        <ColumnResizeHandle
          side="left"
          onDelta={(dx) => resizeColumn("left", dx)}
          onReset={resetColumns}
        />

        {/* 中栏：文件区（列表/缩略图双视图，共享勾选与统计；工具栏=统计+全选/反选） */}
        <section className="flex min-h-0 flex-col overflow-hidden rounded-xl border border-edge bg-surface shadow-sm" aria-label={t("wizard.fileTable")}>
          <div
            className="flex min-h-12 shrink-0 items-center justify-between gap-3 border-b border-edge bg-panel/20 px-3 text-xs"
            data-testid="wizard-table-stats"
          >
            <div className="min-w-0">
              <p className="truncate text-xs font-medium text-text-primary">{t("wizard.previewTitle")}</p>
              <p className="mt-0.5 text-[11px] text-text-muted tabular-nums">
                {t("wizard.selectedStats", { selected: selectedCount, total: files.length, size: formatBytes(selectedBytes) })}
              </p>
            </div>
            <div className="flex shrink-0 items-center gap-2">
              <ViewControls viewMode={viewMode} tileSize={tileSize}
                onViewMode={(next) => { setViewMode(next); saveViewMode(next); }}
                onTileSize={(next) => { setTileSize(next); saveTileSize(next); }} />
              <span className="mx-0.5 h-5 w-px bg-edge" aria-hidden="true" />
              <button
                type="button"
                onClick={selectAll}
                className="rounded-md px-1.5 py-1 text-[11px] text-text-muted transition-colors hover:bg-panel hover:text-accent"
              >
                {t("wizard.selectAll")}
              </button>
              <button
                type="button"
                onClick={invertSelection}
                className="rounded-md px-1.5 py-1 text-[11px] text-text-muted transition-colors hover:bg-panel hover:text-accent"
              >
                {t("wizard.invert")}
              </button>
            </div>
          </div>
          {sourceError && (
            <p className="shrink-0 border-b border-edge px-3 py-1.5 text-[11px] text-red-400" role="alert" data-testid="wizard-source-error">
              {sourceError}
            </p>
          )}
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
        <section className="flex min-h-0 flex-col overflow-y-auto rounded-xl border border-edge bg-surface p-4 shadow-sm" aria-label={t("wizard.planPane")}>
          <h2 className="text-sm font-semibold text-text-primary">{t("wizard.plan")}</h2>
          <p className="mt-1 text-[11px] leading-relaxed text-text-muted">{t("wizard.planHint")}</p>

          {/* 导入位置（库属性，只读）：目标根/模板随库走，在设置中修改 */}
          <div className="mt-5 flex flex-col gap-1.5">
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
                      {t("wizard.location.layout")}
                    </span>
                    <span
                      className="truncate font-mono text-[11px] text-text-secondary"
                      title={FIXED_ALBUM_LAYOUT}
                      data-testid="wizard-preview"
                    >
                      {FIXED_ALBUM_LAYOUT}
                    </span>
                  </div>
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

          {/* 存入相册（必选，规格修订）：已有相册（默认预选「未分组」）/ 新建；
              albumId 随导入启动负载下发（后端 None 报错） */}
          <fieldset className="mt-4 flex flex-col gap-1" data-testid="wizard-album-section">
            <legend className="mb-1 text-xs font-medium text-text-secondary">
              {t("wizard.album.label")}
            </legend>
            <label className="flex cursor-pointer items-center gap-2 text-xs text-text-secondary">
              <input
                type="radio"
                name="wizard.albumChoice"
                value="existing"
                checked={albumChoice === "existing"}
                onChange={() => {
                  setAlbumChoice("existing");
                  setAlbumError(null);
                }}
                className="h-3 w-3 accent-[#F0A83C]"
                data-testid="wizard-album-existing"
              />
              {t("wizard.album.existing")}
            </label>
            {albumChoice === "existing" && (
              <select
                value={albumId === null ? "" : String(albumId)}
                onChange={(e) => setAlbumId(e.target.value === "" ? null : Number(e.target.value))}
                aria-label={t("wizard.album.existing")}
                className="ml-5 rounded-md border border-edge bg-bg px-2 py-1.5 text-xs text-text-primary outline-none transition-colors focus:border-accent"
                data-testid="wizard-album-select"
              >
                <option value="">{t("wizard.album.selectPlaceholder")}</option>
                {albums.map((album) => (
                  <option key={album.id} value={album.id}>
                    {album.name}（{album.itemCount}）
                  </option>
                ))}
              </select>
            )}
            {albumChoice === "existing" && albumId !== null && (
              <input
                type="text"
                value={albumSubgroup}
                onChange={(e) => setAlbumSubgroup(e.target.value)}
                list="wizard-subgroup-options"
                placeholder={t("wizard.album.subgroupPlaceholder")}
                aria-label={t("wizard.album.subgroup")}
                className="ml-5 rounded-md border border-edge bg-bg px-2 py-1.5 text-xs text-text-primary outline-none transition-colors placeholder:text-text-muted/60 focus:border-accent"
                data-testid="wizard-album-subgroup"
              />
            )}
            {albumChoice === "existing" && albumId !== null && (
              <datalist id="wizard-subgroup-options">
                {subgroupSuggestions(albumSubgroupNames).map((name) => (
                  <option key={name} value={name} />
                ))}
              </datalist>
            )}
            <label className="flex cursor-pointer items-center gap-2 text-xs text-text-secondary">
              <input
                type="radio"
                name="wizard.albumChoice"
                value="new"
                checked={albumChoice === "new"}
                onChange={() => {
                  setAlbumChoice("new");
                  setAlbumError(null);
                }}
                className="h-3 w-3 accent-[#F0A83C]"
                data-testid="wizard-album-new"
              />
              {t("wizard.album.new")}
            </label>
            {albumChoice === "new" && (
              <input
                type="text"
                value={newAlbumName}
                onChange={(e) => {
                  setNewAlbumName(e.target.value);
                  setAlbumError(null);
                }}
                placeholder={t("albums.newNamePlaceholder")}
                aria-label={t("wizard.album.new")}
                className="ml-5 rounded-md border border-edge bg-bg px-2 py-1.5 text-xs text-text-primary outline-none transition-colors placeholder:text-text-muted/60 focus:border-accent"
                data-testid="wizard-album-new-name"
              />
            )}
            {albumError !== null && (
              <p className="ml-5 text-[11px] text-red-400" role="alert" data-testid="wizard-album-error">
                {albumError}
              </p>
            )}
            {/* 导入位置实时预览（时间/相册布局定案）：目标 = 照片根/{相册创建YYYY}/{相册创建MM}/{相册目录名}，
                相册内平铺不按日期分层（应用内按拍摄日分组），随相册选择/输入即时更新 */}
            <p className="ml-5 mt-1 text-[11px] text-text-muted" data-testid="wizard-album-path-preview">
              {t("wizard.album.pathPreview")}：
              <span className="break-all font-mono text-text-secondary">
                {importTargetPreview}\
              </span>
            </p>
            <p className="ml-5 text-[11px] leading-relaxed text-text-muted" data-testid="wizard-album-flat-note">
              {t("wizard.album.flatNote")}
            </p>
          </fieldset>

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
                {secondRoot.trim() !== "" && (
                  <p
                    className="break-all font-mono text-[11px] text-text-secondary"
                    data-testid="wizard-second-path-preview"
                    title={secondImportTargetPreview}
                  >
                    {secondImportTargetPreview}\
                  </p>
                )}
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
