import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
  type Ref,
} from "react";
import { useTranslation } from "react-i18next";
import { useVirtualizer } from "@tanstack/react-virtual";

import type { AssetDto } from "@/ipc/api";
import { SIMILARITY_BADGE_CLASS, similarityTier } from "@/features/ai/scoreBadge";
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
 * - justify（画廊新默认）：统一行高的 justify 网格——行内按 width/height 宽高比
 *   分配宽度（经典贪心算法：逐项累加，行高跌破目标即封行；组尾行不足整行按
 *   目标行高左对齐），无尺寸资产按 4:3 兜底；tile 语义变为「目标行高」。
 *
 * 虚拟化：TanStack Virtual 行模型（组头行 40px + 内容行自带高度），行划分与
 * 行高在 useMemo 重算（justify 行高逐行精确，estimateSize 即实高）。
 * 吸顶组头由上层以覆盖条实现；本组件通过 onViewportChange 上报
 * 「视口首行所属组 + scrollTop」。
 */

/** 默认方格边长（中档；三档切换见 useGalleryTileSize） */
const TILE = 200;
const GAP = 4;
const HEADER_H = 40;
/** 内容区水平内边距（px-3 两侧）；justify 可用宽需扣除 */
const H_PADDING = 24;
/** 无尺寸资产的宽高比兜底（4:3） */
export const ASPECT_FALLBACK = 4 / 3;
/** 日期跳转预留的吸顶条高度（scrollToGroup 对齐补偿） */
export const STICKY_OFFSET = 44;
/** 网格缩略图名义边长（后端 snap 到 256 档就近） */
export const GRID_THUMB_SIZE = 240;

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
  | { type: "header"; group: AssetGroup }
  | { type: "tiles"; group: AssetGroup; assets: AssetDto[]; height: number; widths: number[] };

export interface AssetGridHandle {
  /** 滚动到指定组（组头对齐吸顶条下缘）；组不存在时静默 */
  scrollToGroup: (key: string) => void;
  /** 恢复到绝对滚动位置（画廊会话快照重挂载还原用） */
  restoreScroll: (top: number) => void;
}

export interface ViewportInfo {
  scrollTop: number;
  /** 视口首行所属组（空网格为 null） */
  group: AssetGroup | null;
}

interface AssetGridProps {
  groups: AssetGroup[];
  /** 点击资产块（打开查看器）；省略时块为纯展示 */
  onOpenAsset?: (asset: AssetDto, group: AssetGroup) => void;
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
  scrollTestId?: string;
}

const AssetGrid = forwardRef<AssetGridHandle, AssetGridProps>(function AssetGrid(
  {
    groups,
    onOpenAsset,
    sentinelRef,
    onViewportChange,
    tile = TILE,
    layout = "square",
    badges,
    scores,
    scrollTestId = "gallery-grid-scroll",
  },
  ref,
) {
  const { t } = useTranslation();
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(0);

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

  // 行划分（useMemo 重算）：square 按列数切片等宽；justify 按宽高比贪心切行
  const rows = useMemo<GridRow[]>(() => {
    const out: GridRow[] = [];
    for (const group of groups) {
      out.push({ type: "header", group });
      if (layout === "justify") {
        if (group.assets.length === 0) continue;
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
  }, [groups, columns, layout, usableWidth, tile]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => (rows[i].type === "header" ? HEADER_H : rows[i].height + GAP),
    overscan: 6,
  });

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
    }),
    [rows, virtualizer],
  );

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
                  className="flex h-[40px] items-center gap-2"
                  data-testid={row.group.key === UNKNOWN_GROUP_KEY ? "gallery-group-unknown" : "gallery-group"}
                  data-group-key={row.group.key}
                  data-count={row.group.assets.length}
                >
                  <h2 className="text-[13px] font-semibold text-text-primary">
                    {row.group.date === null
                      ? t("gallery.unknownDate")
                      : formatDateLabel(row.group.date)}
                  </h2>
                  <span className="text-xs text-text-muted">
                    {t("gallery.groupCount", { count: row.group.assets.length })}
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
                    const inner = (
                      <>
                        <AssetThumb asset={asset} size={GRID_THUMB_SIZE} className="h-full w-full" />
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
                            className={`absolute bottom-1 right-1 rounded px-1 py-0.5 font-mono text-[10px] leading-none ${
                              SIMILARITY_BADGE_CLASS[similarityTier(score)]
                            }`}
                            data-testid="search-score-badge"
                            data-score-tier={similarityTier(score)}
                          >
                            {Math.round(score * 100)}%
                          </span>
                        )}
                      </>
                    );
                    const itemWidth = row.widths[itemIndex];
                    return onOpenAsset ? (
                      <button
                        key={asset.id}
                        type="button"
                        onClick={() => onOpenAsset(asset, row.group)}
                        className={`relative overflow-hidden rounded-md bg-panel/40 outline-none transition-[transform,outline-color] duration-100 focus-visible:outline-2 focus-visible:outline-accent hover:outline hover:outline-1 hover:outline-edge ${
                          isCursor ? "outline outline-2 -outline-offset-2 outline-accent" : ""
                        }`}
                        style={{ width: itemWidth, height: row.height }}
                        title={asset.name}
                        data-testid="gallery-tile"
                        data-asset-id={asset.id}
                        data-kind={asset.kind}
                        data-cursor={isCursor}
                      >
                        {inner}
                      </button>
                    ) : (
                      <div
                        key={asset.id}
                        className={`relative overflow-hidden rounded-md bg-panel/40 ${
                          isCursor ? "outline outline-2 -outline-offset-2 outline-accent" : ""
                        }`}
                        style={{ width: itemWidth, height: row.height }}
                        title={asset.name}
                        data-testid="gallery-tile"
                        data-asset-id={asset.id}
                        data-kind={asset.kind}
                        data-cursor={isCursor}
                      >
                        {inner}
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
