import { type SourceFile } from "@/stores/importStore";
import { type FileKind, type DeviceSnapshot, deviceThumbGet, type FsDirEntry } from "@/ipc/api";
import { useTranslation } from "react-i18next";
import { type TileSizeSpec } from "./lib/useImportLayout";
import { useState, useRef, useEffect, useMemo } from "react";
import { fetchThumbUrl, cachedThumbUrl, THUMB_SIZE, thumbUrlCache } from "./lib/sourceThumbs";
import { formatBytes } from "@/lib/format";
import { useVirtualizer } from "@tanstack/react-virtual";

/** 来源文件树、虚拟化列表和缩略图网格共用选中文件与预览管线。 */

export interface DirGroup {
  dir: string;
  files: SourceFile[];
}

export function groupByDir(files: SourceFile[], scanning = false): DirGroup[] {
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

export const KIND_LABEL_COLOR: Record<FileKind, string> = {
  photo: "text-accent",
  raw: "text-sky-400",
  other: "text-text-muted",
};

/** 文件夹图标（stroke 风格与现有图标一致，16 viewBox） */
export function FolderGlyph({ size = 14, className = "" }: { size?: number; className?: string }) {
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
export function normalizeFsPath(path: string): string {
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

// --- 缩略图管线：asset 协议 + 解码并发信号量 --------------------------------------

/** 源根的文件系统绝对路径（folder=去 FOLDER: 前缀；volume=设备 id；MTP 无路径） */
export function sourceBasePath(device: DeviceSnapshot | null): string | null {
  if (!device) return null;
  if (device.kind === "folder") return device.id.slice("FOLDER:".length);
  if (device.kind === "volume") return device.id;
  return null;
}

/** 源内相对路径（dir + name 统一拼接，与设备枚举的 relPath 语义一致） */
function relPathOf(file: SourceFile): string {
  return file.dir ? `${file.dir}/${file.name}` : file.name;
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
  deviceId,
  previewReady = true,
}: {
  file: SourceFile;
  selected: boolean;
  onToggle: (path: string) => void;
  /** 本地照片/RAW 的绝对路径；MTP 使用独立的设备与对象 ID。 */
  absPath: string | null;
  size: TileSizeSpec;
  deviceId?: string;
  previewReady?: boolean;
}) {
  const { t } = useTranslation();
  // M2 起 img 一律读后端小图（~30KB），不再解码原图；onLoad 淡入，onError/15s 超时静默保持占位
  const [src, setSrc] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [pending, setPending] = useState(false);
  const settledRef = useRef(false);
  const [failed, setFailed] = useState(false);
  const [retry, setRetry] = useState(0);
  const version = `${file.mtime ?? ""}:${file.size}`;
  const previewKey = absPath ? JSON.stringify([absPath, version])
    : deviceId && file.objectId ? JSON.stringify([deviceId, file.objectId, version]) : null;

  useEffect(() => {
    let cancelled = false;
    setSrc(null);
    setLoaded(false);
    setFailed(false);
    setPending(false);
    settledRef.current = false;
    if (!previewKey || !previewReady) return;
    setPending(true);
    const request = absPath ? fetchThumbUrl(absPath, version, () => !cancelled)
      : cachedThumbUrl(previewKey, () => deviceThumbGet(deviceId!, file.objectId!, version, THUMB_SIZE), () => !cancelled);
    void request.then((url) => {
      // null=无缩略图或提取失败：保持占位
      if (cancelled) return;
      setPending(false);
      if (url === null) { setFailed(true); return; }
      setSrc(url);
    });
    return () => {
      cancelled = true;
    };
  }, [previewKey, previewReady, retry]);

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
    else {
      setSrc(null);
      setFailed(true);
      if (previewKey) thumbUrlCache.delete(previewKey);
    }
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
            loading="eager"
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
        {!loaded && (
          <div className="absolute bottom-2 left-2 right-2 flex items-center justify-center gap-2 rounded-md bg-black/55 px-2 py-1 text-[11px] text-white/80" role="status">
            {thumbLoading ? t("wizard.thumbLoading") : !previewReady ? t("wizard.thumbWaiting") : t("wizard.thumbUnavailable")}
            {failed && previewKey && (
              <button type="button" className="shrink-0 font-medium text-accent hover:underline"
                onClick={(event) => { event.stopPropagation(); setRetry((value) => value + 1); }}>
                {t("wizard.retry")}
              </button>
            )}
          </div>
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
        className="flex flex-col justify-center gap-0.5 px-1.5"
        style={{ height: size.infoH }}
      >
        <span className="w-full truncate text-[11px] text-text-secondary" title={file.name}>
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
  selectedCount,
  onToggleSelection,
  isCollapsed,
  onToggleCollapse,
  testId,
}: {
  label: string;
  dir: string;
  count: number;
  selectedCount: number;
  onToggleSelection: () => void;
  isCollapsed: boolean;
  onToggleCollapse: (dir: string) => void;
  testId: string;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex h-[26px] items-center rounded bg-panel/30">
      <input type="checkbox" checked={count > 0 && selectedCount === count}
        ref={(el) => { if (el) el.indeterminate = selectedCount > 0 && selectedCount < count; }}
        onChange={onToggleSelection} aria-label={t("wizard.groupToggle", { dir: label })}
        className="ml-2 h-3.5 w-3.5 shrink-0 cursor-pointer accent-[#F0A83C]" />
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

export function SourceTree({ groups, collapsed, selected, onToggleGroup, onToggleCollapse, onToggleFile }: {
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
export function FileSystemTree({
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

export function FileListView({
  groups,
  collapsed,
  selected,
  onToggleFile,
  onToggleGroup,
  onToggleCollapse,
  rootDirLabel,
}: {
  groups: DirGroup[];
  collapsed: Set<string>;
  selected: Set<string>;
  onToggleFile: (path: string) => void;
  onToggleGroup: (group: DirGroup) => void;
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
                    selectedCount={row.group.files.filter((f) => selected.has(f.path)).length}
                    onToggleSelection={() => onToggleGroup(row.group)}
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
  | { type: "header"; group: DirGroup }
  | { type: "tiles"; files: SourceFile[] };

export function FileGridView({
  groups,
  collapsed,
  selected,
  onToggleFile,
  onToggleGroup,
  onToggleCollapse,
  basePath,
  rootDirLabel,
  tile,
  device,
}: {
  groups: DirGroup[];
  collapsed: Set<string>;
  selected: Set<string>;
  onToggleFile: (path: string) => void;
  onToggleGroup: (group: DirGroup) => void;
  onToggleCollapse: (dir: string) => void;
  basePath: string | null;
  rootDirLabel: string;
  tile: TileSizeSpec;
  device: DeviceSnapshot | null;
}) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(0);

  // 动态列数：容器宽（滚动区）自适应；ResizeObserver 跟踪
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const update = () => { setWidth(el.clientWidth); setViewportHeight(el.clientHeight); };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const tileRowH = tile.thumbH + tile.infoH + 4 + GRID_GAP;
  const columns = Math.max(1, Math.floor((width - GRID_GAP) / (tile.width + GRID_GAP)));

  const rows = useMemo<GridRow[]>(() => {
    const out: GridRow[] = [];
    for (const g of groups) {
      out.push({ type: "header", group: g });
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
    getItemKey: (i) => rows[i].type === "header" ? `dir:${rows[i].group.dir}` : `tiles:${rows[i].files[0].path}`,
    // 前后各预加载一屏；仅虚拟列表范围内的图片会申请预览。
    overscan: Math.max(3, Math.ceil(viewportHeight / tileRowH)),
  });
  useEffect(() => { virtualizer.measure(); }, [columns, tileRowH]);

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
                    label={row.group.dir || rootDirLabel}
                    dir={row.group.dir}
                    count={row.group.files.length}
                    selectedCount={row.group.files.filter((f) => selected.has(f.path)).length}
                    onToggleSelection={() => onToggleGroup(row.group)}
                    isCollapsed={collapsed.has(row.group.dir)}
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
                          basePath !== null ? `${basePath}/${relPathOf(f)}` : null
                        }
                        deviceId={device?.kind === "mtp" ? device.id : undefined}
                        previewReady={device?.scanStatus !== "scanning"}
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
