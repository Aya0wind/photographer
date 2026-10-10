import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Navigate, useNavigate, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { motion } from "motion/react";

import {
  assetsCount,
  assetsPage,
  type AssetDto,
  type AssetFilters,
} from "@/ipc/api";
import { mapRegionTree } from "@/ipc/api/map";
import { useExportRefresh } from "@/features/editor/lib/useExportRefresh";
import { usePageSentinel } from "@/features/gallery/lib/usePageSentinel";
import { usePhotoTimeline } from "@/features/gallery/lib/usePhotoTimeline";
import { useAssetSelection } from "@/features/gallery/lib/useAssetSelection";
import { usePhotoCards } from "@/features/gallery/lib/usePhotoCards";
import { groupAssetsByDate } from "@/features/gallery/lib/assetGroups";
import { useAssetViewer } from "@/features/gallery/lib/useAssetViewer";
import AssetGrid from "@/features/gallery/components/AssetGrid";
import TileSizeSwitch from "@/features/gallery/components/TileSizeSwitch";
import ViewerOverlay from "@/features/gallery/components/ViewerOverlay";
import AssetActionsHost from "@/features/gallery/components/AssetActionsHost";
import { GALLERY_JUSTIFY_ROW_PX, useGalleryTileSize } from "@/features/gallery/lib/useGalleryTileSize";
import { TRANS, motionInitial, useMotionOn } from "@/lib/motion";
import { regionPathOf } from "../lib/regionPath";

/**
 * 拍摄地图联动照片子页（/map/photos?region=<regionId>，MapPage 嵌套子路由）：
 * - 数据源 assets_page（keyset 游标分页，filters={regionId} 含子树），无限
 *   滚动哨兵补页；计数 assets_count({regionId})；日期目录 asset_group_dates
 * - 结构照抄 AlbumDetailPage 网格页范式：AssetGrid justify 布局 + 日期分组 +
 *   时间线条 + 全屏查看器 + 多选（右键/批量操作走 AssetActionsHost）
 * - 头部：返回地图 + 面包屑标题（mapRegionTree 上溯「中国 / 浙江省 / 杭州市」，
 *   缓存未就绪回退只显 #id）+ 资产计数
 * - 空态防御兜底（气泡保证有照片才可进入）；非法 region 参数回 /map
 */

const PAGE_LIMIT = 100;

