import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type Ref,
} from "react";
import { useTranslation } from "react-i18next";
import { useVirtualizer } from "@tanstack/react-virtual";

import { assetRatingSet, type AssetDto } from "@/ipc/api";
import { SIMILARITY_BADGE_CLASS, similarityTier } from "@/features/ai/scoreBadge";
import { asColorLabel, COLOR_DOT_CLASS, COLOR_DOT_RING } from "../lib/colorLabels";
import {
  UNKNOWN_GROUP_KEY,
  formatDateLabel,
  type AssetGroup,
} from "../lib/assetGroups";
import AssetThumb from "./AssetThumb";

/**
 * 日期分组照片墙（画廊/搜索/人物页共用）：
 *
 * 布局双模式（M4.5 A4）：
 * - square（默认，兼容视图）：等宽方格（tile×tile），搜索/人物页等继续使用
 * - justify（画廊默认）：统一行高的 justify 网格——行内按 width/height 宽高比
 *   分配宽度（经典贪心算法：逐项累加，行高跌破目标即封行；组尾行不足整行按
 *   目标行高左对齐），无尺寸资产按 4:3 兜底；tile 语义变为「目标行高」
 *
 * 组头折叠（M4.5）：chevron 点击折叠为一行 36px 头（再点展开）；状态存组件本地。
 *
 * 多选（M4.5）：selection.active 时点击瓦片=切换选中（高亮描边+左上角序号角标）；
 * Ctrl/Cmd+点击随时进入多选（onCtrlClick）；长按 500ms 同理（onLongPress）。
 *
 * 虚拟化：TanStack Virtual 行模型（组头行 40px/折叠 36px + 内容行自带高度），
 * 行划分与行高在 useMemo 重算。吸顶组头由上层以覆盖条实现；本组件通过
 * onViewportChange 上报「视口首行所属组 + scrollTop」。
 */

/** 默认方格边长（大档；两档切换见 useGalleryTileSize） */
const TILE = 200;
const GAP = 4;
const HEADER_H = 40;
/** 折叠态组头高度 */
const COLLAPSED_HEADER_H = 36;
/** 内容区水平内边距（px-3 两侧）；justify 可用宽需扣除 */
const H_PADDING = 24;
/** 无尺寸资产的宽高比兜底（4:3） */
export const ASPECT_FALLBACK = 4 / 3;
/** 日期跳转预留的吸顶条高度（scrollToGroup 对齐补偿） */
export const STICKY_OFFSET = 44;
/** 网格缩略图名义边长（后端 snap 到 256 档就近） */
export const GRID_THUMB_SIZE = 240;
/** 长按进入多选的阈值 */
export const LONG_PRESS_MS = 500;

export type GridLayout = "square" | "justify";

/** 资产宽高比（width/height）；缺失/非法 → 4:3 兜底 */
function aspectOf(asset: AssetDto): number {
  const w = asset.width;
  const h = asset.height;
  if (typeof w === "number" && typeof h === "number" && w > 0 && h > 0) return w / h;
  return ASPECT_FALLBACK;
}

/** justify 切好的内容行（宽度取整，行高精确） */
interface JustifyRow {
  assets: AssetDto[];
  height: number;
  widths: number[];
}

/**
 * 经典贪心 justify：逐项累加，加入下一项后行高（可用宽/宽高比和）跌破目标即封行；
 * 组尾不足整行按目标行高左对齐（不拉伸）。导出供测试对齐断言。
 */
