import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { useTranslation } from "react-i18next";

import {
  albumAssetsPage,
  albumItemMoveSubgroup,
  albumList,
  albumRename,
  albumSubgroups,
  type AlbumDto,
  type AlbumSubgroupDto,
  type AssetDto,
  type AssetFilters,
} from "@/ipc/api";
import { groupAssetsByDate } from "@/features/gallery/lib/assetGroups";
import { useAssetViewer } from "@/features/gallery/lib/useAssetViewer";
import AssetGrid from "@/features/gallery/components/AssetGrid";
import { AssetContextMenu } from "@/features/gallery/components/ContextMenu";
import SelectionBar from "@/features/gallery/components/SelectionBar";
import ImportDerivedDialog from "@/features/albums/components/ImportDerivedDialog";
import TileSizeSwitch from "@/features/gallery/components/TileSizeSwitch";
import ViewerOverlay from "@/features/gallery/components/ViewerOverlay";
import AddToAlbumDialog from "@/features/albums/components/AddToAlbumDialog";
import {
  FilterChipsRow,
  FilterPanel,
  GroupRoleSegment,
  buildChips,
  buildFilters,
  hasActiveFilters,
  parseInputs,
  serializeInputs,
  EMPTY_INPUTS,
} from "@/features/gallery/FilterPanel";
import { useDebouncedValue } from "@/lib/useDebouncedValue";
import { GALLERY_JUSTIFY_ROW_PX, useGalleryTileSize } from "@/features/gallery/lib/useGalleryTileSize";

/**
 * 手工相册详情页（/albums/:id，经 AlbumEntryPage 数字参数分发进入）：
 * - 数据源 album_assets_page（keyset 游标分页，与 assets_page 同风格；按拍摄时间
 *   排序由后端保证）；无限滚动哨兵补页
 * - 复用图库 AssetGrid square 布局 + 查看器链路（参考 RecentPage）
 * - FilterPanel 可筛选（filters 透传；隐藏相册维度——本页已在相册上下文内）
 * - 多选操作条/右键菜单在相册上下文多一项「从相册移除」（仅删引用），同时保留
 *   「加入相册」（加入其他相册）；移除后重置重拉并刷新相册计数
 * - 页头：相册名（点击行内重命名）、张数、「添加照片」入口（v1：提示到图库多选加入）
 */

const PAGE_LIMIT = 100;
const DEBOUNCE_MS = 300;