export default function MapPhotosPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const motionOn = useMotionOn();
  const [searchParams] = useSearchParams();
  const regionId = Number(searchParams.get("region"));
  const valid = Number.isInteger(regionId) && regionId > 0;

  // --- 面包屑标题（地区树上溯完整路径；缓存未就绪回退只显 #id） -----------------------
  const [path, setPath] = useState<string | null>(null);
  useEffect(() => {
    if (!valid) return;
    let cancelled = false;
    setPath(null);
    void mapRegionTree().then((rows) => {
      if (cancelled) return;
      setPath(rows !== null ? regionPathOf(rows, regionId) : null);
    });
    return () => {
      cancelled = true;
    };
  }, [valid, regionId]);
  const title = path ?? `#${regionId}`;

  // --- 资产管线（keyset 分页 + 无限滚动哨兵） ------------------------------------------
  const filters = useMemo<AssetFilters>(() => ({ regionId }), [regionId]);
  const [assets, setAssets] = useState<AssetDto[]>([]);
  const [status, setStatus] = useState<"loading" | "ready">("loading");
  const [loadingMore, setLoadingMore] = useState(false);
  const [totalCount, setTotalCount] = useState<number | null>(null);
  const assetsRef = useRef<AssetDto[]>([]);
  const hasMoreRef = useRef(true);
  const loadingRef = useRef(false);
  const loadSeqRef = useRef(0);
  const loadedOnce = useRef(false);
  const [reloadNonce, setReloadNonce] = useState(0);
  useExportRefresh(() => setReloadNonce((n) => n + 1));

  const fetchPage = useCallback(
    (afterId: number) => assetsPage(afterId, PAGE_LIMIT, filters),
    [filters],
  );

  const appendPage = useCallback(async (): Promise<AssetDto[]> => {
    if (loadingRef.current || !hasMoreRef.current) return [];
    loadingRef.current = true;
    setLoadingMore(true);
    const seq = loadSeqRef.current;
    const afterId =
      assetsRef.current.length > 0 ? assetsRef.current[assetsRef.current.length - 1].id : 0;
    const page = await fetchPage(afterId);
    // 补页不参与主加载的 seq 竞争（同 AlbumDetailPage）：await 期间主加载重置
    // （loadingRef 已被清零）则本页丢弃，由新管线接管
    if (loadingRef.current && seq === loadSeqRef.current) {
      if (page.length > 0) {
        const seen = new Set(assetsRef.current.map((a) => a.id));
        const fresh = page.filter((a) => !seen.has(a.id));
        assetsRef.current = [...assetsRef.current, ...fresh];
        setAssets(assetsRef.current);
      }
      if (page.length < PAGE_LIMIT) hasMoreRef.current = false;
      loadingRef.current = false;
      setLoadingMore(false);
    }
    return page;
  }, [fetchPage]);

  // 首屏 / 换地区：整页重置重拉
  useEffect(() => {
    if (!valid) return;
    let cancelled = false;
    const seq = ++loadSeqRef.current;
    setStatus("loading");
    hasMoreRef.current = true;
    loadingRef.current = true;
    void fetchPage(0).then((page) => {
      if (cancelled || seq !== loadSeqRef.current) return;
      assetsRef.current = page;
      setAssets(page);
      if (page.length < PAGE_LIMIT) hasMoreRef.current = false;
      loadingRef.current = false;
      setLoadingMore(false);
      loadedOnce.current = true;
      setStatus("ready");
    });
    return () => {
      cancelled = true;
    };
  }, [valid, regionId, fetchPage, reloadNonce]);

  // 资产计数（头部徽标；独立于已加载分页）
  useEffect(() => {
    if (!valid) return;
    let cancelled = false;
    setTotalCount(null);
    void assetsCount(filters).then((count) => {
      if (!cancelled) setTotalCount(count);
    });
    return () => {
      cancelled = true;
    };
  }, [valid, filters, reloadNonce]);

  const sentinelRef = usePageSentinel(valid && status === "ready", assets.length, appendPage);

  // --- 日期目录时间线（asset_group_dates / assets_seek 同吃 regionId） ------------------
  const timelineCurrentAssets = useCallback(() => assetsRef.current, []);
  const beforeTimelineJump = useCallback(() => {
    ++loadSeqRef.current;
    loadingRef.current = true;
    setLoadingMore(false);
  }, []);
  const finishTimelineJump = useCallback(() => {
    loadingRef.current = false;
  }, []);
  const replaceTimelineAssets = useCallback((page: AssetDto[]) => {
    assetsRef.current = page;
    setAssets(page);
    hasMoreRef.current = page.length === PAGE_LIMIT;
    setStatus("ready");
  }, []);
  const prependTimelineAssets = useCallback((page: AssetDto[]) => {
    const seen = new Set(assetsRef.current.map((asset) => asset.id));
    assetsRef.current = [...page.filter((asset) => !seen.has(asset.id)), ...assetsRef.current];
    setAssets(assetsRef.current);
  }, []);
  const timeline = usePhotoTimeline({
    enabled: valid && status === "ready",
    scopeKey: `${regionId}:${reloadNonce}`,
    filters,
    currentAssets: timelineCurrentAssets,
    onBeforeJump: beforeTimelineJump,
    onJumpFinished: finishTimelineJump,
    onReplace: replaceTimelineAssets,
    onPrepend: prependTimelineAssets,
  });

  // --- 多选 + 查看器（RecentPage 同款：AssetActionsHost 接齐右键/批量弹窗） -------------
  const { selecting, selected, setSelected, toggleSelected, ctrlSelect, exitSelection, contextTargets } = useAssetSelection();
  const { cards, badges } = usePhotoCards(assets);
  const groups = useMemo(() => groupAssetsByDate(cards), [cards]);
  const { viewer, openAsset, closeViewer, navigateTo, selectVersion } = useAssetViewer(groups, assets);
  // 预览靠近已加载末尾时提前补页（跨日期连续翻页越过分页边界）
  useEffect(() => {
    if (!viewer || !hasMoreRef.current) return;
    if (viewer.index >= viewer.group.assets.length - 8) void appendPage();
  }, [viewer?.index, viewer?.group.assets.length, appendPage]);
  const [tileSize, setTileSize] = useGalleryTileSize();
  const assetsById = useMemo(() => new Map(assets.map((a) => [a.id, a])), [assets]);

  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number; assets: AssetDto[] } | null>(null);
  /** 批量乐观补丁（星级/色标/拒绝/查看器回传共用） */
  const patchAssets = useCallback((ids: number[], patch: Partial<AssetDto>) => {
    if (ids.length === 0) return;
    const idSet = new Set(ids);
    assetsRef.current = assetsRef.current.map((a) => (idSet.has(a.id) ? { ...a, ...patch } : a));
    setAssets(assetsRef.current);
  }, []);
  /** 移入回收站后本地剔除（计数同步扣减） */
  const removeAssets = useCallback((ids: number[]) => {
    const idSet = new Set(ids);
    assetsRef.current = assetsRef.current.filter((a) => !idSet.has(a.id));
    setAssets(assetsRef.current);
    setTotalCount((count) => (count === null ? null : Math.max(0, count - ids.length)));
  }, []);

  function handleTileContextMenu(asset: AssetDto, at: { x: number; y: number }): void {
    setCtxMenu({ ...at, assets: contextTargets(asset, assetsById) });
  }

  // 非法 region 参数（缺省/非正整数）：回地图（理论不会出现，防御兜底）
  if (!valid) return <Navigate to="/map" replace />;

  return (
    <motion.div
      className="h-full"
      initial={motionInitial(motionOn, { y: 8 })}
      animate={{ y: 0 }}
      transition={TRANS.slide}
      data-testid="mapphotos-page"
      data-region-id={regionId}
    >
      <div className="flex h-full w-full flex-col px-4">
        {/* 头部：返回地图 + 面包屑标题 + 计数 + 三档尺寸 */}
        <div className="flex h-14 shrink-0 items-center gap-2.5" data-testid="mapphotos-toolbar">
          <button
            type="button"
            onClick={() => navigate("/map")}
            aria-label={t("mapphotos.backToMap")}
            className="flex h-7 shrink-0 items-center gap-1 rounded-md border border-edge px-2 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
            data-testid="mapphotos-back"
          >
            <svg viewBox="0 0 16 16" width="9" height="9" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M10 3L5 8l5 5" />
            </svg>
            {t("mapphotos.backToMap")}
          </button>
          <h1 className="min-w-0 truncate text-sm font-semibold text-text-primary" title={title} data-testid="mapphotos-title">
            {title}
          </h1>
          <span
            className="shrink-0 rounded-full bg-panel px-2 py-0.5 font-mono text-[11px] tabular-nums text-text-secondary"
            data-testid="mapphotos-count"
          >
            {t("gallery.groupCount", { count: totalCount ?? assets.length })}
          </span>
          <div className="ml-auto shrink-0">
            <TileSizeSwitch value={tileSize} onChange={setTileSize} />
          </div>
        </div>

        {/* 照片墙（与图库/相册详情同款 justify 网格）/ 加载 / 空态 */}
        <div className="relative min-h-0 flex-1">
          {status === "loading" ? (
            <div
              className="flex h-full items-center justify-center text-xs text-text-muted"
              data-testid="mapphotos-loading"
            >
              {t("recent.loading")}
            </div>
          ) : assets.length === 0 ? (
            <div
              className="flex h-full flex-col items-center justify-center gap-2 px-8 text-center"
              data-testid="mapphotos-empty"
            >
              <p className="text-sm text-text-secondary">{t("mapphotos.empty")}</p>
              <p className="text-xs text-text-muted">{t("map.empty")}</p>
            </div>
          ) : (
            <AssetGrid
              groups={groups}
              timelineDates={timeline.dates}
              onTimelineJump={timeline.jump}
              onNearTop={timeline.prepend}
              badges={badges}
              onOpenAsset={openAsset}
              onCtrlClick={ctrlSelect}
              onLongPress={ctrlSelect}
              onCheckClick={ctrlSelect}
              onAssetContextMenu={handleTileContextMenu}
              selection={selecting ? { active: true, selected, onToggle: toggleSelected } : undefined}
              sentinelRef={sentinelRef}
              loadingMore={loadingMore}
              hasMore={hasMoreRef.current}
              loadingTestId="mapphotos-loading-more"
              layout="justify"
              tile={GALLERY_JUSTIFY_ROW_PX[tileSize]}
              scrollTestId="mapphotos-grid-scroll"
            />
          )}
        </div>
      </div>

      {/* 右键操作和后续弹窗（AssetActionsHost：批量标记/加册/回收站） */}
      <AssetActionsHost
        assets={assets}
        selectedIds={selected}
        onSelectIds={setSelected}
        onDone={exitSelection}
        contextMenu={ctxMenu}
        onCloseContextMenu={() => setCtxMenu(null)}
        onPatched={patchAssets}
        onRemoved={removeAssets}
      />

      {/* 全屏查看器 */}
      {viewer && (
        <ViewerOverlay
          asset={viewer.asset}
          group={viewer.group}
          index={viewer.index}
          onAssetPatched={(id, patch) => patchAssets([id], patch)}
          onVersionSelect={selectVersion}
          onNavigate={navigateTo}
          onClose={closeViewer}
        />
      )}
    </motion.div>
  );
}
