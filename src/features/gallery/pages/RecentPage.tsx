import { usePhotoCards } from "@/features/gallery/lib/usePhotoCards";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { recentViewed, type AssetDto } from "@/ipc/api";
import { groupAssetsByDate } from "../lib/assetGroups";
import AssetGrid from "../components/AssetGrid";
import AssetActionsHost from "../components/AssetActionsHost";
import TileSizeSwitch from "../components/TileSizeSwitch";
import ViewerOverlay from "../components/ViewerOverlay";
import { useAssetSelection } from "../lib/useAssetSelection";
import { useAssetViewer } from "../lib/useAssetViewer";
import { GALLERY_TILE_PX, useGalleryTileSize } from "../lib/useGalleryTileSize";

/**
 * 最近浏览页（M4.5 B3 → 2026-09-20 改向「最近浏览」，/recent 路由保留）：
 * - 数据源 recentViewed(200)：按最后浏览时间 DESC、同资产取最新一次（查看器
 *   打开/切图经 markAssetViewed 打点，见 lib/viewMark）
 * - 复用 AssetGrid square 布局 + 查看器（打点闭环：在此页浏览也会进入历史）
 * - 多选（2026-09-29 补齐）：与图库一致的操作条（收藏/旗标/色标/拒绝/
 *   分享/加册/全选/反选/回收站），Ctrl+点击/长按/勾选进入
 * - 后端命令未就绪 / 无浏览记录：空态「打开过的照片会出现在这里」
 */

/** 拉取上限（契约：后端封顶 200；单页渲染无需分页） */
const VIEWED_LIMIT = 200;

export default function RecentPage() {
  const { t } = useTranslation();

  const [assets, setAssets] = useState<AssetDto[]>([]);
  const [status, setStatus] = useState<"loading" | "ready">("loading");

  // 进入页面拉一次（浏览历史量小，无哨兵补页）
  useEffect(() => {
    let cancelled = false;
    void recentViewed(VIEWED_LIMIT).then((list) => {
      if (cancelled) return;
      setAssets(list);
      setStatus("ready");
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const { cards, badges } = usePhotoCards(assets);
  const groups = useMemo(() => groupAssetsByDate(cards), [cards]);
  const { viewer, openAsset, closeViewer, navigateTo, selectVersion } = useAssetViewer(groups, assets);
  const [tileSize, setTileSize] = useGalleryTileSize();

  // --- 多选与右键批量操作（弹窗由 AssetActionsHost 接齐） ---
  const { selecting, selected, ctrlSelect, toggleSelected, setSelected, exitSelection, contextTargets } = useAssetSelection();
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number; assets: AssetDto[] } | null>(null);
  const patchAssets = useCallback((ids: number[], patch: Partial<AssetDto>) => {
    const idSet = new Set(ids);
    setAssets((prev) => prev.map((a) => (idSet.has(a.id) ? { ...a, ...patch } : a)));
  }, []);

  return (
    <div className="h-full" data-testid="recent-page">
      <div
        className="flex h-full w-full flex-col px-4"
        data-testid="recent-content"
      >
        {/* 头部：标题 + 说明 + 尺寸切换 */}
        <div className="flex h-11 shrink-0 items-center gap-3 border-b border-edge">
          <h1 className="text-sm font-semibold text-text-primary">{t("recent.title")}</h1>
          <p className="text-xs text-text-muted">{t("recent.desc")}</p>
          <div className="ml-auto shrink-0">
            <TileSizeSwitch value={tileSize} onChange={setTileSize} />
          </div>
        </div>

        {/* 网格 / 空态 */}
        <div className="relative min-h-0 flex-1">
          {status === "loading" ? (
            <div
              className="flex h-full items-center justify-center text-xs text-text-muted"
              data-testid="recent-loading"
            >
              {t("recent.loading")}
            </div>
          ) : assets.length === 0 ? (
            <div
              className="flex h-full flex-col items-center justify-center gap-2 px-8 text-center"
              data-testid="recent-empty"
            >
              <p className="text-sm text-text-secondary">{t("recent.empty")}</p>
              <p className="text-xs text-text-muted">{t("recent.emptyHint")}</p>
            </div>
          ) : (
            <AssetGrid
              badges={badges}
              groups={groups}
              onOpenAsset={openAsset}
              tile={GALLERY_TILE_PX[tileSize]}
              scrollTestId="recent-grid-scroll"
              selection={selecting ? { active: true, selected, onToggle: toggleSelected } : undefined}
              onCtrlClick={ctrlSelect}
              onLongPress={ctrlSelect}
              onCheckClick={ctrlSelect}
              onAssetContextMenu={(asset, at) => setContextMenu({ ...at, assets: contextTargets(asset, new Map(assets.map((item) => [item.id, item]))) })}
            />
          )}
        </div>
      </div>

      {/* 右键操作和后续弹窗始终挂载，支持未进入多选时的单张操作。 */}
      {(
        <AssetActionsHost
          assets={assets}
          selectedIds={selected}
          onSelectIds={setSelected}
          onDone={exitSelection}
          contextMenu={contextMenu}
          onCloseContextMenu={() => setContextMenu(null)}
          onPatched={patchAssets}
          onRemoved={(ids) => {
            const idSet = new Set(ids);
            setAssets((prev) => prev.filter((a) => !idSet.has(a.id)));
          }}
        />
      )}

      {viewer && (
        <ViewerOverlay
          asset={viewer.asset}
          group={viewer.group}
          index={viewer.index}
          onVersionSelect={selectVersion}
          onNavigate={navigateTo}
          onClose={closeViewer}
        />
      )}
    </div>
  );
}
