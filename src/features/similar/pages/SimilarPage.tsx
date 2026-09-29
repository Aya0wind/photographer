import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  assetTrashMoveChecked,
  duplicatesList,
  type DuplicateGroupDto,
  type AssetDto,
} from "@/ipc/api";
import ViewerOverlay from "@/features/gallery/components/ViewerOverlay";
import { useAssetViewer } from "@/features/gallery/lib/useAssetViewer";
import AssetThumb from "@/features/gallery/components/AssetThumb";
import TileSizeSwitch from "@/features/gallery/components/TileSizeSwitch";
import { useGalleryTileSize, type GalleryTileSize } from "@/features/gallery/lib/useGalleryTileSize";

/**
 * 相似照片页（M7 F8 两级去重，/similar）：
 * - 相似组：pHash 汉明 ≤6 近似
 *   （RAW+JPG 孪生后端已排除；连拍组内不排除——正是挑片场景）
 * - 组卡片列表：组头「N 张 · 近似」，组内缩略图走 thumbPipeline
 *   240 档（AssetThumb 与画廊同款）；点击照片预览；独立勾选要删的照片 → 「删除所选」二次确认
 *   → assetTrashMoveChecked（移入回收站——2026-09-29 定案：除回收站外
 *     一律软删，真删只发生在回收站页）
 * - 删除后乐观更新：组内剔除已删项，<2 张的组整卡移除（后端同样不再返回）；
 *   游标 seen 同步减去消失的组数——after 是 0 基组偏移（skip 计数），
 *   服务端列表收缩后偏移对齐，加载更多不会跳组
 * - 分页：底部「加载更多」按钮（after=seen；短页=到底）
 * - 空态分 kind 正面文案；后端未就绪（duplicatesList 静默 []）同样空态
 */

/** 每页组数（后端上限 100） */
const PAGE_GROUPS = 20;
/** 组卡缩略图名义边长（后端 snap 256 档，与网格/胶片条同缓存） */
const CARD_THUMB_PX = 240;

interface GroupCardProps {
  group: DuplicateGroupDto;
  index: number;
  onDelete: (index: number, assetIds: number[]) => void;
  tileSize: GalleryTileSize;
  onOpen: (asset: AssetDto) => void;
}

/** 单个重复组卡：组头 + 勾选区 + 删除所选（勾选态由本卡自持）。
 *  选择（2026-09-29 定案：划选退役——适用移动端而非 PC）：勾选钮点击
 *  切换单张；Shift+点击从锚点拉组内区间（Explorer 语义，锚点不随 Shift
 *  移动）。 */
