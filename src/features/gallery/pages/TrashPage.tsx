import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  trashList,
  trashEmpty,
  trashPurge,
  trashRestore,
  type AssetDto,
} from "@/ipc/api";
import { notifyCountsRefresh } from "@/lib/countsRefresh";
import { groupAssetsByDate } from "../lib/assetGroups";
import AssetGrid from "../components/AssetGrid";
import { useAssetSelection } from "../lib/useAssetSelection";
import TileSizeSwitch from "../components/TileSizeSwitch";
import { GALLERY_TILE_PX, useGalleryTileSize } from "../lib/useGalleryTileSize";
import ContextMenu from "../components/ContextMenu";
import TrashPurgeDialog from "../components/TrashPurgeDialog";

/**
 * 回收站页（B1）：软删资产的集中管理——
 * - 数据源 trash_list（trashedAt DESC keyset；单页 200，「加载更多」补页）
 * - 网格复用 AssetGrid（readOnly 只读态：隐藏收藏等写入口；多选机制复用——
 *   点击/check 圆钮即切换选中，不打开查看器）
 * - 批量恢复 / 删除所选 / 清空整个回收站；默认保留文件。
 *   删除原文件通过勾选并输入“确认删除”启用，前后端均校验。
 * - 移入回收站在图库侧完成（多选操作条/右键菜单），此处不做
 */

const TRASH_PAGE_LIMIT = 200;
const DELETE_CONFIRMATION = "确认删除";

