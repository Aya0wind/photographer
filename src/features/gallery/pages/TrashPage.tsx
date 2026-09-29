import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  trashList,
  trashPurge,
  trashRestore,
  type AssetDto,
} from "@/ipc/api";
import { groupAssetsByDate } from "../lib/assetGroups";
import AssetGrid from "../components/AssetGrid";
import TileSizeSwitch from "../components/TileSizeSwitch";
import { GALLERY_TILE_PX, useGalleryTileSize } from "../lib/useGalleryTileSize";

/**
 * 回收站页（B1）：软删资产的集中管理——
 * - 数据源 trash_list（trashedAt DESC keyset；单页 200，「加载更多」补页）
 * - 网格复用 AssetGrid（readOnly 只读态：隐藏收藏等写入口；多选机制复用——
 *   点击/check 圆钮即切换选中，不打开查看器）
 * - 批量恢复（trash_restore）/ 彻底删除（trash_purge 两档确认：
 *   「从库中移除（保留文件）」deleteFiles=false /「同时删除文件」true，
 *   列出 N 项并提示不可恢复）
 * - 移入回收站在图库侧完成（多选操作条/右键菜单），此处不做
 */

const TRASH_PAGE_LIMIT = 200;

export default function TrashPage() {
  const { t } = useTranslation();

  const [assets, setAssets] = useState<AssetDto[]>([]);
  const [status, setStatus] = useState<"loading" | "ready">("loading");
  const [hasMore, setHasMore] = useState(false);
  const [selected, setSelected] = useState<number[]>([]);
  /** 彻底删除确认弹窗目标（null=关闭；deleteFiles 两档由弹窗内选择） */
  const [purgeTargets, setPurgeTargets] = useState<number[] | null>(null);
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

  // --- 多选（复用 AssetGrid 选择机制；点击瓦片=切换选中） --------------------------------
  const toggleSelected = useCallback((asset: AssetDto) => {
    setSelected((prev) =>
      prev.includes(asset.id) ? prev.filter((id) => id !== asset.id) : [...prev, asset.id],
    );
  }, []);
  const exitSelection = useCallback(() => setSelected([]), []);

  /** 本地从列表剔除（乐观更新；IPC 失败靠重进页面重拉兜底） */
  const removeFromList = useCallback((ids: number[]) => {
    const set = new Set(ids);
    setAssets((prev) => prev.filter((a) => !set.has(a.id)));
    setSelected((prev) => prev.filter((id) => !set.has(id)));
  }, []);

  // --- 恢复 ----------------------------------------------------------------------------
  async function restore(): Promise<void> {
    if (selected.length === 0) return;
    const ok = await trashRestore(selected);
    if (!ok) {
      flash(t("trash.restoreFailed"));
      return;
    }
    removeFromList(selected);
    flash(t("trash.restored", { count: selected.length }));
  }

  // --- 彻底删除（两档确认） --------------------------------------------------------------
  async function purge(deleteFiles: boolean): Promise<void> {
    if (purgeTargets === null || purgeTargets.length === 0) return;
    const ids = purgeTargets;
    setPurgeTargets(null);
    try {
      await trashPurge(ids, deleteFiles);
    } catch {
      flash(t("trash.purgeFailed"));
      return;
    }
    removeFromList(ids);
    flash(deleteFiles ? t("trash.purgeDoneFiles", { count: ids.length }) : t("trash.purgeDone", { count: ids.length }));
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
          <div className="ml-auto shrink-0">
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
                selection={{ active: true, selected, onToggle: toggleSelected }}
                onCheckClick={toggleSelected}
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

      {/* 底部多选操作条：恢复 / 彻底删除（红，两档确认）/ 取消 */}
      {selected.length > 0 && (
        <div className="fixed bottom-6 left-1/2 z-30 -translate-x-1/2" data-testid="trash-actions" data-count={selected.length}>
          <div className="flex items-center gap-1.5 rounded-full border border-edge bg-surface px-3 py-1.5 shadow-xl">
            <span className="shrink-0 font-mono text-[11px] tabular-nums text-accent" data-testid="trash-selected-count">
              {t("trash.selectedCount", { count: selected.length })}
            </span>
            <span className="h-4 w-px bg-edge" aria-hidden="true" />
            {/* 全选（已全选再点=取消全选）/ 反选：数据窗口=当前已加载列表 */}
            <button
              type="button"
              onClick={() => {
                const allSelected = selected.length > 0 && selected.length === assets.length;
                setSelected(allSelected ? [] : assets.map((a) => a.id));
              }}
              className="rounded-full px-2.5 py-1 text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent"
              data-testid="trash-select-all"
            >
              {selected.length > 0 && selected.length === assets.length
                ? t("selection.deselectAll")
                : t("selection.all")}
            </button>
            <button
              type="button"
              onClick={() => {
                const selectedSet = new Set(selected);
                setSelected(assets.filter((a) => !selectedSet.has(a.id)).map((a) => a.id));
              }}
              className="rounded-full px-2.5 py-1 text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent"
              data-testid="trash-invert"
            >
              {t("selection.invert")}
            </button>
            <span className="h-4 w-px bg-edge" aria-hidden="true" />
            <button
              type="button"
              onClick={() => void restore()}
              className="rounded-full px-2.5 py-1 text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent"
              data-testid="trash-restore"
            >
              {t("trash.restore")}
            </button>
            <button
              type="button"
              onClick={() => setPurgeTargets([...selected])}
              className="rounded-full px-2.5 py-1 text-[11px] text-red-400 transition-colors hover:bg-red-400/10"
              data-testid="trash-purge-open"
            >
              {t("trash.purge")}
            </button>
            <span className="h-4 w-px bg-edge" aria-hidden="true" />
            <button
              type="button"
              onClick={exitSelection}
              className="rounded-full px-2 py-1 text-[11px] text-text-muted transition-colors hover:bg-panel hover:text-text-primary"
              data-testid="trash-cancel"
            >
              {t("common.cancel")}
            </button>
          </div>
        </div>
      )}

      {/* 操作反馈 toast（操作条可能已随清空选中收起，独立于操作条渲染） */}
      {toast !== null && (
        <div className="pointer-events-none fixed bottom-20 left-1/2 z-30 -translate-x-1/2">
          <p className="rounded-full border border-edge bg-surface px-3 py-1 text-[11px] text-text-secondary shadow-xl" data-testid="trash-toast" role="status">
            {toast}
          </p>
        </div>
      )}

      {/* 彻底删除两档确认弹窗：列出 N 项 + 不可恢复提示 */}
      {purgeTargets !== null && (
        <div
          className="fixed inset-0 z-40 flex items-center justify-center bg-black/55 p-6"
          data-testid="trash-purge-dialog"
          role="dialog"
          aria-modal="true"
          aria-label={t("trash.purgeTitle")}
          onClick={(e) => {
            if (e.target === e.currentTarget) setPurgeTargets(null);
          }}
        >
          <div className="w-full max-w-md rounded-xl border border-edge bg-surface p-4 shadow-2xl">
            <h2 className="text-sm font-semibold text-text-primary" data-testid="trash-purge-title">
              {t("trash.purgeTitle", { count: purgeTargets.length })}
            </h2>
            <p className="mt-2 text-xs leading-relaxed text-text-secondary">
              {t("trash.purgeDesc", { count: purgeTargets.length })}
            </p>
            <p className="mt-1 text-xs text-red-400">{t("trash.purgeIrreversible")}</p>
            <div className="mt-4 flex flex-col gap-2">
              <button
                type="button"
                onClick={() => void purge(false)}
                className="rounded-md border border-red-400/60 px-3 py-2 text-xs text-red-400 transition-colors hover:bg-red-400/10"
                data-testid="trash-purge-keep"
              >
                {t("trash.purgeKeep")}
              </button>
              <button
                type="button"
                onClick={() => void purge(true)}
                className="rounded-md bg-red-500 px-3 py-2 text-xs font-medium text-white transition-colors hover:bg-red-500/85"
                data-testid="trash-purge-files"
              >
                {t("trash.purgeFiles")}
              </button>
              <button
                type="button"
                onClick={() => setPurgeTargets(null)}
                className="rounded-md border border-edge px-3 py-2 text-xs text-text-secondary transition-colors hover:bg-panel hover:text-text-primary"
                data-testid="trash-purge-cancel"
              >
                {t("common.cancel")}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