export function justifyItems(
  assets: AssetDto[],
  containerWidth: number,
  targetHeight: number,
  gap = GAP,
): JustifyRow[] {
  const rows: JustifyRow[] = [];
  let current: AssetDto[] = [];
  let aspectSum = 0;

  const rowHeightOf = (count: number, sum: number): number =>
    count > 0 ? (containerWidth - (count - 1) * gap) / sum : targetHeight;

  for (const asset of assets) {
    current.push(asset);
    aspectSum += aspectOf(asset);
    const height = rowHeightOf(current.length, aspectSum);
    if (height <= targetHeight) {
      // 封行：以当前行高精确分配各项宽度（取整，±几 px 由 flex 收缩吸收）
      rows.push({
        assets: current,
        height: Math.round(height),
        widths: current.map((a) => Math.round(aspectOf(a) * height)),
      });
      current = [];
      aspectSum = 0;
    }
  }
  // 组尾余量：目标行高左对齐（不足整行不拉伸）
  if (current.length > 0) {
    rows.push({
      assets: current,
      height: targetHeight,
      widths: current.map((a) => Math.round(aspectOf(a) * targetHeight)),
    });
  }
  return rows;
}

type GridRow =
  | { type: "header"; group: AssetGroup; collapsed: boolean }
  | { type: "tiles"; group: AssetGroup; assets: AssetDto[]; height: number; widths: number[] };

export interface AssetGridHandle {
  /** 滚动到指定组（组头对齐吸顶条下缘）；组不存在时静默 */
  scrollToGroup: (key: string) => void;
  /** 恢复到绝对滚动位置（画廊会话快照重挂载还原用） */
  restoreScroll: (top: number) => void;
  toggleGroup: (key: string) => void;
}

export interface ViewportInfo {
  scrollTop: number;
  /** 视口首行所属组（空网格为 null） */
  group: AssetGroup | null;
}

/** 多选支持（M4.5）：active 时点击=切换选中；selected 为选中序（角标序号=index+1） */
export interface GridSelection {
  active: boolean;
  selected: readonly number[];
  onToggle: (asset: AssetDto) => void;
}

interface AssetGridProps {
  groups: AssetGroup[];
  /** 点击资产块（打开查看器）；省略时块为纯展示 */
  onOpenAsset?: (asset: AssetDto, group: AssetGroup) => void;
  /** Ctrl/Cmd+点击（进入多选并选中该资产）；省略时不响应 */
  onCtrlClick?: (asset: AssetDto) => void;
  /** 长按瓦片 500ms（进入多选并选中该资产）；省略时不响应 */
  onLongPress?: (asset: AssetDto) => void;
  /** 瓦片左上角 check 圆钮点击（进入多选并选中该资产；多选态=切换选中）；省略不渲染 */
  onCheckClick?: (asset: AssetDto) => void;
  onFavoriteChange?: (asset: AssetDto, favorite: boolean) => void;
  /** 瓦片右键（自定义菜单；坐标为光标 client 坐标）；省略时不响应 */
  onAssetContextMenu?: (asset: AssetDto, at: { x: number; y: number }) => void;
  /** 多选状态 */
  selection?: GridSelection;
  /** 无限滚动哨兵节点（挂在本网格滚动容器内、全部内容之后） */
  sentinelRef?: Ref<HTMLDivElement>;
  /** 视口变化上报（吸顶组头/滚动状态用；仅组键或 scrollTop 显著变化时触发） */
  onViewportChange?: (info: ViewportInfo) => void;
  /** square=方格边长；justify=目标行高（三档切换见调用方） */
  tile?: number;
  /** 布局模式（默认 square 兼容旧视图；画廊用 justify） */
  layout?: GridLayout;
  /** 合并卡角标（RAW+JPG）：代表资产 id → 文案；无合并时不传 */
  badges?: Map<number, string>;
  /** 相似度角标（语义搜索）：assetId → 0..1，右下角百分比 */
  scores?: Map<number, number>;
  /** 连拍堆叠角标（M6）：封面 assetId → 连拍张数 N（含封面） */
  burstBadges?: Map<number, number>;
  /** 只读态（B1 回收站页）：隐藏收藏星钮等写操作入口，仅保留多选/浏览 */
  readOnly?: boolean;
  scrollTestId?: string;
}