export default function TrashPage() {
  const { t } = useTranslation();

  const [assets, setAssets] = useState<AssetDto[]>([]);
  const [status, setStatus] = useState<"loading" | "ready">("loading");
  const [hasMore, setHasMore] = useState(false);
  // 共享选择钩子（Shift 区间 + 锚点；回收站恒多选态，selecting 不用）
  const { selected, setSelected, ctrlSelect } = useAssetSelection();
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number; ids: number[] } | null>(null);
  /** null=关闭，all=整个回收站，数组=固定选中快照。 */
  const [purgeTargets, setPurgeTargets] = useState<number[] | "all" | null>(null);
  const [purging, setPurging] = useState(false);
  const [purgeError, setPurgeError] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);

  function flash(message: string): void {
    setToast(message);
    window.setTimeout(() => setToast(null), 1800);
  }

  const appendTrash = useCallback(async (): Promise<void> => {
    const afterId = assets.length > 0 ? assets[assets.length - 1].id : 0;
    const page = await trashList(afterId, TRASH_PAGE_LIMIT);
    if (page.length < TRASH_PAGE_LIMIT) setHasMore(false);
    else setHasMore(true);
    if (page.length === 0) return;
    const seen = new Set(assets.map((a) => a.id));
    const fresh = page.filter((a) => !seen.has(a.id));
    if (fresh.length === 0) return;
    setAssets((prev) => [...prev, ...fresh]);
  }, [assets]);

  // 挂载拉首页
  useEffect(() => {
    let cancelled = false;
    void trashList(0, TRASH_PAGE_LIMIT).then((page) => {
      if (cancelled) return;
      setAssets(page);
      setHasMore(page.length >= TRASH_PAGE_LIMIT);
      setStatus("ready");
    });
    return () => {
      cancelled = true;
    };
  }, []);



  /** 本地从列表剔除（乐观更新；IPC 失败靠重进页面重拉兜底） */
  const removeFromList = useCallback((ids: number[]) => {
    const set = new Set(ids);
    setAssets((prev) => prev.filter((a) => !set.has(a.id)));
    setSelected((prev) => prev.filter((id) => !set.has(id)));
  }, []);

  // --- 恢复 ----------------------------------------------------------------------------
  async function restore(ids: number[]): Promise<void> {
    if (ids.length === 0) return;
    const ok = await trashRestore(ids);
    if (!ok) {
      flash(t("trash.restoreFailed"));
      return;
    }
    removeFromList(ids);
    flash(t("trash.restored", { count: ids.length }));
    // 恢复回库改变 sidebar_counts 口径内的行（in_trash=0）——通知侧栏重拉
    notifyCountsRefresh();
  }

  function openPurge(targets: number[] | "all"): void {
    if (purging) return;
    setPurgeError(null);
    setPurgeTargets(targets);
  }

  async function reloadTrash(): Promise<void> {
    const page = await trashList(0, TRASH_PAGE_LIMIT);
    setAssets(page);
    setHasMore(page.length >= TRASH_PAGE_LIMIT);
    setSelected([]);
  }

  async function purge(deleteFiles: boolean, confirmation: string): Promise<void> {
    if (purging || purgeTargets === null || (purgeTargets !== "all" && purgeTargets.length === 0)) return;
    if (deleteFiles && confirmation !== DELETE_CONFIRMATION) return;
    setPurgeError(null);
    setPurging(true);
    try {
      const result = purgeTargets === "all"
        ? await trashEmpty(deleteFiles, deleteFiles ? confirmation : undefined)
        : await trashPurge(purgeTargets, deleteFiles, deleteFiles ? confirmation : undefined);
      setPurgeTargets(null);
      await reloadTrash();
      const message = t(deleteFiles ? "trash.purgeDoneFiles" : "trash.purgeDone", { count: result.deletedRecords });
      flash(result.offlineKept > 0 ? `${message} · ${t("trash.offlineKept", { count: result.offlineKept })}` : message);
    } catch {
      setPurgeError(t("trash.purgeFailed"));
      flash(t("trash.purgeFailed"));
      // A whole-trash operation may have committed earlier batches before an error.
      await reloadTrash();
    } finally {
      notifyCountsRefresh();
      setPurging(false);
    }
  }

  const groups = useMemo(() => groupAssetsByDate(assets), [assets]);
  const [tileSize, setTileSize] = useGalleryTileSize();

  return (
    <div className="relative h-full" data-testid="trash-page">
      <div className="flex h-full w-full flex-col px-4" data-testid="trash-content">
        {/* 头部：标题 + 说明 + 尺寸切换 */}
        <div className="flex h-11 shrink-0 items-center gap-3 border-b border-edge">
          <h1 className="text-sm font-semibold text-text-primary">{t("trash.title")}</h1>
          <p className="text-xs text-text-muted">{t("trash.desc")}</p>
          <div className="ml-auto flex shrink-0 items-center gap-3">
            <button type="button" onClick={() => openPurge("all")} disabled={status !== "ready" || assets.length === 0 || purging}
              className="rounded-md border border-red-400/60 px-3 py-1.5 text-xs text-red-400 hover:bg-red-400/10 disabled:opacity-40"
              data-testid="trash-empty-all">{t("trash.emptyAll")}</button>
            <TileSizeSwitch value={tileSize} onChange={setTileSize} />
          </div>
        </div>

        {/* 网格 / 空态 */}
        <div className="relative min-h-0 flex-1">
          {status === "loading" ? (
            <div className="flex h-full items-center justify-center text-xs text-text-muted" data-testid="trash-loading">
              {t("trash.loading")}
            </div>
          ) : assets.length === 0 ? (
            <div className="flex h-full flex-col items-center justify-center gap-2 px-8 text-center" data-testid="trash-empty">
              <svg
                viewBox="0 0 24 24"
                width="44"
                height="44"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.2"
                strokeLinecap="round"
                strokeLinejoin="round"
                className="text-text-muted"
                aria-hidden="true"
              >
                <path d="M4 6h16M9 6V4h6v2M6 6l1 13a2 2 0 0 0 2 1.8h6A2 2 0 0 0 17 19l1-13" />
                <path d="M10 10.5v6M14 10.5v6" />
              </svg>
              <p className="text-sm text-text-secondary">{t("trash.empty")}</p>
              <p className="max-w-sm text-xs leading-relaxed text-text-muted">{t("trash.emptyHint")}</p>
            </div>
          ) : (
            <>
              <AssetGrid
                groups={groups}
                selection={{ active: true, selected, onToggle: ctrlSelect }}
                onCheckClick={ctrlSelect}
                onAssetContextMenu={(asset, at) => {
                  const ids = selected.includes(asset.id) ? selected : [asset.id];
                  setSelected(ids);
                  setContextMenu({ ...at, ids });
                }}
                readOnly
                tile={GALLERY_TILE_PX[tileSize]}
                scrollTestId="trash-grid-scroll"
              />
              {hasMore && (
                <div className="flex h-10 shrink-0 items-center justify-center">
                  <button
                    type="button"
                    onClick={() => void appendTrash()}
                    className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
                    data-testid="trash-load-more"
                  >
                    {t("trash.loadMore")}
                  </button>
                </div>
              )}
            </>
          )}
        </div>
      </div>

      {contextMenu && <ContextMenu at={{ x: contextMenu.x, y: contextMenu.y }} onClose={() => setContextMenu(null)} testId="trash-context-menu" entries={[
        { key: "restore", label: t("trash.restore"), onSelect: () => { void restore(contextMenu.ids); } },
        { key: "purge", label: t("trash.purge"), danger: true, onSelect: () => openPurge(contextMenu.ids) },
        { key: "select-all", label: t(selected.length === assets.length ? "selection.deselectAll" : "selection.all"), onSelect: () => setSelected(selected.length === assets.length ? [] : assets.map((asset) => asset.id)) },
        { key: "invert", label: t("selection.invert"), onSelect: () => setSelected(assets.filter((asset) => !selected.includes(asset.id)).map((asset) => asset.id)) },
      ]} />}

      {/* 操作反馈 toast（操作条可能已随清空选中收起，独立于操作条渲染） */}
      {toast !== null && (
        <div className="pointer-events-none fixed bottom-20 left-1/2 z-30 -translate-x-1/2">
          <p className="rounded-full border border-edge bg-surface px-3 py-1 text-[11px] text-text-secondary shadow-xl" data-testid="trash-toast" role="status">
            {toast}
          </p>
        </div>
      )}

      {purgeTargets !== null && <TrashPurgeDialog key={purgeTargets === "all" ? "all" : purgeTargets.join(",")} targets={purgeTargets} busy={purging} error={purgeError}
        onConfirm={purge} onCancel={() => setPurgeTargets(null)} />}

    </div>
  );
}