function GroupCard({ group, index, onDelete, tileSize, onOpen }: GroupCardProps) {
  const { t } = useTranslation();
  const [selected, setSelected] = useState<Set<number>>(() => new Set());
  /** Shift 区间锚点：组内最近一次普通勾选；Shift 拉区间不动锚点 */
  const anchorRef = useRef<number | null>(null);

  const apply = (id: number, target: boolean) => {
    setSelected((current) => {
      if (current.has(id) === target) return current;
      const next = new Set(current);
      if (target) next.add(id);
      else next.delete(id);
      return next;
    });
  };

  const toggle = (assetId: number, checked: boolean, shift = false) => {
    const target = !checked;
    // Shift：锚点到该瓦片的组内区间一步到位（方向=被点瓦片的目标态）
    if (shift && anchorRef.current !== null) {
      const assets = group.assets;
      const anchorIdx = assets.findIndex((item) => item.id === anchorRef.current);
      const targetIdx = assets.findIndex((item) => item.id === assetId);
      if (anchorIdx >= 0 && targetIdx >= 0) {
        const [lo, hi] = anchorIdx < targetIdx ? [anchorIdx, targetIdx] : [targetIdx, anchorIdx];
        for (let i = lo; i <= hi; i++) apply(assets[i].id, target);
        return;
      }
      // 锚点不在组内（异常态）：按无锚点重新起锚
    }
    anchorRef.current = assetId;
    apply(assetId, target);
  };

  const ids = [...selected];

  return (
    <section
      className="rounded-lg border border-edge bg-surface p-4"
      data-testid="similar-group"
      data-group-index={index}
      data-kind={group.kind}
    >
      <div className="flex items-center gap-2 pb-3">
        <h2 className="text-xs font-semibold text-text-primary">
          {t("similar.groupCount", { count: group.assets.length })}
        </h2>
        <span className="rounded bg-panel px-1.5 py-0.5 text-[10px] leading-none text-text-secondary">
          {t("similar.kind.similar")}
        </span>
        <span className="text-[10px] text-text-muted">{t("similar.selectHint")}</span>
        <button
          type="button"
          disabled={ids.length === 0}
          onClick={() => onDelete(index, ids)}
          className={`ml-auto shrink-0 rounded-md px-2.5 py-1 text-[11px] font-medium transition-colors ${
            ids.length === 0
              ? "cursor-default bg-panel text-text-muted"
              : "border border-red-400/60 text-red-400 hover:bg-red-400/10"
          }`}
          data-testid="similar-group-delete"
        >
          {t("similar.deleteSelected", { count: ids.length })}
        </button>
      </div>
      <div className={`grid touch-none select-none gap-2 ${tileSize === "small" ? "grid-cols-[repeat(auto-fill,minmax(120px,1fr))]" : "grid-cols-[repeat(auto-fill,minmax(200px,1fr))]"}`}>
        {group.assets.map((asset) => {
          const checked = selected.has(asset.id);
          return (
            <div
              key={asset.id}
              className={`relative aspect-[4/3] overflow-hidden rounded-md border transition-colors ${
                checked ? "border-red-400 ring-1 ring-red-400/60" : "border-edge/60 hover:border-accent/60"
              }`}
              data-testid="similar-asset"
              data-asset-id={asset.id}
              data-selected={checked ? "true" : undefined}
              title={asset.name}
            >
              <button type="button" onClick={() => onOpen(asset)} className="h-full w-full" aria-label={asset.name} data-testid="similar-asset-preview">
                <AssetThumb asset={asset} size={CARD_THUMB_PX} className="h-full w-full" />
              </button>
              <button
                type="button"
                onPointerDown={(e) => e.stopPropagation()}
                onClick={(e) => {
                  e.stopPropagation();
                  toggle(asset.id, checked, e.shiftKey);
                }}
                aria-pressed={checked}
                aria-label={t("similar.selectPhoto")}
                className={`absolute left-1.5 top-1.5 flex h-4 w-4 touch-none items-center justify-center rounded border text-[10px] font-bold leading-none transition-colors ${
                  checked
                    ? "border-red-400 bg-red-400 text-white"
                    : "border-white/50 bg-black/40 text-transparent"
                }`}
                data-testid="similar-asset-check"
              >
                ✓
              </button>
            </div>
          );
        })}
      </div>
    </section>
  );
}

/** 删除确认弹窗（红色强确认，两步防误触） */
function ConfirmDialog({
  count,
  onConfirm,
  onCancel,
}: {
  count: number;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
      role="dialog"
      aria-modal="true"
      data-testid="similar-confirm"
    >
      <div className="w-80 rounded-lg border border-edge bg-surface p-4 shadow-xl">
        <p className="text-sm text-text-primary">{t("similar.deleteConfirm", { count })}</p>
        <div className="mt-4 flex justify-end gap-2">
          <button
            type="button"
            onClick={onCancel}
            className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:text-text-primary"
            data-testid="similar-confirm-cancel"
          >
            {t("similar.deleteCancel")}
          </button>
          <button
            type="button"
            onClick={onConfirm}
            className="rounded-md bg-red-500 px-3 py-1.5 text-xs font-medium text-white transition-colors hover:bg-red-600"
            data-testid="similar-confirm-yes"
          >
            {t("similar.deleteConfirmYes")}
          </button>
        </div>
      </div>
    </div>
  );
}

