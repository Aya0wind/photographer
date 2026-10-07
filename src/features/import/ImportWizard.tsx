import {
  type WizardViewMode,
  type PanelSectionKey,
  type TileSizeKey,
  TILE_SIZE_ORDER,
  TILE_SIZE_ICON,
  useImportLayout,
  TILE_SIZE_SPECS,
} from "./lib/useImportLayout";
import {
  type DevicePresentationKind,
  deviceKindLabelKey,
  devicePresentationKind,
} from "./devicePresentation";
import {
  type DeviceSnapshot,
  deviceFiles,
  platformCapabilities,
  kindFromName,
  type AlbumDto,
  albumList,
  albumSubgroups,
  type ImportMode,
  type FsDirEntry,
  fsListDirs,
  folderScan,
  albumCreate,
  type ImportPlan,
  type PlatformCapabilities,
  FIXED_PLAN_DIR_TEMPLATE,
  importStart,
  isIpcAvailable,
} from "@/ipc/api";
import { getIntlLocale } from "@/i18n";
import { directoryPreview } from "@/lib/filesystemPaths";
import { useState, useRef, useEffect, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate, useSearchParams } from "react-router";
import { useImportStore, seedDevicesFromBackend, type RecentSource } from "@/stores/importStore";
import { useSettingsStore } from "@/stores/settingsStore";
import {
  groupByDir,
  normalizeFsPath,
  type DirGroup,
  FolderGlyph,
  KIND_LABEL_COLOR,
  FileSystemTree,
  SourceTree,
  sourceBasePath,
  FileListView,
  FileGridView,
} from "./SourceFileViews";
import { importRootOf } from "@/features/onboarding/onboardingConfig";
import {
  isUngroupedAlbum,
  UNGROUPED_ALBUM_NAME,
  subgroupSuggestions,
} from "@/features/albums/lib/ungroupedAlbum";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { formatBytes } from "@/lib/format";
import ErrorModal from "@/shared/components/ErrorModal";
export { fetchThumbUrl, resetThumbCacheForTests } from "./lib/sourceThumbs";

