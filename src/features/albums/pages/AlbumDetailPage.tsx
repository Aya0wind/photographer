import FilterResultsTransition from "@/shared/components/FilterResultsTransition";
import { usePageSentinel } from "@/features/gallery/lib/usePageSentinel";
import { usePhotoTimeline } from "@/features/gallery/lib/usePhotoTimeline";
import { useAssetSelection } from "@/features/gallery/lib/useAssetSelection";
import { usePhotoCards } from "@/features/gallery/lib/usePhotoCards";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { useTranslation } from "react-i18next";

import {
  albumAssetsPage,
  albumItemMoveSubgroup,
  albumList,
  albumRename,
  albumSubgroups,
  assetTrashMove,
  cullSessionCreate,
  type AlbumDto,
  type AlbumSubgroupDto,
  type AssetDto,
  type AssetFilters,
} from "@/ipc/api";
import { useCullingStore } from "@/features/culling/cullingStore";
import { groupAssetsByDate } from "@/features/gallery/lib/assetGroups";
import { useAssetViewer } from "@/features/gallery/lib/useAssetViewer";
import AssetGrid from "@/features/gallery/components/AssetGrid";
import { AssetContextMenu } from "@/features/gallery/components/ContextMenu";
import ActionPopover from "@/shared/components/ActionPopover";
import FloatingToolbar from "@/shared/components/FloatingToolbar";
import SelectionDock from "@/shared/components/SelectionDock";
import TileSizeSwitch from "@/features/gallery/components/TileSizeSwitch";
import ViewerOverlay from "@/features/gallery/components/ViewerOverlay";
import AddToAlbumDialog from "@/features/albums/components/AddToAlbumDialog";
import { subgroupSuggestions } from "@/features/albums/lib/ungroupedAlbum";
import ErrorModal from "@/shared/components/ErrorModal";
import {
  FilterChipsRow,
  ClearFiltersButton,
  FilterPanel,
  buildChips,
  buildFilters,
  hasActiveFilters,
  parseInputs,
  serializeInputs,
  EMPTY_INPUTS,
} from "@/features/gallery/FilterPanel";
import { useDebouncedValue } from "@/lib/useDebouncedValue";
import { subscribeAppEvents } from "@/ipc/api";
import TetherStartDialog from "@/features/tethering/TetherStartDialog";
import ExportAlbumDialog from "@/features/albums/components/ExportAlbumDialog";
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
  const loadedOnce=useRef(false);
  const resultKey=useRef<string|null>(null);
  const [resultsRevision,setResultsRevision]=useState(0);


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
    const seq = loadSeqRef.current;
    const page = await fetchPage(afterId, appliedKeyRef.current);
    // 补页不参与主加载的 seq 竞争（bump 会丢弃在途主加载且 status 卡 loading）：
    // 若 await 期间发生主加载重置（loadingRef 已被主管线清零），本页丢弃由新管线接管
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

  useEffect(() => {
    let cancelled = false;
    const seq = ++loadSeqRef.current;
    setStatus("loading");
    hasMoreRef.current = true;
    loadingRef.current = true;
    void fetchPage(0, debouncedKey).then((page) => {
      if (cancelled || seq !== loadSeqRef.current) return;
      assetsRef.current = page;
      setAssets(page);
      if (page.length < PAGE_LIMIT) hasMoreRef.current = false;
      loadingRef.current = false;
      setLoadingMore(false);
      if(resultKey.current!==null&&resultKey.current!==debouncedKey)setResultsRevision(n=>n+1);
      resultKey.current=debouncedKey;
      loadedOnce.current=true;
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

  const sentinelRef = usePageSentinel(status === "ready", assets.length, appendPage);

  const timelineCurrentAssets = useCallback(() => assetsRef.current, []);
  const beforeTimelineJump = useCallback(() => {
    ++loadSeqRef.current;
    loadingRef.current = true;
    setLoadingMore(false);
  }, []);
  const finishTimelineJump = useCallback(() => { loadingRef.current = false; }, []);
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
    enabled: status === "ready",
    scopeKey: `${albumId}:${subgroup ?? "root"}:${debouncedKey}:${reloadToken}`,
    filters: { ...buildFilters(parseInputs(debouncedKey)), albumId, ...(subgroup === null ? { subgroupIsNull: true } : { subgroup }) },
    currentAssets: timelineCurrentAssets,
    onBeforeJump: beforeTimelineJump,
    onJumpFinished: finishTimelineJump,
    onReplace: replaceTimelineAssets,
    onPrepend: prependTimelineAssets,
  });

  // --- 多选（与画廊同语义：check 圆钮 / Ctrl+点击 / 长按；Esc 退出） -------------------
  const { selecting, selected, setSelected, toggleSelected, ctrlSelect, exitSelection, contextTargets } = useAssetSelection();


  function enterSubgroup(name: string): void {
    exitSelection();
    setSubgroup(name);
  }
  function backToRoot(): void {
    exitSelection();
    setSubgroup(null);
  }

  // --- 分组 + 查看器 -------------------------------------------------------------------
  const { cards, badges } = usePhotoCards(assets);
  const groups = useMemo(() => groupAssetsByDate(cards), [cards]);
  const { viewer, openAsset, closeViewer, navigateTo, selectVersion } = useAssetViewer(groups, assets);
  useEffect(() => {
    if (!viewer || !hasMoreRef.current) return;
    if (viewer.index >= viewer.group.assets.length - 8) void appendPage();
  }, [viewer?.index, viewer?.group.assets.length, appendPage]);
  const [tileSize, setTileSize] = useGalleryTileSize();
  const [toolsOpen, setToolsOpen] = useState(false);

  const loadedById = useMemo(() => {
    const map = new Map<number, AssetDto>();
    for (const group of groups) for (const a of group.assets) map.set(a.id, a);
    return map;
  }, [groups]);
  const patchAlbumAssets = useCallback((items: AssetDto[], patch: Partial<AssetDto>) => {
    const ids = new Set(items.map((item) => item.id));
    assetsRef.current = assetsRef.current.map((asset) => ids.has(asset.id) ? { ...asset, ...patch } : asset);
    setAssets(assetsRef.current);
  }, []);

  // --- 瓦片右键菜单（相册上下文：多一项「从相册移除」）+ 加入相册弹窗 -------------------
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number; assets: AssetDto[] } | null>(null);
  const [addToAlbumTargets, setAddToAlbumTargets] = useState<AssetDto[] | null>(null);
  const [moveTargets, setMoveTargets] = useState<AssetDto[] | null>(null);
  const [moveName, setMoveName] = useState("");
  const [moving, setMoving] = useState(false);
  const [moveError, setMoveError] = useState<string | null>(null);

  function handleTileContextMenu(asset: AssetDto, at: { x: number; y: number }): void {
    setCtxMenu({ ...at, assets: contextTargets(asset, loadedById) });
  }

  /** 列表刷新（移入回收站后）：重置分页重拉 + 相册计数 + 子分组清单 */
  const handleAssetsRemoved = useCallback(() => {
    exitSelection();
    setReloadToken((token) => token + 1);
    void refreshMeta();
  }, [exitSelection, refreshMeta]);

  /** 「移入回收站」确认目标（一照一册模型：从相册移除 = 移入回收站，
   * 真删只发生在回收站页；还原时原册仍在则回原册）。null=弹窗关闭 */
  const [trashConfirm, setTrashConfirm] = useState<{ ids: number[] } | null>(null);
  const requestTrashMove = useCallback((targets: AssetDto[]) => {
    setTrashConfirm({ ids: targets.map((a) => a.id) });
  }, []);
  async function confirmTrashMove(): Promise<void> {
    if (trashConfirm === null) return;
    const ids = trashConfirm.ids;
    setTrashConfirm(null);
    await assetTrashMove(ids);
    handleAssetsRemoved();
  }

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
      } else {
        setMoveError(t("albums.subgroupMoveFailed"));
      }
      return ok;
    },
    [albumId, exitSelection, refreshMeta, refreshSubgroups, t],
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

  // --- 「联机拍摄」入口（阶段 E）：弹窗选相机，后端开独立拍摄窗口 -----------------------
  const [tetherOpen, setTetherOpen] = useState(false);

  // --- 「导出为文件夹」入口（M6）：当前作用域（根/子组）整册导出 -----------------------
  const [exportOpen, setExportOpen] = useState(false);
  // 子组作用域成员数从子分组清单取（根作用域直接用相册计数）
  const exportCount =
    subgroup === null
      ? itemCount
      : subgroups.find((group) => group.name === subgroup)?.itemCount;

  // 联拍新片入册（独立拍摄窗口触发）→ 本相册列表与计数即时刷新
  useEffect(() => {
    let off: (() => void) | null = null;
    let cancelled = false;
    void subscribeAppEvents((event) => {
      if (event.type !== "tetheringPhotoAdded" || event.albumId !== albumId) return;
      setReloadToken((v) => v + 1);
      void refreshMeta();
    })
      .then((fn) => {
        if (cancelled) fn();
        else off = fn;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      off?.();
    };
  }, [albumId, refreshMeta]);

  // --- 「选片」入口（Culling V1）：当前作用域（根/子组）一键开会话 → /culling 续选 ---
  const [cullBusy, setCullBusy] = useState(false);
  async function startCulling(): Promise<void> {
    if (cullBusy || itemCount === 0) return;
    setCullBusy(true);
    const result = await cullSessionCreate({
      kind: "album",
      albumId,
      subgroup,
    });
    setCullBusy(false);
    if (!result.ok) return; // 后端未就绪：静默降级（按钮可重试）
    void useCullingStore.getState().refreshActiveCount();
    navigate("/culling", { state: { open: result.session.id } });
  }

  return (
    <div className="relative h-full" data-testid="album-detail-page" data-album-id={albumId}>
      <div className="flex h-full w-full flex-col px-4">
        {/* 页头：相册名（点击重命名）+ 张数 + 添加照片 + 筛选 + 尺寸 */}
        <div className="flex h-14 shrink-0 items-center gap-2.5 pr-64" data-testid="album-detail-toolbar">
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


        </div>
        <FloatingToolbar label={t("ui.browseActions")} locked={panelOpen || toolsOpen || addHintOpen || renaming} inactive={viewer !== null}>
          {/* 筛选按钮（激活条件计数徽标） */}
          <button
            type="button"
            onClick={() => setPanelOpen((v) => !v)}
            aria-expanded={panelOpen}
            className="ui-icon-button relative"
            aria-label={t("search.filter")} title={t("search.filter")}
            data-testid="album-detail-filter-toggle"
          >
            <svg viewBox="0 0 20 20" width="17" height="17" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="M3 4h14l-5.5 6v5l-3 1v-6z" /></svg>
            {chips.length > 0 && (
              <span className="rounded-full bg-accent px-1.5 text-[10px] font-bold leading-4 text-black">
                {chips.length}
              </span>
            )}
          </button>

          <TileSizeSwitch value={tileSize} onChange={setTileSize} />
          <ActionPopover label={t("ui.more")} onOpenChange={setToolsOpen} panelClassName="items-start">
          {/* 导出为文件夹（M6，Photo Hub → LR）：当前作用域（根/子组）整册导出 */}
          <button
            type="button"
            onClick={() => setExportOpen(true)}
            title={subgroup === null ? t("albums.export.hint") : t("albums.export.hintSubgroup", { subgroup })}
            className="flex h-8 shrink-0 items-center justify-center gap-2 rounded-md border border-edge px-3 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
            data-testid="album-export-start"
          >
            {t("albums.export.action")}
          </button>
          {/* 联机拍摄（阶段 E）：先选相册再开拍——独立窗口内调参/取景/按快门，新片直接入本相册 */}
          <button
            type="button"
            onClick={() => setTetherOpen(true)}
            title={t("albums.tetherHint")}
            className="flex h-8 shrink-0 items-center justify-center gap-2 rounded-md border border-edge px-3 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
            data-testid="album-tether-start"
          >
            {t("albums.tether")}
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
                className="ui-glass ui-popover absolute right-0 top-8 z-40 w-64 rounded-xl border border-edge bg-surface p-3 shadow-2xl shadow-black/40"
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

          </ActionPopover>
          {/* 选片（Culling V1）：当前根/子组作用域一键开会话（空相册禁用） */}
          <button
            type="button"
            onClick={() => void startCulling()}
            disabled={cullBusy || itemCount === 0}
            title={subgroup === null ? t("albums.cullHint") : t("albums.cullHintSubgroup", { subgroup })}
            className="flex h-8 shrink-0 items-center justify-center gap-2 rounded-md bg-accent px-3 text-xs font-semibold text-black shadow-sm transition-colors hover:brightness-110 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="album-cull-start"
            data-busy={cullBusy}
          >
            {cullBusy ? t("albums.cullBusy") : t("albums.cull")}
          </button>

        </FloatingToolbar>

        {/* 筛选面板（相册维度隐藏：本页已在相册上下文内） */}
        {panelOpen && <div className="ui-glass ui-popover absolute inset-x-4 top-14 z-20 max-h-[calc(100%_-_80px)] overflow-y-auto rounded-2xl border border-edge p-4">
          <div className="mb-2 flex items-center gap-3 text-xs text-text-secondary"><span>{t("search.filter")}</span><ClearFiltersButton disabled={!hasActiveFilters(inputs)} onClear={()=>setInputs(EMPTY_INPUTS)}/><span className="flex-1"/><button type="button" className="ui-icon-button" aria-label={t("ui.close")} onClick={() => setPanelOpen(false)}>×</button></div>
          <FilterPanel inputs={inputs} onPatch={patchInputs} hideAlbum />
        </div>}

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
          <FilterResultsTransition busy={status==="loading"||rawKey!==debouncedKey} revision={resultsRevision}>
          {status === "loading" && !loadedOnce.current ? (
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
              timelineDates={timeline.dates}
              onTimelineJump={timeline.jump}
              onNearTop={timeline.prepend}
              badges={badges}
              groups={groups}
              onOpenAsset={openAsset}
              onCtrlClick={ctrlSelect}
              onLongPress={ctrlSelect}
              onCheckClick={ctrlSelect}
              onAssetContextMenu={handleTileContextMenu}
              selection={selecting ? { active: true, selected, onToggle: toggleSelected } : undefined}
              sentinelRef={sentinelRef}
              loadingMore={loadingMore}
              hasMore={hasMoreRef.current}
              loadingTestId="album-detail-loading-more"
              layout="justify"
              tile={GALLERY_JUSTIFY_ROW_PX[tileSize]}
              scrollTestId="album-detail-grid-scroll"
            />
          )}
          </FilterResultsTransition>
        </div>

      </div>

      {selecting && <SelectionDock count={selected.length} onClear={exitSelection}
        onMove={() => setAddToAlbumTargets(Array.from(selected).flatMap((id) => { const item = loadedById.get(id); return item ? [item] : []; }))}
        onMore={(event) => { const rect = event.currentTarget.getBoundingClientRect(); setCtxMenu({ x: rect.left, y: rect.top, assets: Array.from(selected).flatMap((id) => { const item = loadedById.get(id); return item ? [item] : []; }) }); }} />}

      {/* 瓦片右键菜单 */}
      {ctxMenu && (
        <AssetContextMenu
          at={{ x: ctxMenu.x, y: ctxMenu.y }}
          assets={ctxMenu.assets}
          onClose={() => setCtxMenu(null)}
          testId="album-asset-context-menu"
          onAddToAlbum={(targets) => setAddToAlbumTargets(targets)}
          onTrashRequest={requestTrashMove}
          onFavoritesChanged={(items, favorite) => patchAlbumAssets(items, { rating: favorite ? 5 : 0 })}
          onFlagged={(items, flagged) => patchAlbumAssets(items, { flagged })}
          onColorLabeled={(items, label) => patchAlbumAssets(items, { colorLabel: label })}
          onRejected={(items, rejected) => patchAlbumAssets(items, { rejected })}
          windowIds={assets.map((asset) => asset.id)}
          selectedIds={selected}
          onSelectIds={setSelected}
          extraEntries={[{ key: "move-subgroup", label: t("albums.moveToSubgroup"), onSelect: () => { setMoveTargets(ctxMenu.assets); setMoveName(""); } }, ...(subgroup !== null ? [{ key: "move-root", label: t("albums.moveToRoot"), onSelect: () => { void handleSubgroupMove(null, ctxMenu.assets); } }] : [])]}
        />
      )}

      {/* 「移入回收站」确认一步（多选操作条/右键菜单共用） */}
      <ErrorModal message={moveError} onClose={() => setMoveError(null)} />
      {moveTargets && <div className="fixed inset-0 z-[80] flex items-center justify-center bg-black/55 p-6" role="dialog" aria-modal="true" aria-label={t("albums.moveToSubgroup")} data-testid="album-move-subgroup-dialog">
        <div className="w-full max-w-sm rounded-xl border border-edge bg-surface p-4 shadow-2xl">
          <h2 className="text-sm font-semibold text-text-primary">{t("albums.moveToSubgroup")}</h2>
          <input autoFocus value={moveName} onChange={(event) => setMoveName(event.target.value)} list="album-move-subgroups" placeholder={t("albums.subgroupInputPlaceholder")} aria-label={t("albums.moveToSubgroup")} data-testid="album-move-subgroup-name" className="mt-3 w-full rounded-md border border-edge bg-bg px-3 py-2 text-sm text-text-primary outline-none focus:border-accent" />
          <datalist id="album-move-subgroups">{subgroupSuggestions(subgroups.map((item) => item.name)).filter((name) => name !== subgroup).map((name) => <option key={name} value={name} />)}</datalist>
          <div className="mt-4 flex justify-end gap-2">
            <button type="button" disabled={moving} onClick={() => setMoveTargets(null)} className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary">{t("common.cancel")}</button>
            <button type="button" data-testid="album-move-subgroup-confirm" disabled={moving || !moveName.trim()} onClick={async () => { setMoving(true); try { if (await handleSubgroupMove(moveName.trim(), moveTargets)) setMoveTargets(null); } finally { setMoving(false); } }} className="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-black disabled:opacity-40">{t("albums.subgroupMoveConfirm", { name: moveName.trim() || "…" })}</button>
          </div>
        </div>
      </div>}
      {trashConfirm !== null && (
        <div
          className="fixed inset-0 z-40 flex items-center justify-center bg-black/55 p-6"
          data-testid="trash-move-dialog"
          role="dialog"
          aria-modal="true"
          aria-label={t("trash.moveTitle")}
          onClick={(e) => {
            if (e.target === e.currentTarget) setTrashConfirm(null);
          }}
        >
          <div className="w-full max-w-sm rounded-xl border border-edge bg-surface p-4 shadow-2xl">
            <h2 className="text-sm font-semibold text-text-primary" data-testid="trash-move-title">
              {t("trash.moveTitle")}
            </h2>
            <p className="mt-2 text-xs leading-relaxed text-text-secondary">
              {t("trash.moveDesc", { count: trashConfirm.ids.length })}
            </p>
            <div className="mt-4 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setTrashConfirm(null)}
                className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:bg-panel hover:text-text-primary"
                data-testid="trash-move-cancel"
              >
                {t("common.cancel")}
              </button>
              <button
                type="button"
                onClick={() => void confirmTrashMove()}
                className="rounded-md bg-red-500 px-3 py-1.5 text-xs font-medium text-white transition-colors hover:bg-red-500/85"
                data-testid="trash-move-accept"
              >
                {t("trash.moveConfirm")}
              </button>
            </div>
          </div>
        </div>
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
          onAssetPatched={(id, patch) => {
            assetsRef.current = assetsRef.current.map((photo) => photo.id === id ? { ...photo, ...patch } : photo);
            setAssets(assetsRef.current);
          }}
          onVersionSelect={selectVersion}
          onNavigate={navigateTo}
          onClose={closeViewer}
        />
      )}
      {tetherOpen && (
        <TetherStartDialog
          albumId={albumId}
          albumName={albumName}
          onClose={() => setTetherOpen(false)}
        />
      )}

      {/* 导出为文件夹（M6）：导出当前作用域（根=整册 / 子组视图=该子组） */}
      {exportOpen && (
        <ExportAlbumDialog
          albumId={albumId}
          albumName={albumName}
          subgroup={subgroup}
          itemCount={exportCount}
          onClose={() => setExportOpen(false)}
        />
      )}
    </div>
  );
}