export default function SimilarPage() {
  const { t } = useTranslation();
  const [tileSize, setTileSize] = useGalleryTileSize();

  const [groups, setGroups] = useState<DuplicateGroupDto[]>([]);
  /** 服务端游标（已取组数；after 为 0 基组偏移） */
  const [seen, setSeen] = useState(0);
  const [hasMore, setHasMore] = useState(false);
  const [status, setStatus] = useState<"loading" | "ready">("loading");
  const [loadingMore, setLoadingMore] = useState(false);
  /** 待确认删除：组下标 + 资产 id 集 */
  const [pending, setPending] = useState<{ index: number; ids: number[] } | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [feedback, setFeedback] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const viewerGroups = useMemo(() => [{
    key: "similar", date: null,
    assets: [...new Map(groups.flatMap((group) => group.assets).map((asset) => [asset.id, asset])).values()],
  }], [groups]);
  const { viewer, openAsset, closeViewer, navigateTo, selectVersion } = useAssetViewer(viewerGroups);

  const loadFirstPage = useCallback(async () => {
    setStatus("loading");
    setFeedback(null);
    setError(null);
    const first = await duplicatesList("similar", 0, PAGE_GROUPS);
    setGroups(first);
    setSeen(first.length);
    setHasMore(first.length === PAGE_GROUPS);
    setStatus("ready");
  }, []);

  // 进页 / 切档拉首页
  useEffect(() => {
    void loadFirstPage();
  }, [loadFirstPage]);

  const loadMore = async () => {
    if (loadingMore) return;
    setLoadingMore(true);
    const next = await duplicatesList("similar", seen, PAGE_GROUPS);
    setGroups((current) => [...current, ...next]);
    setSeen((current) => current + next.length);
    if (next.length < PAGE_GROUPS) setHasMore(false);
    setLoadingMore(false);
  };

  const confirmDelete = async () => {
    if (!pending || deleting) return;
    setDeleting(true);
    setError(null);
    try {
      await assetTrashMoveChecked(pending.ids);
      const deleted = pending.ids.length;
      // 乐观更新：组内剔除已删项；<2 张的组整卡移除，游标同步收缩
      //（after 是 0 基组偏移——服务端列表少了几组，偏移也要减同数）
      const remaining = groups
        .map((group, i) =>
          i === pending.index
            ? { ...group, assets: group.assets.filter((a) => !pending.ids.includes(a.id)) }
            : group,
        )
        .filter((group) => group.assets.length >= 2);
      const dropped = groups.length - remaining.length;
      setGroups(remaining);
      setSeen((current) => Math.max(0, current - dropped));
      setFeedback(t("similar.deleted", { count: deleted }));
    } catch (err) {
      // 后端 Err 文案透传（如未选库）
      setError(err instanceof Error ? err.message : typeof err === "string" ? err : null);
    } finally {
      setDeleting(false);
      setPending(null);
    }
  };

  return (
    <div className="h-full" data-testid="similar-page">
      <div
        className="sp-scroll h-full w-full overflow-y-auto px-4"
        data-testid="similar-content"
      >
        {/* 头部：标题 + 两档 Tab */}
        <div className="flex h-11 shrink-0 items-center gap-3 border-b border-edge">
          <h1 className="text-sm font-semibold text-text-primary">{t("similar.title")}</h1>
          <p className="text-xs text-text-muted">{t("similar.desc")}</p>
          <div className="ml-auto"><TileSizeSwitch value={tileSize} onChange={setTileSize} /></div>
        </div>

        {/* 删除反馈 / 错误（行内，切档清除） */}
        {feedback !== null && (
          <p className="pt-2 text-[11px] text-emerald-400" data-testid="similar-feedback">
            {feedback}
          </p>
        )}
        {error !== null && (
          <p className="pt-2 text-[11px] text-red-400" role="alert" data-testid="similar-error">
            {error}
          </p>
        )}

        {status === "loading" ? (
          <div
            className="flex h-full items-center justify-center text-xs text-text-muted"
            data-testid="similar-loading"
          >
            {t("similar.loading")}
          </div>
        ) : groups.length === 0 ? (
          <div
            className="flex h-full flex-col items-center justify-center gap-2 px-8 text-center"
            data-testid="similar-empty"
          >
            <p className="text-sm text-text-secondary">
              {t("similar.empty.similar")}
            </p>
            <p className="text-xs text-text-muted">{t("similar.emptyHint")}</p>
          </div>
        ) : (
          <div className="flex flex-col gap-4 py-4">
            {groups.map((group, index) => (
              <GroupCard
                key={`${group.kind}-${group.assets[0]?.id ?? index}-${index}`}
                group={group}
                index={index}
                tileSize={tileSize}
                onOpen={openAsset}
                onDelete={(groupIndex, ids) => setPending({ index: groupIndex, ids })}
              />
            ))}
            {hasMore && (
              <button
                type="button"
                onClick={() => void loadMore()}
                disabled={loadingMore}
                className="mx-auto rounded-md border border-edge px-4 py-1.5 text-xs text-text-secondary transition-colors hover:border-accent/60 hover:text-text-primary disabled:opacity-60"
                data-testid="similar-load-more"
              >
                {loadingMore ? t("similar.loadingMore") : t("similar.loadMore")}
              </button>
            )}
          </div>
        )}
      </div>

      {viewer && <ViewerOverlay asset={viewer.asset} group={viewer.group} index={viewer.index}
        onNavigate={navigateTo} onClose={closeViewer} onVersionSelect={selectVersion} />}

      {pending !== null && (
        <ConfirmDialog
          count={pending.ids.length}
          onConfirm={() => void confirmDelete()}
          onCancel={() => setPending(null)}
        />
      )}
    </div>
  );
}