const AssetGrid = forwardRef<AssetGridHandle, AssetGridProps>(function AssetGrid(
  {
    groups,
    onOpenAsset,
    onCtrlClick,
    onLongPress,
    onCheckClick,
    onFavoriteChange,
    onAssetContextMenu,
    selection,
    sentinelRef,
    onViewportChange,
    tile = TILE,
    layout = "square",
    badges,
    scores,
    burstBadges,
    readOnly = false,
    scrollTestId = "gallery-grid-scroll",
  },
  ref,
) {
  const { t } = useTranslation();
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(0);

  // --- 组头折叠（组件本地状态；切页/刷新不保留） -----------------------------------
  const [collapsedKeys, setCollapsedKeys] = useState<ReadonlySet<string>>(() => new Set());
  const [localFavorites, setLocalFavorites] = useState<ReadonlyMap<number, boolean>>(() => new Map());
  const toggleCollapsed = useCallback((key: string) => {
    setCollapsedKeys((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }, []);

  // 容器宽自适应（offsetWidth 读初值 + ResizeObserver 跟踪；jsdom 下 RO 不触发但
  // offsetWidth 已被测试 mock 为非零）
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const update = () => setWidth(el.offsetWidth);
    update();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // 键盘导航的近似列数：square=按方格宽；justify=按目标行高的 4:3 均值估
  const usableWidth = Math.max(0, width - H_PADDING);
  const columns = Math.max(
    1,
    Math.floor(
      layout === "justify"
        ? usableWidth / (tile * ASPECT_FALLBACK + GAP)
        : (width + GAP) / (tile + GAP),
    ),
  );

  // 行划分（useMemo 重算）：折叠组只保留头行；square 按列数切片等宽；justify 按宽高比贪心切行
  const rows = useMemo<GridRow[]>(() => {
    const out: GridRow[] = [];
    for (const group of groups) {
      const collapsed = collapsedKeys.has(group.key);
      out.push({ type: "header", group, collapsed });
      if (collapsed || group.assets.length === 0) continue;
      if (layout === "justify") {
        for (const row of justifyItems(group.assets, usableWidth, tile)) {
          out.push({ type: "tiles", group, ...row });
        }
      } else {
        for (let i = 0; i < group.assets.length; i += columns) {
          const assets = group.assets.slice(i, i + columns);
          out.push({ type: "tiles", group, assets, height: tile, widths: assets.map(() => tile) });
        }
      }
    }
    return out;
  }, [groups, columns, layout, usableWidth, tile, collapsedKeys]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    getItemKey: (i) => rows[i].type === "header"
      ? `header:${rows[i].group.key}`
      : `tiles:${rows[i].group.key}:${rows[i].assets.map((asset) => asset.id).join(",")}`,
    estimateSize: (i) =>
      rows[i].type === "header"
        ? rows[i].collapsed
          ? COLLAPSED_HEADER_H
          : HEADER_H
        : rows[i].height + GAP,
    overscan: 6,
  });

  // 搜索替换结果、图片尺寸补齐、切换大小都会改变行高；旧测量不能按
  // 数字下标复用到新行。绘制前清缓存，避免标题/照片相互覆盖。
  useLayoutEffect(() => {
    virtualizer.measure();
  }, [rows, virtualizer]);

  // --- 键盘导航（#8）：网格聚焦后 ←→↑↓ 移动高亮（outline accent），Enter 打开查看器 ---
  const [cursor, setCursor] = useState<number | null>(null);
  const flatIndexById = useMemo(() => {
    const map = new Map<number, number>();
    let i = 0;
    for (const group of groups) {
      for (const asset of group.assets) {
        map.set(asset.id, i);
        i += 1;
      }
    }
    return map;
  }, [groups]);
  const flatAssets = useMemo(() => groups.flatMap((g) => g.assets), [groups]);

  const moveCursor = useCallback(
    (delta: number) => {
      setCursor((prev) => {
        const base = prev ?? 0;
        return Math.min(Math.max(base + delta, 0), flatAssets.length - 1);
      });
    },
    [flatAssets.length],
  );

  function handleGridKeyDown(e: React.KeyboardEvent<HTMLDivElement>): void {
    if (e.key === "ArrowRight") moveCursor(1);
    else if (e.key === "ArrowLeft") moveCursor(-1);
    else if (e.key === "ArrowDown") moveCursor(columns);
    else if (e.key === "ArrowUp") moveCursor(-columns);
    else if (e.key === "Home") setCursor(0);
    else if (e.key === "End") setCursor(flatAssets.length - 1);
    else if (e.key === "Enter") {
      if (cursor === null) return;
      const asset = flatAssets[cursor];
      if (asset && onOpenAsset) {
        if (selection?.active) {
          selection.onToggle(asset);
          return;
        }
        const owner = groups.find((g) => g.assets.includes(asset));
        if (owner) onOpenAsset(asset, owner);
      }
    } else return;
    e.preventDefault();
  }

  // 高亮格滚入可视区（jsdom 无 scrollIntoView，静默跳过）
  useEffect(() => {
    if (cursor === null) return;
    const el = scrollRef.current?.querySelector('[data-cursor="true"]');
    if (el && typeof el.scrollIntoView === "function") {
      el.scrollIntoView({ block: "nearest" });
    }
  }, [cursor]);

  // 视口上报：首虚拟行所属组 + scrollTop（显著变化才上报，避免每次渲染触发上层 setState）
  const lastReport = useRef<{ key: string; top: number }>({ key: "", top: -1 });
  useEffect(() => {
    const first = virtualizer.getVirtualItems()[0];
    const row = first ? rows[first.index] : undefined;
    const group = row ? row.group : null;
    const top = scrollRef.current?.scrollTop ?? 0;
    const key = group?.key ?? "";
    if (key === lastReport.current.key && Math.abs(top - lastReport.current.top) <= 4) return;
    lastReport.current = { key, top };
    onViewportChange?.({ scrollTop: top, group });
  });

  useImperativeHandle(
    ref,
    () => ({
      scrollToGroup: (key: string) => {
        const index = rows.findIndex((r) => r.type === "header" && r.group.key === key);
        const offset = virtualizer.getOffsetForIndex(index, "start");
        const el = scrollRef.current;
        if (index < 0 || !el || !offset) return;
        // 直接赋值 scrollTop（元素 scrollTo 在 jsdom 缺失；真机等价滚动）。
        // 赋值后补发 scroll 事件：真机浏览器赋值本就会异步派发（此处幂等），
        // jsdom/测试环境下赋值不派发——补发让虚拟化器立即按新偏移重算可视行。
        el.scrollTop = Math.max(0, offset[0] - STICKY_OFFSET);
        el.dispatchEvent(new Event("scroll"));
      },
      restoreScroll: (top: number) => {
        const el = scrollRef.current;
        if (!el) return;
        // 与 scrollToGroup 同语义：直接赋值 + 补发 scroll 事件（jsdom 兼容）
        el.scrollTop = Math.max(0, top);
        el.dispatchEvent(new Event("scroll"));
      },
      toggleGroup: toggleCollapsed,
    }),
    [rows, virtualizer, toggleCollapsed],
  );

  // --- 长按（进入多选）：pointerdown 起 500ms 计时，抬起/离开/滚动取消 ----------------
  const longPressTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const longPressFired = useRef(false);
  const dragSelection = useRef<{ intent: "select" | "deselect"; visited: Set<number> } | null>(null);
  const suppressTileClick = useRef(false);
  const selectedIds = useRef(new Set(selection?.selected ?? []));
  selectedIds.current = new Set(selection?.selected ?? []);
  const clearLongPress = useCallback(() => {
    if (longPressTimer.current !== null) {
      clearTimeout(longPressTimer.current);
      longPressTimer.current = null;
    }
  }, []);
  useEffect(() => clearLongPress, [clearLongPress]);

  useEffect(() => {
    const finishDrag = () => {
      dragSelection.current = null;
      // click follows pointerup synchronously. Keep suppression through that click, then
      // release it so a pointerup outside the tile cannot swallow the next interaction.
      window.setTimeout(() => {
        suppressTileClick.current = false;
      }, 0);
    };
    window.addEventListener("pointerup", finishDrag);
    window.addEventListener("pointercancel", finishDrag);
    return () => {
      window.removeEventListener("pointerup", finishDrag);
      window.removeEventListener("pointercancel", finishDrag);
    };
  }, []);

  function applyDragSelection(asset: AssetDto, intent: "select" | "deselect"): void {
    const selected = selectedIds.current.has(asset.id);
    if ((intent === "select") === selected) return;
    if (intent === "select") selectedIds.current.add(asset.id);
    else selectedIds.current.delete(asset.id);
    selection?.onToggle(asset);
  }

  function handleTilePointerDown(asset: AssetDto, event: React.PointerEvent<HTMLButtonElement>): void {
    if (selection?.active && event.button === 0 && event.pointerType === "mouse") {
      const intent = selectedIds.current.has(asset.id) ? "deselect" : "select";
      dragSelection.current = { intent, visited: new Set([asset.id]) };
      suppressTileClick.current = true;
      applyDragSelection(asset, intent);
      return;
    }
    if (selection?.active || !onLongPress) return; // 已在多选或调用方不支持
    longPressFired.current = false;
    clearLongPress();
    longPressTimer.current = setTimeout(() => {
      longPressTimer.current = null;
      longPressFired.current = true;
      onLongPress(asset);
    }, LONG_PRESS_MS);
  }

  function handleTilePointerEnter(asset: AssetDto, event: React.PointerEvent<HTMLButtonElement>): void {
    const drag = dragSelection.current;
    if (!drag || event.pointerType !== "mouse" || drag.visited.has(asset.id)) return;
    drag.visited.add(asset.id);
    applyDragSelection(asset, drag.intent);
  }

  function handleTileClick(asset: AssetDto, group: AssetGroup, ctrl: boolean): void {
    if (longPressFired.current) {
      // 长按刚触发：吞掉本次 click（多选已切换）
      longPressFired.current = false;
      return;
    }
    if (selection?.active) {
      if (suppressTileClick.current) return;
      selection.onToggle(asset);
      return;
    }
    if (ctrl && onCtrlClick) {
      onCtrlClick(asset);
      return;
    }
    onOpenAsset?.(asset, group);
  }

  return (
    <div
      ref={scrollRef}
      tabIndex={0}
      onKeyDown={handleGridKeyDown}
      className="sp-scroll h-full overflow-y-auto outline-none"
      data-testid={scrollTestId}
      data-layout={layout}
    >
      <div className="px-3 pb-6" style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
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
                <div
                  // 组头=折叠开关（M4.5）：点击折叠为一行 36px 头，再点展开
                  role="button"
                  tabIndex={0}
                  aria-expanded={!row.collapsed}
                  onClick={() => toggleCollapsed(row.group.key)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      toggleCollapsed(row.group.key);
                    }
                  }}
                  className={`flex cursor-pointer select-none items-center gap-2 ${
                    row.collapsed ? "h-[36px]" : "h-[40px]"
                  }`}
                  data-testid={row.group.key === UNKNOWN_GROUP_KEY ? "gallery-group-unknown" : "gallery-group"}
                  data-group-key={row.group.key}
                  data-count={row.group.assets.length}
                  data-collapsed={row.collapsed}
                >
                  {/* 折叠 chevron：展开朝下 / 折叠朝右 */}
                  <svg
                    viewBox="0 0 16 16"
                    width="10"
                    height="10"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="1.8"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    className={`shrink-0 text-text-muted transition-transform duration-150 ${
                      row.collapsed ? "-rotate-90" : ""
                    }`}
                    aria-hidden="true"
                    data-testid="gallery-group-collapse"
                  >
                    <path d="M3.5 6l4.5 4.5L12.5 6" />
                  </svg>
                  <h2 className="truncate text-[13px] font-semibold text-text-primary">
                    {row.group.date === null
                      ? t("gallery.unknownDate")
                      : formatDateLabel(row.group.date)}
                  </h2>
                  <span className="text-xs text-text-muted">
                    {t("gallery.groupCount", {
                      count: row.group.totalCount ?? row.group.assets.length,
                    })}
                  </span>
                </div>
              ) : (
                <div
                  className="flex flex-nowrap gap-1 pb-1"
                  style={{ height: row.height }}
                  data-testid="gallery-row"
                >
                  {row.assets.map((asset, itemIndex) => {
                    const badge = badges?.get(asset.id) ?? null;
                    const score = scores?.get(asset.id);
                    const isCursor = cursor !== null && flatIndexById.get(asset.id) === cursor;
                    const isSelected =
                      selection?.active && selection.selected.includes(asset.id);
                    const isFavorite = localFavorites.get(asset.id) ?? (asset.rating === 5);
                    // 颜色标签（LR 五色标）：瓦片左下角小圆点（有 colorLabel 才显示）
                    const colorDot = asColorLabel(asset.colorLabel);
                    // 拒绝旗标：瓦片弱化（整体降不透明度）+ 右上红旗角标
                    const isRejected = asset.rejected === true;
                    const inner = (
                      <>
                        <AssetThumb asset={asset} size={GRID_THUMB_SIZE} className="h-[calc(100%-20px)] w-full" />
                        <span className="absolute inset-x-0 bottom-0 flex h-5 items-center justify-center bg-panel/90 font-mono text-[10px] tabular-nums text-text-secondary" data-testid="tile-resolution">
                          {asset.width && asset.height ? `${asset.width} × ${asset.height}` : "—"}
                        </span>
                        {colorDot && (
                          <span
                            className={`absolute bottom-1.5 left-1.5 z-20 h-2 w-2 rounded-full ${COLOR_DOT_CLASS[colorDot]} ${COLOR_DOT_RING}`}
                            data-testid="tile-color-dot"
                            data-label={colorDot}
                            title={t(`gallery.color.${colorDot}`)}
                          />
                        )}
                        {isRejected && (
                          <span
                            className="absolute right-1 top-7 z-10 flex h-4 w-4 items-center justify-center rounded-sm bg-red-500/90 text-white"
                            data-testid="tile-reject-badge"
                            title={t("gallery.rejectedBadge")}
                          >
                            <svg viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                              <path d="M3.5 14V2.5M3.5 3h7l-1 2.5 1 2.5h-7" />
                            </svg>
                          </span>
                        )}
                        {badge && (
                          <span
                            className="absolute right-1 top-1 rounded bg-black/60 px-1 py-0.5 font-mono text-[10px] font-bold leading-none text-white"
                            data-testid="gallery-pair-badge"
                          >
                            {badge}
                          </span>
                        )}
                        {score !== undefined && (
                          <span
                            className={`absolute bottom-6 left-1 rounded px-1 py-0.5 font-mono text-[10px] leading-none ${
                              SIMILARITY_BADGE_CLASS[similarityTier(score)]
                            }`}
                            data-testid="search-score-badge"
                            data-score-tier={similarityTier(score)}
                          >
                            {Math.round(score * 100)}%
                          </span>
                        )}
                        {/* 多选 check 圆钮（Google Photos 式，替代序号角标）：默认
                            hover 显示，点击进入多选并选中该张；多选态常显，
                            选中=实心勾。点击/右键都 stopPropagation，不触发瓦片。 */}
                        {onCheckClick && (
                          <span
                            role="checkbox"
                            aria-checked={isSelected}
                            aria-label={asset.name}
                            onClick={(e) => {
                              e.stopPropagation();
                              onCheckClick(asset);
                            }}
                            onPointerDown={(e) => e.stopPropagation()}
                            onContextMenu={(e) => e.stopPropagation()}
                            className={`absolute left-1.5 top-1.5 z-20 flex h-5 w-5 items-center justify-center rounded-full border transition-opacity ${
                              isSelected
                                ? "border-accent bg-accent text-black opacity-100"
                                : "border-white/70 bg-black/45 text-white"
                            } ${
                              selection?.active ? "opacity-100" : "opacity-0 group-hover:opacity-100"
                            }`}
                            data-testid="tile-check"
                            data-asset-id={asset.id}
                            data-selected={isSelected ? "true" : "false"}
                          >
                            {isSelected && (
                              <svg
                                viewBox="0 0 16 16"
                                width="11"
                                height="11"
                                fill="none"
                                stroke="currentColor"
                                strokeWidth="2.2"
                                strokeLinecap="round"
                                strokeLinejoin="round"
                                aria-hidden="true"
                              >
                                <path d="M3.5 8.5l3 3 6-6.5" />
                              </svg>
                            )}
                          </span>
                        )}
                        {!readOnly && (
                          <span
                            role="button"
                            tabIndex={0}
                            aria-label={isFavorite ? t("gallery.favoriteRemove") : t("gallery.favoriteAdd")}
                            aria-pressed={isFavorite}
                            onPointerDown={(e) => e.stopPropagation()}
                            onClick={(e) => {
                              e.stopPropagation();
                              const favorite = !isFavorite;
                              setLocalFavorites((current) => new Map(current).set(asset.id, favorite));
                              void assetRatingSet(asset.id, favorite ? 5 : 0);
                              onFavoriteChange?.(asset, favorite);
                            }}
                            onKeyDown={(e) => {
                              if (e.key === "Enter" || e.key === " ") {
                                e.preventDefault();
                                e.stopPropagation();
                                e.currentTarget.click();
                              }
                            }}
                            className={`absolute bottom-6 right-1.5 z-20 flex h-5 w-5 cursor-pointer items-center justify-center rounded-full bg-black/45 transition-colors ${
                              isFavorite ? "text-amber-400" : "text-white/75 hover:text-amber-300"
                            }`}
                            data-testid="tile-favorite"
                            data-asset-id={asset.id}
                          >
                            <svg viewBox="0 0 24 24" width="17" height="17" fill={isFavorite ? "currentColor" : "none"} stroke="currentColor" strokeWidth="1.8" aria-hidden="true"><path d="m12 2 3.1 6.3 7 .9-5 4.9 1.2 7-6.3-3.3-6.3 3.3 1.2-7-5-4.9 7-.9z" /></svg>
                          </span>
                        )}
                      </>
                    );
                    const itemWidth = row.widths[itemIndex];
                    const burstCount = burstBadges?.get(asset.id);
                    const selectionClass = isSelected
                      ? "outline outline-2 -outline-offset-2 outline-accent"
                      : selection?.active
                        ? "outline outline-1 -outline-offset-2 outline-transparent hover:outline-edge"
                        : "";
                    return onOpenAsset || selection?.active || onCtrlClick || onLongPress || onCheckClick || onAssetContextMenu ? (
                      <button
                        key={asset.id}
                        type="button"
                        onClick={(e) => handleTileClick(asset, row.group, e.ctrlKey || e.metaKey)}
                        onPointerDown={(e) => handleTilePointerDown(asset, e)}
                        onPointerEnter={(e) => handleTilePointerEnter(asset, e)}
                        onPointerUp={clearLongPress}
                        onPointerLeave={clearLongPress}
                        onDragStart={(e) => e.preventDefault()}
                        onContextMenu={(e) => {
                          // 右键自定义菜单（原生菜单已被全局 guard 屏蔽，此处兜底）
                          e.preventDefault();
                          e.stopPropagation();
                          onAssetContextMenu?.(asset, { x: e.clientX, y: e.clientY });
                        }}
                        className={`group relative isolate touch-none overflow-hidden rounded-md bg-panel/40 outline-none transition-[transform,outline-color] duration-100 focus-visible:outline-2 focus-visible:outline-accent ${
                          isCursor ? "outline outline-2 -outline-offset-2 outline-accent" : ""
                        } ${selectionClass} ${isRejected ? "opacity-50" : ""}`}
                        style={{ width: itemWidth, height: row.height }}
                        title={asset.name}
                        data-testid="gallery-tile"
                        data-asset-id={asset.id}
                        data-kind={asset.kind}
                        data-cursor={isCursor}
                        data-selected={isSelected}
                        data-rejected={isRejected || undefined}
                        data-burst={burstCount !== undefined ? burstCount : undefined}
                      >
                        {/* 连拍堆叠底片层（纯 CSS 偏移，不动画；绘制在封面之下） */}
                        {burstCount !== undefined && (
                          <>
                            <span
                              className="absolute inset-0 translate-x-[5px] translate-y-[5px] rounded-md border border-edge/50 bg-panel/50"
                              aria-hidden="true"
                              data-testid="gallery-burst-layer"
                            />
                            <span
                              className="absolute inset-0 translate-x-[10px] translate-y-[10px] rounded-md border border-edge/30 bg-panel/30"
                              aria-hidden="true"
                            />
                          </>
                        )}
                        {inner}
                        {burstCount !== undefined && (
                          <span
                            className="absolute bottom-6 left-1 z-10 rounded bg-black/70 px-1.5 py-0.5 font-mono text-[10px] font-bold leading-none text-white"
                            data-testid="gallery-burst-badge"
                          >
                            {t("gallery.burstBadge", { count: burstCount })}
                          </span>
                        )}
                      </button>
                    ) : (
                      <div
                        key={asset.id}
                        className={`relative overflow-hidden rounded-md bg-panel/40 ${
                          isCursor ? "outline outline-2 -outline-offset-2 outline-accent" : ""
                        } ${selectionClass} ${isRejected ? "opacity-50" : ""}`}
                        style={{ width: itemWidth, height: row.height }}
                        title={asset.name}
                        data-testid="gallery-tile"
                        data-asset-id={asset.id}
                        data-kind={asset.kind}
                        data-cursor={isCursor}
                        data-selected={isSelected}
                        data-rejected={isRejected || undefined}
                        data-burst={burstCount !== undefined ? burstCount : undefined}
                      >
                        {/* 连拍堆叠底片层（纯 CSS 偏移，不动画；绘制在封面之下） */}
                        {burstCount !== undefined && (
                          <>
                            <span
                              className="absolute inset-0 translate-x-[5px] translate-y-[5px] rounded-md border border-edge/50 bg-panel/50"
                              aria-hidden="true"
                              data-testid="gallery-burst-layer"
                            />
                            <span
                              className="absolute inset-0 translate-x-[10px] translate-y-[10px] rounded-md border border-edge/30 bg-panel/30"
                              aria-hidden="true"
                            />
                          </>
                        )}
                        {inner}
                        {burstCount !== undefined && (
                          <span
                          className="absolute bottom-6 left-1 z-10 rounded bg-black/70 px-1.5 py-0.5 font-mono text-[10px] font-bold leading-none text-white"
                            data-testid="gallery-burst-badge"
                          >
                            {t("gallery.burstBadge", { count: burstCount })}
                          </span>
                        )}
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          );
        })}
      </div>
      {/* 无限滚动哨兵：进入视口（rootMargin 提前 800px）触发下一页 */}
      <div ref={sentinelRef} data-testid="gallery-sentinel" className="h-px" />
    </div>
  );
});

export default AssetGrid;