export default function AlbumDetailPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { tag = "" } = useParams();
  const albumId = Number(tag);

  // --- 相册元数据（名称/引用计数；相册清单里找不到时兜底展示 id） -----------------------
  const [meta, setMeta] = useState<AlbumDto | null>(null);
  const refreshMeta = useCallback(async () => {
    const list = await albumList();
    setMeta(list.find((a) => a.id === albumId) ?? null);
  }, [albumId]);
  useEffect(() => {
    void refreshMeta();
  }, [refreshMeta]);

  const albumName = meta?.name ?? t("albums.fallbackName", { id: albumId });
  const itemCount = meta?.itemCount ?? 0;
  // --- 子分组（B4 定案）：null = 相册根散照片视图；进入子分组 = 页内状态切换 ------------
  const [subgroup, setSubgroup] = useState<string | null>(null);
  const [subgroups, setSubgroups] = useState<AlbumSubgroupDto[]>([]);
  const [derivedImportOpen, setDerivedImportOpen] = useState(false);
  const refreshSubgroups = useCallback(async () => {
    setSubgroups(await albumSubgroups(albumId));
  }, [albumId]);


  // --- 筛选（复用图库 FilterPanel；相册维度隐藏） --------------------------------------
  const [inputs, setInputs] = useState(EMPTY_INPUTS);
  const patchInputs = useCallback((patch: Partial<typeof EMPTY_INPUTS>) => {
    setInputs((prev) => ({ ...prev, ...patch }));
  }, []);
  const [panelOpen, setPanelOpen] = useState(false);
  const rawKey = serializeInputs(inputs);
  const debouncedKey = useDebouncedValue(rawKey, DEBOUNCE_MS);
  const filtersActive = useMemo(() => hasActiveFilters(parseInputs(debouncedKey)), [debouncedKey]);
  const chips = useMemo(() => buildChips(inputs, t), [inputs, t]);

  // --- 资产管线（keyset 分页；移除引用后 reloadToken 重置重拉） -------------------------
  const [assets, setAssets] = useState<AssetDto[]>([]);
  const [status, setStatus] = useState<"loading" | "ready">("loading");
  const [loadingMore, setLoadingMore] = useState(false);
  const assetsRef = useRef<AssetDto[]>([]);
  const hasMoreRef = useRef(true);
  const loadingRef = useRef(false);
  const loadSeqRef = useRef(0);
  const appliedKeyRef = useRef(debouncedKey);
  appliedKeyRef.current = debouncedKey;
  const [reloadToken, setReloadToken] = useState(0);
  const sentinelRef = useRef<HTMLDivElement | null>(null);

  const fetchPage = useCallback(
    async (afterId: number, key: string): Promise<AssetDto[]> => {
      // 子分组作用域（B4）：默认根视图 subgroupIsNull=true；子分组视图精确名。
      // 与面板筛选（groupRole 等）正交叠加。
      const pageFilters: AssetFilters = {
        ...(hasActiveFilters(parseInputs(key)) ? buildFilters(parseInputs(key)) : {}),
        ...(subgroup === null ? { subgroupIsNull: true } : { subgroup }),
      };
      return albumAssetsPage(albumId, afterId, PAGE_LIMIT, pageFilters);
    },
    [albumId, subgroup],
  );

  const appendPage = useCallback(async (): Promise<AssetDto[]> => {
    if (loadingRef.current || !hasMoreRef.current) return [];
    loadingRef.current = true;
    setLoadingMore(true);
    const afterId =
      assetsRef.current.length > 0 ? assetsRef.current[assetsRef.current.length - 1].id : 0;
    const page = await fetchPage(afterId, appliedKeyRef.current);
    // 补页不参与主加载的 seq 竞争（bump 会丢弃在途主加载且 status 卡 loading）：
    // 若 await 期间发生主加载重置（loadingRef 已被主管线清零），本页丢弃由新管线接管
    if (loadingRef.current) {
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

  useEffect(() => {
    let cancelled = false;
    const seq = ++loadSeqRef.current;
    setStatus("loading");
    hasMoreRef.current = true;
    loadingRef.current = false;
    void fetchPage(0, debouncedKey).then((page) => {
      if (cancelled || seq !== loadSeqRef.current) return;
      assetsRef.current = page;
      setAssets(page);
      if (page.length < PAGE_LIMIT) hasMoreRef.current = false;
      setStatus("ready");
    });
    return () => {
      cancelled = true;
    };
  }, [fetchPage, debouncedKey, reloadToken]);

  // 子分组清单（B4）：挂载/换相册/移组与导入后（reloadToken）刷新
  useEffect(() => {
    void refreshSubgroups();
  }, [refreshSubgroups, reloadToken]);

  // 无限滚动：哨兵进入视口（提前 800px 预载）
  useEffect(() => {
    const el = sentinelRef.current;
    if (!el || status !== "ready" || typeof IntersectionObserver === "undefined") return;
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) void appendPage();
      },
      { rootMargin: "800px 0px" },
    );
    io.observe(el);
    return () => io.disconnect();
  }, [status, appendPage, assets.length]);

  // --- 多选（与画廊同语义：check 圆钮 / Ctrl+点击 / 长按；Esc 退出） -------------------
  const [selecting, setSelecting] = useState(false);
  const [selected, setSelected] = useState<number[]>([]);
  const toggleSelected = useCallback((asset: AssetDto) => {
    setSelected((prev) =>
      prev.includes(asset.id) ? prev.filter((id) => id !== asset.id) : [...prev, asset.id],
    );
  }, []);
  const ctrlSelect = useCallback((asset: AssetDto) => {
    setSelecting(true);
    setSelected((prev) =>
      prev.includes(asset.id) ? prev.filter((id) => id !== asset.id) : [...prev, asset.id],
    );
  }, []);
  const exitSelection = useCallback(() => {
    setSelecting(false);
    setSelected([]);
  }, []);

  useEffect(() => {
    if (!selecting) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        exitSelection();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selecting, exitSelection]);


  function enterSubgroup(name: string): void {
    exitSelection();
    setSubgroup(name);
  }
  function backToRoot(): void {
    exitSelection();
    setSubgroup(null);
  }

  // --- 分组 + 查看器 -------------------------------------------------------------------
  const groups = useMemo(() => groupAssetsByDate(assets), [assets]);
  const { viewer, openAsset, closeViewer, navigateTo } = useAssetViewer(groups);
  useEffect(() => {
    if (!viewer || !hasMoreRef.current) return;
    if (viewer.index >= viewer.group.assets.length - 8) void appendPage();
  }, [viewer?.index, viewer?.group.assets.length, appendPage]);
  const [tileSize, setTileSize] = useGalleryTileSize();

  const loadedById = useMemo(() => {
    const map = new Map<number, AssetDto>();
    for (const group of groups) for (const a of group.assets) map.set(a.id, a);
    return map;
  }, [groups]);
  const selectedAssets = useMemo(
    () => selected.map((id) => loadedById.get(id)).filter((a): a is AssetDto => a !== undefined),
    [selected, loadedById],
  );

  // --- 瓦片右键菜单（相册上下文：多一项「从相册移除」）+ 加入相册弹窗 -------------------
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number; assets: AssetDto[] } | null>(null);
  const [addToAlbumTargets, setAddToAlbumTargets] = useState<AssetDto[] | null>(null);

  function handleTileContextMenu(asset: AssetDto, at: { x: number; y: number }): void {
    if (selecting) {
      if (selected.includes(asset.id)) {
        const targets = selected
          .map((id) => loadedById.get(id))
          .filter((a): a is AssetDto => a !== undefined);
        setCtxMenu({ ...at, assets: targets });
        return;
      }
      setSelected([asset.id]);
    }
    setCtxMenu({ ...at, assets: [asset] });
  }

  /** 相册上下文刷新：移除引用后重置分页重拉 + 相册计数 + 子分组清单 */
  const handleAssetsRemoved = useCallback(() => {
    exitSelection();
    setReloadToken((token) => token + 1);
    void refreshMeta();
  }, [exitSelection, refreshMeta]);

  /** 子分组移组（B4）：target=null 移回根；输入新名即建（后端按名幂等）。
   *  完成后重拉当前视图 + 子分组清单 + 相册计数。 */
  const handleSubgroupMove = useCallback(
    async (target: string | null, items: AssetDto[]): Promise<boolean> => {
      const ok = await albumItemMoveSubgroup(
        albumId,
        items.map((a) => a.id),
        target,
      );
      if (ok) {
        exitSelection();
        setReloadToken((token) => token + 1);
        void refreshMeta();
        void refreshSubgroups();
      }
      return ok;
    },
    [albumId, exitSelection, refreshMeta, refreshSubgroups],
  );

  // --- 页头行内重命名 -----------------------------------------------------------------
  const [renaming, setRenaming] = useState(false);
  const [renameName, setRenameName] = useState("");
  const [renameError, setRenameError] = useState<string | null>(null);

  async function submitRename(): Promise<void> {
    const name = renameName.trim();
    if (name === "" || name === albumName) {
      setRenaming(false);
      return;
    }
    const result = await albumRename(albumId, name);
    if (!result.ok) {
      setRenameError(result.error ?? t("albums.renameFailed"));
      return;
    }
    setMeta((prev) => (prev ? { ...prev, name } : prev));
    setRenaming(false);
    setRenameError(null);
  }

  // --- 「添加照片」入口（v1：提示到图库多选加入） ---------------------------------------
  const [addHintOpen, setAddHintOpen] = useState(false);

  return (
    <div className="h-full" data-testid="album-detail-page" data-album-id={albumId}>
      <div className="flex h-full w-full flex-col px-4">
        {/* 页头：相册名（点击重命名）+ 张数 + 添加照片 + 筛选 + 尺寸 */}
        <div className="flex h-11 shrink-0 items-center gap-2.5 border-b border-edge" data-testid="album-detail-toolbar">
          {renaming ? (
            <div className="flex min-w-0 items-center gap-1.5">
              <input
                autoFocus
                type="text"
                value={renameName}
                onChange={(e) => {
                  setRenameName(e.target.value);
                  setRenameError(null);
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    void submitRename();
                  }
                  if (e.key === "Escape") setRenaming(false);
                }}
                aria-label={t("albums.renameTitle")}
                className="h-7 w-48 rounded-md border border-edge bg-panel/55 px-2 text-sm text-text-primary outline-none transition-colors focus:border-accent"
                data-testid="album-detail-rename-input"
              />
              <button
                type="button"
                onClick={() => void submitRename()}
                className="h-7 rounded-md bg-accent px-2.5 text-xs font-medium text-black transition-colors hover:brightness-110"
                data-testid="album-detail-rename-confirm"
              >
                {t("albums.renameConfirm")}
              </button>
              {renameError !== null && (
                <span className="text-[11px] text-red-400" role="alert" data-testid="album-detail-rename-error">
                  {renameError}
                </span>
              )}
            </div>
          ) : (
            <button
              type="button"
              onClick={() => {
                setRenaming(true);
                setRenameName(albumName);
                setRenameError(null);
              }}
              title={t("albums.rename")}
              className="min-w-0 shrink-0 truncate text-sm font-semibold text-text-primary transition-colors hover:text-accent"
              data-testid="album-detail-name"
            >
              {albumName}
            </button>
          )}
          <span
            className="shrink-0 rounded-full bg-panel px-2 py-0.5 font-mono text-[11px] tabular-nums text-text-secondary"
            data-testid="album-detail-count"
          >
            {t("gallery.groupCount", { count: itemCount })}
          </span>

          {/* 原片/成片四态分段（B2）：全部/只看原片/只看成片/尚无成片；与筛选正交，
              在相册根/子分组视图内均作为照片属性辅助筛选 */}
          <GroupRoleSegment
            value={inputs.groupRole}
            onChange={(groupRole) => patchInputs({ groupRole })}
            testId="album-grouprole"
          />

          {/* 导入成片（B4 子分组定案）：LR 导出的成片导回本相册（目标子分组默认「成片」） */}
          <button
            type="button"
            onClick={() => setDerivedImportOpen(true)}
            className="flex shrink-0 items-center gap-1.5 rounded-md border border-violet-400/60 px-2.5 py-1 text-[11px] text-violet-300 transition-colors hover:bg-violet-400/10"
            data-testid="album-import-derived"
          >
            {t("albums.importDerived")}
          </button>

          {/* 筛选按钮（激活条件计数徽标） */}
          <button
            type="button"
            onClick={() => setPanelOpen((v) => !v)}
            aria-expanded={panelOpen}
            className={`flex shrink-0 items-center gap-1.5 rounded-md border px-2.5 py-1 text-[11px] transition-colors ${
              panelOpen || chips.length > 0
                ? "border-accent text-accent"
                : "border-edge text-text-secondary hover:border-text-muted hover:text-text-primary"
            }`}
            data-testid="album-detail-filter-toggle"
          >
            {t("search.filter")}
            {chips.length > 0 && (
              <span className="rounded-full bg-accent px-1.5 text-[10px] font-bold leading-4 text-black">
                {chips.length}
              </span>
            )}
          </button>

          <div className="relative shrink-0">
            <button
              type="button"
              onClick={() => setAddHintOpen((v) => !v)}
              aria-expanded={addHintOpen}
              className="flex items-center gap-1 rounded-md border border-edge px-2.5 py-1 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
              data-testid="album-add-photos"
            >
              {t("albums.addPhotos")}
            </button>
            {addHintOpen && (
              <div
                className="absolute left-0 top-8 z-20 w-64 rounded-xl border border-edge bg-surface p-3 shadow-2xl shadow-black/40"
                data-testid="album-add-photos-hint"
              >
                <p className="text-[11px] leading-relaxed text-text-secondary">
                  {t("albums.addPhotosHint")}
                </p>
                {/* 归入引导（目录化）：日期根未归册照片加入本相册时默认移动文件进相册目录 */}
                <p className="mt-1.5 text-[11px] leading-relaxed text-text-muted" data-testid="album-add-photos-claim-hint">
                  {t("albums.addPhotosClaimHint")}
                </p>
                <button
                  type="button"
                  onClick={() => navigate("/gallery")}
                  className="mt-2 w-full rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110"
                  data-testid="album-add-photos-go-gallery"
                >
                  {t("albums.goGallery")}
                </button>
              </div>
            )}
          </div>

          <div className="ml-auto shrink-0">
            <TileSizeSwitch value={tileSize} onChange={setTileSize} />
          </div>
        </div>

        {/* 筛选面板（相册维度隐藏：本页已在相册上下文内） */}
        {panelOpen && <FilterPanel inputs={inputs} onPatch={patchInputs} hideAlbum />}

        {/* 激活条件 chips */}
        <FilterChipsRow
          chips={chips}
          onPatch={(next) => setInputs(next)}
          onClearAll={() => setInputs(EMPTY_INPUTS)}
        />

        {/* 子分组视图面包屑（B4）：相册名 ‹ 子分组名 + 返回 */}
        {subgroup !== null && (
          <div
            className="flex shrink-0 items-center gap-2 border-b border-edge/60 py-1.5"
            data-testid="album-subgroup-bar"
          >
            <button
              type="button"
              onClick={backToRoot}
              className="flex shrink-0 items-center gap-1 rounded-md border border-edge px-2 py-1 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
              data-testid="album-subgroup-back"
            >
              <svg viewBox="0 0 16 16" width="9" height="9" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <path d="M10 3L5 8l5 5" />
              </svg>
              {t("albums.subgroupBack")}
            </button>
            <span className="min-w-0 truncate text-xs text-text-secondary" data-testid="album-subgroup-breadcrumb">
              {albumName}
              <span className="mx-1.5 text-text-muted">‹</span>
              <span className="font-medium text-text-primary">{subgroup}</span>
            </span>
          </div>
        )}

        {/* 子分组文件夹卡（B4，仅根视图）：文件夹图标 + 名称 + 张数；点击进入子分组视图 */}
        {subgroup === null && subgroups.length > 0 && (
          <div className="flex shrink-0 flex-wrap gap-2 border-b border-edge/60 py-2" data-testid="album-subgroups">
            {subgroups.map((group) => (
              <button
                key={group.name}
                type="button"
                onClick={() => enterSubgroup(group.name)}
                className="flex items-center gap-2 rounded-lg border border-edge bg-surface px-3 py-2 text-left transition-colors hover:border-accent"
                data-testid="album-subgroup-card"
                data-subgroup={group.name}
              >
                <svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" className="shrink-0 text-amber-400" aria-hidden="true">
                  <path d="M1.5 4a1 1 0 0 1 1-1h3.2l1.5 1.8h6.3a1 1 0 0 1 1 1V12a1 1 0 0 1-1 1h-11a1 1 0 0 1-1-1z" />
                </svg>
                <span className="min-w-0">
                  <span className="block max-w-[140px] truncate text-xs text-text-primary" title={group.name} data-testid="album-subgroup-name">
                    {group.name}
                  </span>
                  <span className="block font-mono text-[10px] tabular-nums text-text-muted" data-testid="album-subgroup-count">
                    {t("albums.itemCountBadge", { count: group.itemCount })}
                  </span>
                </span>
              </button>
            ))}
          </div>
        )}

        {/* 照片墙（与图库同款虚拟网格） */}
        <div className="relative min-h-0 flex-1">
          {status === "loading" ? (
            <div
              className="flex h-full items-center justify-center text-xs text-text-muted"
              data-testid="album-detail-loading"
            >
              {t("recent.loading")}
            </div>
          ) : assets.length === 0 ? (
            <div
              className="flex h-full flex-col items-center justify-center gap-2 px-8 text-center"
              data-testid="album-detail-empty"
            >
              <p className="text-sm text-text-secondary">
                {filtersActive
                  ? t("search.empty")
                  : subgroup !== null
                    ? t("albums.subgroupEmpty", { name: subgroup })
                    : t("albums.empty")}
              </p>
              {!filtersActive && (
                <p className="text-xs text-text-muted">{t("albums.addPhotosHint")}</p>
              )}
            </div>
          ) : (
            <AssetGrid
              groups={groups}
              onOpenAsset={openAsset}
              onCtrlClick={ctrlSelect}
              onLongPress={ctrlSelect}
              onCheckClick={ctrlSelect}
              onAssetContextMenu={handleTileContextMenu}
              selection={selecting ? { active: true, selected, onToggle: toggleSelected } : undefined}
              sentinelRef={sentinelRef}
              layout="justify"
              tile={GALLERY_JUSTIFY_ROW_PX[tileSize]}
              scrollTestId="album-detail-grid-scroll"
            />
          )}
        </div>

        {/* 底部加载指示（无限滚动补页中） */}
        {loadingMore && (
          <div
            className="flex h-7 shrink-0 items-center justify-center text-[11px] text-text-muted"
            data-testid="album-detail-loading-more"
          >
            {t("gallery.loadingMore")}
          </div>
        )}
      </div>

      {/* 多选浮动操作条（相册上下文：多一项「从相册移除」） */}
      {selecting && (
        <SelectionBar
          count={selectedAssets.length}
          assets={selectedAssets}
          onDone={exitSelection}
          onAddToAlbum={(targets) => setAddToAlbumTargets(targets)}
          album={{
            albumId,
            albumName,
            onRemoved: handleAssetsRemoved,
          }}
          subgroup={{
            current: subgroup,
            names: subgroups.map((g) => g.name),
            onMove: handleSubgroupMove,
          }}
        />
      )}

      {/* 瓦片右键菜单（相册上下文：多一项「从相册移除」） */}
      {ctxMenu && (
        <AssetContextMenu
          at={{ x: ctxMenu.x, y: ctxMenu.y }}
          assets={ctxMenu.assets}
          onClose={() => setCtxMenu(null)}
          testId="album-asset-context-menu"
          onAddToAlbum={(targets) => setAddToAlbumTargets(targets)}
          albumContext={{ albumId, onRemoved: handleAssetsRemoved }}
        />
      )}

      {/* 导入成片弹窗（B4：目标=本相册+子分组，默认「成片」） */}
      {derivedImportOpen && (
        <ImportDerivedDialog
          albumId={albumId}
          albumName={albumName}
          subgroups={subgroups.map((g) => g.name)}
          onClose={() => setDerivedImportOpen(false)}
          onImported={() => {
            setReloadToken((token) => token + 1);
            void refreshMeta();
            void refreshSubgroups();
          }}
        />
      )}

      {/* 加入（其他）相册弹窗 */}
      {addToAlbumTargets !== null && (
        <AddToAlbumDialog
          assets={addToAlbumTargets}
          onClose={() => setAddToAlbumTargets(null)}
        />
      )}

      {/* 全屏查看器 */}
      {viewer && (
        <ViewerOverlay
          asset={viewer.asset}
          group={viewer.group}
          index={viewer.index}
          onNavigate={navigateTo}
          onClose={closeViewer}
        />
      )}
    </div>
  );
}