export { VIEW_MODE_STORAGE_KEY, PANEL_COLLAPSE_KEY, COL_WIDTHS_KEY, TILE_SIZE_KEY } from "./lib/useImportLayout";
export type { WizardViewMode } from "./lib/useImportLayout";

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
  if (d.mediaPresent === false) return t("wizard.noCard");
  if (d.scanStatus === "scanning") return t("wizard.deviceScanning");
  if (d.scanStatus === "failed") return t("wizard.deviceScanFailed");
  const total = Object.values(d.filesByKind).reduce((sum, n) => sum + n, 0);
  return total > 0 ? `${total.toLocaleString(getIntlLocale())} ${t("wizard.deviceFiles")}` : t(deviceKindLabelKey(d));
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
  const explicitValid = explicitId !== null && devices.some((d) => d.id === explicitId && d.mediaPresent !== false);
  const urlValid = urlDevice !== null && devices.some((d) => d.id === urlDevice && d.mediaPresent !== false);
  const selectedId = explicitValid
    ? (explicitId as string)
    : urlValid
      ? (urlDevice as string)
      : (devices.find((d) => d.mediaPresent !== false)?.id ?? null);
  const device = devices.find((d) => d.id === selectedId) ?? null;

  // 选中源变化时拉取文件清单（device_files）；已有缓存的源不重复拉取。
  // 扫描中由文件增量填充；仅已完成且没有缓存的源使用兼容读取。
  const [filesLoading, setFilesLoading] = useState(false);
  useEffect(() => {
    if (!selectedId || device?.mediaPresent === false || device?.scanStatus === "scanning" || device?.scanStatus === "failed") {
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
          return { objectId: e.id, mtime: e.mtime, path: e.relPath, dir, name, size: e.size, kind: kindFromName(name) };
        }),
      );
    });
    return () => {
      cancelled = true;
    };
    // sourceFilesMap[selectedId] 变为存在即触发跳过分支，无需进依赖
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedId, device?.mediaPresent, device?.scanStatus, sourceFilesMap[selectedId ?? ""]]);

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
  const [platformCaps, setPlatformCaps] = useState<PlatformCapabilities | null>(null);
  useEffect(() => {
    let cancelled = false;
    void platformCapabilities()
      .then((caps) => {
        if (!cancelled) setPlatformCaps(caps);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);
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
  const importTargetPreview = directoryPreview(
    targetRoot, albumYearForPreview, albumMonthForPreview, albumDirForPreview,
  );
  // 双目的地（M2）：默认关；移动模式互斥（后端拒 move+secondTarget）
  const [secondEnabled, setSecondEnabled] = useState(false);
  const [secondRoot, setSecondRoot] = useState("");
  // 双目的地第二份预览：第二根目录 + 同公式（后端 engine 覆写 dir_template 后随之对齐）
  const secondImportTargetPreview = directoryPreview(
    secondRoot.trim(), albumYearForPreview, albumMonthForPreview, albumDirForPreview,
  );
  const [starting, setStarting] = useState(false);
  // 启动失败文案：优先透出后端 Err；invoke 不可用时为通用文案（null → 用 i18n 兜底）
  const [startError, setStartError] = useState<string | null>(null);
  // 源选择失败通过确认对话框展示，选中其他源时清除。
  const [sourceError, setSourceError] = useState<string | null>(null);
  // 顶部导入模式：复制、移动或只在数据库登记原文件。
  const [mode, setMode] = useState<ImportMode>("copy");
  useEffect(() => {
    if (device?.kind === "mtp" && mode === "reference") setMode("copy");
  }, [device?.kind, mode]);
  const { viewMode, setViewMode, panelCollapse, colWidths, tileSize, setTileSize,
    togglePanelSection, resizeColumn, resetColumns } = useImportLayout();

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
    Boolean(device && activeLibrary && targetRoot) && device?.scanStatus !== "scanning" && device?.scanStatus !== "failed" && !(mode === "reference" && isMtp) && secondReady && selected.size > 0 && !starting;

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
      // 失败不再静默：确认对话框明确提示。
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
        mode === "copy" && secondEnabled && secondRoot.trim()
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
    // 导入任务已成功启动；偏好保存失败由 store 记录，不能令任务重复启动。
    void save(useSettingsStore.getState().settings).catch(() => {});
    // LR 式后台导入：点了导入立即回画廊继续浏览，进度由全局右下角进度卡常驻呈现
    navigate("/gallery");
  }

  return (
    <div className="flex h-full flex-col bg-bg">
      <header className="flex min-h-[68px] shrink-0 flex-wrap items-center gap-4 border-b border-edge bg-surface/70 px-5">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <h1 className="text-base font-semibold tracking-tight text-text-primary">{t("wizard.title")}</h1>
          </div>
          <p className="mt-0.5 truncate text-[11px] text-text-muted">
            {t("wizard.flowHint")}
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
          {(["copy", "move", "reference"] as const).map((option) => (
            <button
              key={option}
              type="button"
              role="radio"
              aria-checked={mode === option}
              disabled={option === "reference" && device?.kind === "mtp"}
              onClick={() => {
                setMode(option);
                if (option !== "copy") setSecondEnabled(false);
              }}
              className={`rounded-md px-4 py-1.5 text-xs font-medium transition-colors ${
                mode === option
                  ? "bg-accent text-black"
                  : "text-text-secondary hover:text-text-primary disabled:cursor-not-allowed disabled:opacity-40"
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
          <div className="flex min-h-[64px] shrink-0 items-center gap-2 border-b border-edge px-3">
            <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-accent/15 text-xs font-semibold text-accent">1</span>
            <h2 className="text-sm font-semibold text-text-primary">{t("wizard.chooseSource")}</h2>
            <button type="button" onClick={() => void browseFolder()} data-testid="wizard-choose-folder"
              className="ml-auto flex items-center gap-1 rounded-md border border-accent/40 bg-accent/10 px-2 py-1.5 text-xs font-medium text-accent hover:bg-accent/20">
              <FolderGlyph size={14} />{t("wizard.chooseFolder")}
            </button>
          </div>
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
                  {platformCaps &&
                  !platformCaps.volumeDevices &&
                  !platformCaps.portableDevices
                    ? t("wizard.noNativeDevice")
                    : t("wizard.noDevice")}
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
                          disabled={d.mediaPresent === false}
                          className={`flex w-full items-center gap-1.5 rounded border-l-2 py-1 pl-2 pr-1.5 text-left transition-colors ${
                            isSelected
                              ? "border-accent bg-accent/10"
                              : "border-transparent hover:bg-panel/40 disabled:opacity-60 disabled:cursor-default"
                          }`}
                          data-testid="wizard-device-item"
                          data-device-id={d.id}
                          data-selected={isSelected}
                          title={d.mediaPresent === false ? t("wizard.noCardHint") : d.id}
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
            className="flex min-h-[64px] shrink-0 flex-wrap items-center justify-between gap-2 py-2 border-b border-edge bg-panel/20 px-3 text-xs"
            data-testid="wizard-table-stats"
          >
            <div className="min-w-0">
              <div className="flex items-center gap-2">
                <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-accent/15 text-xs font-semibold text-accent">2</span>
                <p className="truncate text-sm font-semibold text-text-primary">{t("wizard.choosePhotos")}</p>
              </div>
              <p className="mt-0.5 text-[11px] text-text-muted tabular-nums">
                {t("wizard.selectedStats", { selected: selectedCount, total: files.length, size: formatBytes(selectedBytes) })}
              </p>
            </div>
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              <ViewControls viewMode={viewMode} tileSize={tileSize}
                onViewMode={setViewMode}
                onTileSize={setTileSize} />
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
          {device && (
            <div className="flex shrink-0 items-center gap-2 border-b border-edge bg-bg/40 px-3 py-2 text-xs" data-testid="wizard-current-source">
              <DeviceGlyph kind={devicePresentationKind(device)} className="text-accent" />
              <span className="min-w-0 flex-1 truncate text-text-secondary" title={sourceBasePath(device) ?? device.name}>{device.name}</span>
              {device.scanStatus === "scanning" && <span role="status" className="flex shrink-0 items-center gap-1.5 text-accent"><span className="h-3 w-3 animate-spin rounded-full border border-accent/30 border-t-accent" />{t("wizard.scanningPhotos", { count: files.length })}</span>}
            </div>
          )}
          {files.length === 0 ? (
            <div className="flex min-h-0 flex-1 flex-col items-center justify-center px-6 py-10 text-center">
              <FolderGlyph size={36} className="mb-4 text-text-muted" />
              <p className="text-sm font-medium leading-relaxed text-text-secondary">
                {(filesLoading || device?.scanStatus === "scanning") && device
                  ? t("wizard.tableLoading")
                  : device
                    ? t("wizard.tableEmpty")
                    : t("wizard.selectSourceHint")}
              </p>
              {!device && <>
                <p className="mt-2 max-w-sm text-xs leading-relaxed text-text-muted">{t("wizard.sourceHelp")}</p>
                <button type="button" onClick={() => void browseFolder()} className="mt-5 rounded-lg bg-accent px-5 py-2 text-sm font-semibold text-black hover:brightness-110">{t("wizard.chooseFolder")}</button>
              </>}
            </div>
          ) : viewMode === "list" ? (
            <FileListView
              groups={groups}
              collapsed={collapsed}
              selected={selected}
              onToggleFile={toggleFile}
              onToggleGroup={toggleGroup}
              onToggleCollapse={toggleCollapse}
              rootDirLabel={t("wizard.rootDir")}
            />
          ) : (
            <FileGridView
              groups={groups}
              collapsed={collapsed}
              selected={selected}
              onToggleFile={toggleFile}
              onToggleGroup={toggleGroup}
              onToggleCollapse={toggleCollapse}
              basePath={sourceBasePath(device)}
              rootDirLabel={t("wizard.rootDir")}
              tile={TILE_SIZE_SPECS[tileSize]}
              device={device}
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
        <section className="flex min-h-0 flex-col overflow-hidden rounded-xl border border-edge bg-surface shadow-sm" aria-label={t("wizard.planPane")}>
          <div className="flex min-h-[64px] shrink-0 items-center gap-2 border-b border-edge px-4">
            <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-accent/15 text-xs font-semibold text-accent">3</span>
            <h2 className="text-sm font-semibold text-text-primary">{t("wizard.saveAndImport")}</h2>
          </div>
          <div className="sp-scroll min-h-0 flex-1 overflow-y-auto px-4 pb-4">
          <p className="mt-4 text-xs leading-relaxed text-text-muted">{t("wizard.planHint")}</p>

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
              {mode === "reference" ? (
                <span className="text-xs leading-relaxed text-text-secondary">{t("wizard.mode.referenceDesc")}</span>
              ) : activeLibrary ? (
                <span className="break-all font-mono text-xs text-text-primary" title={targetRoot}>
                  {targetRoot}
                </span>
              ) : (
                <p className="text-[11px] leading-relaxed text-text-muted">
                  {t("wizard.location.noLibrary")}
                </p>
              )}
            </div>
          </div>

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
            {/* 导入位置实时预览（时间/相册布局定案）：目标 = 照片根/{相册创建YYYY}/{相册创建MM}/{相册目录名}，
                相册内平铺不按日期分层（应用内按拍摄日分组），随相册选择/输入即时更新 */}
            <p className="ml-5 mt-1 text-[11px] text-text-muted" data-testid="wizard-album-path-preview">
              {t("wizard.album.pathPreview")}：
              <span className="break-all font-mono text-text-secondary">
                {mode === "reference" ? t("wizard.mode.reference") : importTargetPreview}
              </span>
            </p>
            <p className="ml-5 text-[11px] leading-relaxed text-text-muted" data-testid="wizard-album-flat-note">
              {t("wizard.album.flatNote")}
            </p>
          </fieldset>

          <details className="mt-5 rounded-lg border border-edge bg-bg/40 p-3" data-testid="wizard-advanced">
            <summary className="cursor-pointer text-xs font-medium text-text-secondary">{t("wizard.advanced")}</summary>
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
                mode !== "copy" ? "cursor-not-allowed text-text-muted" : "cursor-pointer text-text-secondary"
              }`}
              title={mode === "move" ? t("wizard.second.moveUnsupported") : undefined}
            >
              <input
                type="checkbox"
                checked={secondEnabled}
                disabled={mode !== "copy"}
                onChange={(e) => setSecondEnabled(e.target.checked)}
                className="h-3 w-3 accent-[#F0A83C] disabled:opacity-40"
                data-testid="wizard-second-toggle"
              />
              {t("wizard.second.label")}
            </label>
            <p className="pl-5 text-[11px] leading-relaxed text-text-muted">
              {mode !== "copy"
                ? t("wizard.second.moveUnsupported")
                : t("wizard.second.desc")}
            </p>
            {secondEnabled && mode === "copy" && (
              <div className="mt-1 flex flex-col gap-1.5" data-testid="wizard-second-panel">
    <div className="flex min-w-0 flex-wrap items-center gap-1.5">
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
                    {secondImportTargetPreview}
                  </p>
                )}
              </div>
            )}
          </div>

          </details>
          </div>

          {/* 并发流数已下沉库属性（设置页/新建库对话框修改），向导不再展示控件 */}

          <div className="shrink-0 border-t border-edge bg-panel/30 p-4" data-testid="wizard-import-summary">
            <p className={`mb-3 text-xs leading-relaxed ${mode === "move" ? "text-amber-300" : "text-text-secondary"}`}>
              {t(`wizard.mode.${mode}Desc`)}
            </p>
            <div className="mb-3 flex items-baseline justify-between gap-2">
              <span className="text-sm font-semibold text-text-primary">{t("wizard.readyCount", { count: selectedCount })}</span>
              <span className="text-xs text-text-secondary">{formatBytes(selectedBytes)}</span>
            </div>
            <p className="mb-3 text-[11px] leading-relaxed text-text-muted" role="status">
              {!device ? t("wizard.selectSourceHint") : !activeLibrary ? t("wizard.location.noLibrary")
                : device.scanStatus === "scanning" ? t("wizard.scanBeforeImport")
                : device.scanStatus === "failed" ? t("wizard.deviceScanFailed")
                : selectedCount === 0 ? t("wizard.selectPhotosHint")
                : !secondReady ? t("wizard.second.required") : t("wizard.backgroundHint")}
            </p>
            <button
              type="button"
              disabled={!canStart}
              aria-label={t("wizard.start")}
              onClick={() => void startImport()}
              className="w-full rounded-md bg-accent px-4 py-2 text-sm font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
            >
              {starting ? t("wizard.starting") : t("wizard.startCount", { count: selectedCount })}
            </button>
          </div>
        </section>
      </div>
      <ErrorModal
        message={sourceError ?? albumError ?? startError}
        onClose={() => { setSourceError(null); setAlbumError(null); setStartError(null); }}
      />
    </div>
  );
}
