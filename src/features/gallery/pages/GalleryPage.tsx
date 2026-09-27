import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion } from "motion/react";

import {
  assetsByIds,
  assetsCount,
  assetsPage,
  assetTrashMove,
  isIpcAvailable,
  smartViewDelete,
  smartViewList,
  type AssetDto,
  type AssetFilters,
  type SmartViewDto,
} from "@/ipc/api";
import { useSettingsStore } from "@/stores/settingsStore";
import {
  groupAssetsByDate,
  formatDateLabel,
} from "../lib/assetGroups";
import { mergeRawJpgCards } from "../lib/mergeRawJpg";
import { collapseBursts } from "../lib/burstStacks";
import {
  gallerySnapshot,
  saveGallerySnapshot,
} from "../lib/galleryCache";
import {
  GALLERY_JUSTIFY_ROW_PX,
  useGalleryTileSize,
} from "../lib/useGalleryTileSize";
import { useAssetViewer } from "../lib/useAssetViewer";
import { useSemanticSearch, useSemanticGate, useAiIndexingProgress } from "@/features/ai/useSemanticSearch";
import { recordSemanticQuery } from "@/features/ai/semanticHistory";
import {
  SemanticGateNotice,
  SemanticIndexingBanner,
} from "@/features/ai/SemanticResultsView";
import { AssetContextMenu } from "../components/ContextMenu";
import AssetGrid, { type AssetGridHandle, type ViewportInfo } from "../components/AssetGrid";
import LrStagingDialog from "../components/LrStagingDialog";
import SelectionBar from "../components/SelectionBar";
import SmartViewsMenu from "../components/SmartViewsMenu";
import TileSizeSwitch from "../components/TileSizeSwitch";
import ShortcutsHint from "../components/ShortcutsHint";
import ViewerOverlay from "../components/ViewerOverlay";
import AddToAlbumDialog from "@/features/albums/components/AddToAlbumDialog";
import {
  FilterChipsRow,
  FilterPanel,
  QuickFilterBar,
  buildChips,
  buildFilters,
  hasActiveFilters,
  inputsFromFilters,
  parseInputs,
  serializeInputs,
  EMPTY_INPUTS,
} from "../FilterPanel";
import { useDebouncedValue } from "@/lib/useDebouncedValue";
import { motionInitial, useMotionOn } from "@/lib/motion";

/**
 * 画廊页（M4.5 wave-3：画廊+搜索合并，搜索页已并入）：
 * - 顶部工具条：语义查询输入（全局搜索框的页内版）+「筛选」按钮（展开 FilterPanel，
 *   激活条件计数徽标）+「选择」多选开关 + 计数 + 三档尺寸
 * - 三种数据态：默认（全部资产，keyset 补页 + 会话快照 revalidate）/ 筛选
 *   （assetsPage filters，快照不落盘）/ 语义（searchSemantic 结果同网格带分数角标，
 *   无分页）；修改筛选自动退出语义态
 * - 日期组头折叠（AssetGrid）
 * - 多选（M4.5）：选择按钮 / Ctrl+点击 / 长按进入；浮动操作条（收藏/旗标/分享/取消），
 *   Esc 退出；单选=多选下的 N=1
 * - URL 协议：?mode=semantic&q=…（语义直达）、?kind=photo|raw|video（预置类型）
 * - 快照缓存：仅默认态落盘（筛选/语义态不落）；? 键快捷键速查见 AppShell
 */

const PAGE_LIMIT = 100;
const DEBOUNCE_MS = 300;
/** 组头完全滚出视口后显示吸顶条（低于此偏移视为仍在组头处） */
const STICKY_MIN_SCROLL = 48;

type GalleryMode = "default" | "filters" | "semantic";

export default function GalleryPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const motionOn = useMotionOn();

  // --- 三态：语义（useSemanticSearch）+ 筛选输入（序列化键防抖） ----------------------
  const semantic = useSemanticSearch();
  const [inputs, setInputs] = useState<SearchInputsShim>(EMPTY_INPUTS);
  const patchInputs = useCallback((patch: Partial<SearchInputsShim>) => {
    setInputs((prev) => ({ ...prev, ...patch }));
  }, []);
  const [panelOpen, setPanelOpen] = useState(false);

  const rawKey = serializeInputs(inputs);
  const debouncedKey = useDebouncedValue(rawKey, DEBOUNCE_MS);
  const appliedFilters = useMemo(() => buildFilters(parseInputs(debouncedKey)), [debouncedKey]);
  const filtersActive = useMemo(() => hasActiveFilters(parseInputs(debouncedKey)), [debouncedKey]);
  const appliedKeyRef = useRef(debouncedKey);
  appliedKeyRef.current = debouncedKey;

  const semanticMode = semantic.status !== "idle";
  const mode: GalleryMode = semanticMode ? "semantic" : filtersActive ? "filters" : "default";
  const modeRef = useRef(mode);
  modeRef.current = mode;

  // 语义搜索前置门禁 + 索引建立进度（语义态顶部细提示条数据源）
  const semanticGate = useSemanticGate();
  const semanticIndexing = useAiIndexingProgress();
  /** 被门禁拦截过：工具条下方行内提示（成功放行或改走筛选即消） */
  const [semanticGateNotice, setSemanticGateNotice] = useState(false);
  /** 当前语义查询词（状态头展示；本地语义输入框已删，入口唯一=全局搜索框） */
  const [semanticQuery, setSemanticQuery] = useState("");

  function runSemantic(query: string): void {
    if (semanticGate.blocked) {
      // 门禁拦截：不发查询（URL 自动执行共用此口）
      setSemanticGateNotice(true);
      return;
    }
    setSemanticGateNotice(false);
    setSemanticQuery(query);
    recordSemanticQuery(query); // 语义历史（最近 5 条，localStorage）
    void semantic.run(query);
  }

  /** 退出语义态：重置结果 + 撤 URL 参数（全局框随 URL 清空回显），回默认画廊 */
  function exitSemanticMode(): void {
    setSemanticGateNotice(false);
    semantic.reset();
    appliedUrlQueryRef.current = null; // 允许同词稍后再次进入
    if (searchParams.get("mode") === "semantic") {
      navigate("/gallery", { replace: true });
    }
  }

  /** 修改筛选 = 退出语义态回筛选/默认（语义结果与条件筛选互斥）；门禁提示一并撤下 */
  function patchFilters(patch: Partial<SearchInputsShim>): void {
    setSemanticGateNotice(false);
    semantic.reset();
    patchInputs(patch);
  }

  const chips = useMemo(() => buildChips(inputs, t), [inputs, t]);
  const advancedChipCount = chips.filter((chip) => !["favorite", "kind", "orientation", "gps"].includes(chip.key)).length;

  // --- URL 协议（全局搜索框 / 类型筛选直达）：?mode=semantic&q= / ?kind= ----------------
  const urlQuery = searchParams.get("q") ?? "";
  const appliedUrlQueryRef = useRef<string | null>(null);
  const appliedUrlKindRef = useRef<string | null>(null);
  useEffect(() => {
    const urlMode = searchParams.get("mode");
    if (urlMode === "semantic" && urlQuery !== "" && appliedUrlQueryRef.current !== urlQuery) {
      appliedUrlQueryRef.current = urlQuery;
      runSemantic(urlQuery);
    } else if (
      (urlMode !== "semantic" || urlQuery === "") &&
      appliedUrlQueryRef.current !== null
    ) {
      // 语义参数撤离（全局框清空回车/退出按钮）：同步退出语义态
      appliedUrlQueryRef.current = null;
      semantic.reset();
    }
    const urlKind = searchParams.get("kind");
    if (urlKind === "photo" || urlKind === "raw" || urlKind === "video") {
      if (appliedUrlKindRef.current !== urlKind) {
        appliedUrlKindRef.current = urlKind;
        setInputs((prev) => (prev.kind === urlKind ? prev : { ...prev, kind: urlKind }));
      }
    } else if (urlKind === null && appliedUrlKindRef.current !== null) {
      // 参数消失（同路径无参导航，如侧栏图库链接）：同步撤筛选——
      // 此前只清 ref 不清 state，?kind 进入的筛选会永久滞留且 UI 无从察觉。
      // 清回规范空值 "all"（chips/筛选面板以 !== "all" 判定；置 undefined 会
      // 产生幽灵 chip 且类型不合法）
      appliedUrlKindRef.current = null;
      setInputs((prev) => (prev.kind === "all" ? prev : { ...prev, kind: "all" }));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchParams, urlQuery]);

  // --- 数据管线（默认/筛选共用 keyset；快照仅默认态） --------------------------------
  const cachedAtMount = gallerySnapshot();
  const [assets, setAssets] = useState<AssetDto[]>(() => cachedAtMount?.assets ?? []);
  const [totalCount, setTotalCount] = useState<number | null>(null);
  /** 已加载资产（与 state 同步维护，供补页循环同步读取） */
  const assetsRef = useRef<AssetDto[]>(cachedAtMount?.assets ?? []);
  const [status, setStatus] = useState<"loading" | "ready" | "degraded">(() =>
    cachedAtMount && cachedAtMount.assets.length > 0 ? "ready" : "loading",
  );
  const hasMoreRef = useRef(cachedAtMount?.hasMore ?? true);
  const loadingRef = useRef(false);
  const loadSeqRef = useRef(0);
  const [loadingMore, setLoadingMore] = useState(false);

  useEffect(() => {
    if (semanticMode) return;
    let cancelled = false;
    setTotalCount(null);
    void assetsCount(filtersActive ? appliedFilters : undefined).then((count) => {
      if (!cancelled) setTotalCount(count);
    });
    return () => { cancelled = true; };
  }, [appliedFilters, filtersActive, semanticMode]);

  /** 视口滚动位置（快照保存用；重挂载恢复） */
  const scrollTopRef = useRef(cachedAtMount?.scrollTop ?? 0);
  const gridRef = useRef<AssetGridHandle | null>(null);
  const sentinelRef = useRef<HTMLDivElement | null>(null);
  const [viewport, setViewport] = useState<ViewportInfo>({
    scrollTop: scrollTopRef.current,
    group: null,
  });

  /** 快照落盘（仅默认态：筛选/语义态不落，避免脏缓存）；滚动位置由视口回调实时维护 */
  const persistSnapshot = useCallback(() => {
    if (modeRef.current !== "default") return;
    saveGallerySnapshot({
      assets: assetsRef.current,
      dates: [],
      hasMore: hasMoreRef.current,
      scrollTop: scrollTopRef.current,
      savedAt: Date.now(),
    });
  }, []);

  /** 追加下一页（keyset）；返回本页结果供补页循环判断 */
  const appendPage = useCallback(async (): Promise<AssetDto[]> => {
    if (loadingRef.current || !hasMoreRef.current) return [];
    loadingRef.current = true;
    setLoadingMore(true);
    const seq = ++loadSeqRef.current;
    const afterId = assetsRef.current.length > 0
      ? assetsRef.current[assetsRef.current.length - 1].id
      : 0;
    const key = appliedKeyRef.current;
    const page = hasActiveFilters(parseInputs(key))
      ? await assetsPage(afterId, PAGE_LIMIT, buildFilters(parseInputs(key)))
      : await assetsPage(afterId, PAGE_LIMIT);
    if (seq !== loadSeqRef.current) return page; // 已有更新的加载，丢弃本页渲染（数据保留待下轮）
    if (page.length > 0) {
      const seen = new Set(assetsRef.current.map((a) => a.id));
      const fresh = page.filter((a) => !seen.has(a.id));
      assetsRef.current = [...assetsRef.current, ...fresh];
      setAssets(assetsRef.current);
    }
    if (page.length < PAGE_LIMIT) hasMoreRef.current = false;
    loadingRef.current = false;
    setLoadingMore(false);
    persistSnapshot();
    return page;
  }, [persistSnapshot]);

  /** 首屏/条件变化（防抖后）：默认态有会话快照则静默 revalidate；筛选态常规重置 */
  useEffect(() => {
    if (semanticMode) return; // 语义结果不走此管线
    let cancelled = false;
    const seq = ++loadSeqRef.current;
    // 默认态已有快照数据 → 静默 revalidate（不闪骨架）；筛选态/空库常规 loading
    if (filtersActive || assets.length === 0) setStatus("loading");
    hasMoreRef.current = true;
    loadingRef.current = false;
    const fetchFirstPage = filtersActive
      ? () => assetsPage(0, PAGE_LIMIT, appliedFilters)
      : () => assetsPage(0, PAGE_LIMIT);
    void fetchFirstPage().then((page) => {
      if (cancelled || seq !== loadSeqRef.current) return;
      if (filtersActive) {
        assetsRef.current = page;
        setAssets(page);
      } else {
        // 默认态 revalidate：首页与缓存前缀一致（库未变）保留已加载全量
        const cached = gallerySnapshot();
        const samePrefix =
          cached !== null &&
          cached.assets.length >= page.length &&
          page.every((a, i) => cached.assets[i].id === a.id);
        assetsRef.current = samePrefix ? cached.assets : page;
        setAssets(assetsRef.current);
        if (samePrefix) hasMoreRef.current = cached.hasMore;
      }
      if (page.length < PAGE_LIMIT) hasMoreRef.current = false;
      setStatus(page.length === 0 && !isIpcAvailable() ? "degraded" : "ready");
      persistSnapshot();
    });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [appliedFilters, debouncedKey, semanticMode, filtersActive]);

  // 无限滚动：哨兵进入视口（提前 800px 预载）——默认/筛选态
  useEffect(() => {
    const el = sentinelRef.current;
    if (!el || status !== "ready" || semanticMode || typeof IntersectionObserver === "undefined") return;
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) void appendPage();
      },
      { rootMargin: "800px 0px" },
    );
    io.observe(el);
    return () => io.disconnect();
  }, [status, appendPage, assets.length, semanticMode]);

  // 重挂载滚动恢复（仅默认态）：快照有位置且首次 ready 后立即还原
  const scrollRestoredRef = useRef(false);
  useEffect(() => {
    if (scrollRestoredRef.current || status !== "ready" || mode !== "default") return;
    const snap = gallerySnapshot();
    if (snap && snap.scrollTop > 0) {
      scrollRestoredRef.current = true;
      gridRef.current?.restoreScroll(snap.scrollTop);
    }
  }, [status, mode, assets.length]);

  /** 视口回调：上报吸顶/日期之外，滚动位置实时记入快照（切页保留） */
  const handleViewportChange = useCallback((info: ViewportInfo) => {
    scrollTopRef.current = info.scrollTop;
    setViewport(info);
  }, []);

  // --- 多选（M4.5 选择模式） ----------------------------------------------------------
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

  // Esc 退出选择模式（查看器打开时不抢：选择模式下瓦片点击不打开查看器）
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

  // --- 展示分组与查看器 ---------------------------------------------------------------
  const mergeEnabled = useSettingsStore((s) => s.settings.gallery?.mergeRawJpg ?? true);
  const { cards, badges } = useMemo(
    () => mergeRawJpgCards(assets, mergeEnabled),
    [assets, mergeEnabled],
  );
  const groups = useMemo(() => groupAssetsByDate(cards), [cards]);
  const semanticGroups = useMemo(() => groupAssetsByDate(semantic.assets), [semantic.assets]);

  // 连拍堆叠折叠（M6）：仅画廊资产管线（语义结果不折叠）；查看器/胶片条仍用
  // 未折叠全量组（点击堆叠卡打开封面，胶片条天然顺序翻完整组）。折叠在
  // RAW+JPG 合并之后——判定用合并后代表资产的 burstId。
  const { displayGroups, burstBadges } = useMemo(() => {
    if (semanticMode) return { displayGroups: semanticGroups, burstBadges: undefined };
    const badges = new Map<number, number>();
    const display = groups.map((group) => {
      const collapsed = collapseBursts(group.assets);
      for (const [coverId, count] of collapsed.badges) badges.set(coverId, count);
      return collapsed.totalCount === collapsed.assets.length
        ? group
        : { ...group, assets: collapsed.assets, totalCount: collapsed.totalCount };
    });
    return { displayGroups: display, burstBadges: badges.size > 0 ? badges : undefined };
  }, [groups, semanticGroups, semanticMode]);

  // 查看器/选中查找用全量组；网格/视口/吸顶用折叠展示组
  const viewerGroups = semanticMode ? semanticGroups : groups;
  const activeGroups = semanticMode ? semanticGroups : displayGroups;
  const { viewer, openAsset, closeViewer, navigateTo } = useAssetViewer(viewerGroups);
  // 预览靠近已加载末尾时提前补页，让跨日期连续翻页也能越过分页边界。
  useEffect(() => {
    if (semanticMode || !viewer || !hasMoreRef.current) return;
    if (viewer.index >= viewer.group.assets.length - 8) void appendPage();
  }, [semanticMode, viewer?.index, viewer?.group.assets.length, appendPage]);

  // Esc 退出语义态（查看器打开时不抢——查看器自身 Esc 优先；多选退出走独立监听）
  useEffect(() => {
    if (!semanticMode || viewer) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        exitSemanticMode();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [semanticMode, viewer]);

  // --- 瓦片右键菜单（自定义 ContextMenu；原生菜单已被全局 guard 屏蔽） -----------------
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number; assets: AssetDto[] } | null>(
    null,
  );
  // --- 「加入相册」弹窗（多选操作条 / 右键菜单共用入口，③ 全局） -----------------------
  const [addToAlbumTargets, setAddToAlbumTargets] = useState<AssetDto[] | null>(null);
  // --- 「生成 LR 暂存夹」弹窗（B1 追加包：操作条 / 右键菜单共用入口） -------------------
  const [lrTargets, setLrTargets] = useState<AssetDto[] | null>(null);
  const assetsById = useMemo(() => {
    const map = new Map<number, AssetDto>();
    for (const group of viewerGroups) for (const a of group.assets) map.set(a.id, a);
    return map;
  }, [viewerGroups]);

  /** 瓦片右键目标集：非多选=该资产；多选+已选瓦片=全部选中；
   *  多选+未选瓦片=先切换选中集为该图（Windows 语义），再作用于它。 */
  function handleTileContextMenu(asset: AssetDto, at: { x: number; y: number }): void {
    if (selecting) {
      if (selected.includes(asset.id)) {
        const targets = selected
          .map((id) => assetsById.get(id))
          .filter((a): a is AssetDto => a !== undefined);
        setCtxMenu({ ...at, assets: targets });
        return;
      }
      setSelected([asset.id]); // 选中集切换为该图（保持多选态）
    }
    setCtxMenu({ ...at, assets: [asset] });
  }

  /** 选中资产对象（当前态分组内查找；跨态选不中的自动忽略） */
  const loadedById = useMemo(() => {
    const map = new Map<number, AssetDto>();
    for (const group of activeGroups) {
      for (const asset of group.assets) map.set(asset.id, asset);
    }
    return map;
  }, [activeGroups]);
  const selectedAssets = useMemo(
    () => selected.map((id) => loadedById.get(id)).filter((a): a is AssetDto => a !== undefined),
    [selected, loadedById],
  );

  const handleFavoriteChange = useCallback((asset: AssetDto, favorite: boolean) => {
    const favoriteOnly = parseInputs(appliedKeyRef.current).favoriteOnly;
    assetsRef.current = favoriteOnly && !favorite
      ? assetsRef.current.filter((item) => item.id !== asset.id)
      : assetsRef.current.map((item) => item.id === asset.id ? { ...item, rating: favorite ? 5 : 0 } : item);
    setAssets(assetsRef.current);
    if (favoriteOnly && !favorite) setTotalCount((count) => count === null ? null : Math.max(0, count - 1));
    persistSnapshot();
  }, [persistSnapshot]);

  // --- 选片补全（B1）：色标/拒绝/移入回收站/反选/智能视图 --------------------------------
  /** 批量补丁（本地列表乐观更新 + 快照落盘）；色标/拒绝/查看器回传共用 */
  const patchAssetsByIds = useCallback(
    (ids: number[], patchOf: (asset: AssetDto) => Partial<AssetDto>) => {
      if (ids.length === 0) return;
      const idSet = new Set(ids);
      assetsRef.current = assetsRef.current.map((a) => (idSet.has(a.id) ? { ...a, ...patchOf(a) } : a));
      setAssets(assetsRef.current);
      persistSnapshot();
    },
    [persistSnapshot],
  );
  const handleColorLabeled = useCallback(
    (items: AssetDto[], label: string | null) => {
      patchAssetsByIds(items.map((a) => a.id), () => ({ colorLabel: label }));
    },
    [patchAssetsByIds],
  );
  const handleRejected = useCallback(
    (items: AssetDto[], rejected: boolean) => {
      patchAssetsByIds(items.map((a) => a.id), () => ({ rejected }));
    },
    [patchAssetsByIds],
  );
  const handleAssetPatched = useCallback(
    (id: number, patch: Partial<AssetDto>) => {
      patchAssetsByIds([id], () => patch);
    },
    [patchAssetsByIds],
  );

  /** 反选数据窗口：当前数据管线的全部已加载 id（语义态=语义结果；其余=画廊资产） */
  const windowIds = useMemo(
    () => (semanticMode ? semantic.assets.map((a) => a.id) : assets.map((a) => a.id)),
    [semanticMode, semantic.assets, assets],
  );
  const invertSelection = useCallback(
    (ids: number[]) => setSelected(ids.map((id) => id)),
    [],
  );

  /** 版本切换（B2 查看器版本区）：成员在已加载窗口内直接换 asset；不在（如被
   *  groupRole 过滤滤掉）时经 assets_by_ids 取回补进窗口，再走 URL 协议换 asset */
  const handleVersionSelect = useCallback(
    (assetId: number) => {
      const known = viewerGroups.flatMap((g) => g.assets).find((a) => a.id === assetId);
      if (known !== undefined) {
        openAsset(known);
        return;
      }
      void assetsByIds([assetId]).then((list) => {
        const asset = list.find((a) => a.id === assetId);
        if (asset === undefined) return;
        assetsRef.current = [asset, ...assetsRef.current];
        setAssets(assetsRef.current);
        openAsset(asset);
      });
    },
    [openAsset, viewerGroups],
  );

  /** 「移入回收站」确认目标（多选操作条/右键菜单共用；null=弹窗关闭） */
  const [trashConfirm, setTrashConfirm] = useState<{ ids: number[] } | null>(null);
  const requestTrashMove = useCallback((targets: AssetDto[]) => {
    setTrashConfirm({ ids: targets.map((a) => a.id) });
  }, []);
  async function confirmTrashMove(): Promise<void> {
    if (trashConfirm === null) return;
    const ids = trashConfirm.ids;
    setTrashConfirm(null);
    await assetTrashMove(ids); // 后端软删；本地乐观剔除（失败靠重进页面重拉兜底）
    const idSet = new Set(ids);
    assetsRef.current = assetsRef.current.filter((a) => !idSet.has(a.id));
    setAssets(assetsRef.current);
    setTotalCount((count) => (count === null ? null : Math.max(0, count - ids.length)));
    if (selecting) exitSelection();
    persistSnapshot();
  }

  // 智能视图（B1）：清单挂载拉一次 + 面板保存成功后刷新
  const [smartViews, setSmartViews] = useState<SmartViewDto[]>([]);
  const refreshSmartViews = useCallback(() => {
    void smartViewList().then(setSmartViews);
  }, []);
  useEffect(() => {
    refreshSmartViews();
  }, [refreshSmartViews]);
  const applySmartView = useCallback(
    (view: SmartViewDto) => {
      let filters: AssetFilters = {};
      try {
        filters = JSON.parse(view.filtersJson) as AssetFilters;
      } catch {
        filters = {}; // 脏数据兜底：反解失败退回「全部资产」
      }
      setSemanticGateNotice(false);
      semantic.reset();
      setInputs(inputsFromFilters(filters));
    },
    [semantic],
  );
  const deleteSmartView = useCallback(
    async (view: SmartViewDto) => {
      await smartViewDelete(view.id);
      refreshSmartViews();
    },
    [refreshSmartViews],
  );

  // 三档尺寸（justify 行高）
  const [tileSize, setTileSize] = useGalleryTileSize();

  const currentGroup = viewport.group;
  const showSticky =
    currentGroup !== null && viewport.scrollTop > STICKY_MIN_SCROLL && viewer === null;

  // 语义态不阻塞于资产管线（两条管线独立；语义结果有自己的 loading/空态）
  if (status === "loading" && !semanticMode) {
    return (
      <div className="flex h-full flex-col px-3 pt-2" data-testid="gallery-skeleton">
        {[0, 1, 2].map((row) => (
          <div key={row} className="mb-4">
            <div className="mb-2 h-4 w-36 animate-pulse rounded bg-panel/70" />
            <div className="flex gap-1">
              {Array.from({ length: 6 }, (_, i) => (
                <div key={i} className="h-[200px] flex-1 animate-pulse rounded-md bg-panel/50" />
              ))}
            </div>
          </div>
        ))}
      </div>
    );
  }

  if (status === "degraded" && !semanticMode) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 px-8 text-center" data-testid="gallery-degraded">
        <p className="text-sm text-text-secondary">{t("gallery.ipcUnavailable")}</p>
        <button
          type="button"
          onClick={() => {
            setInputs(EMPTY_INPUTS);
            semantic.reset();
            setStatus("loading");
            hasMoreRef.current = true;
            const seq = ++loadSeqRef.current;
            void assetsPage(0, PAGE_LIMIT).then((page) => {
              if (seq !== loadSeqRef.current) return;
              assetsRef.current = page;
              setAssets(page);
              if (page.length < PAGE_LIMIT) hasMoreRef.current = false;
              setStatus(page.length === 0 && !isIpcAvailable() ? "degraded" : "ready");
              persistSnapshot();
            });
          }}
          className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
        >
          {t("gallery.retry")}
        </button>
      </div>
    );
  }

  const gridEmpty = activeGroups.length === 0 || activeGroups.every((g) => g.assets.length === 0);

  return (
    <div className="h-full" data-testid="gallery-page">
      {/* 内容区：水平居中 + 左右对称 padding（修复贴导航边起排/首卡被切） */}
      <div
        className="flex h-full w-full flex-col px-4"
        data-testid="gallery-content"
      >
        {/* 顶部工具条：筛选 + 计数 + 尺寸（语义入口唯一=TitleBar 全局搜索框；
            多选入口=瓦片左上 check 圆钮 / Ctrl+点击 / 长按） */}
        <div className="flex h-11 shrink-0 items-center gap-2.5 border-b border-edge" data-testid="gallery-toolbar">
          {/* 筛选按钮：展开/收起面板；激活条件计数徽标 */}
          <button
            type="button"
            onClick={() => setPanelOpen((v) => !v)}
            aria-expanded={panelOpen}
            className={`flex shrink-0 items-center gap-1.5 rounded-md border px-2.5 py-1 text-[11px] transition-colors ${
              panelOpen || advancedChipCount > 0
                ? "border-accent text-accent"
                : "border-edge text-text-secondary hover:border-text-muted hover:text-text-primary"
            }`}
            data-testid="search-filter-toggle"
          >
            {t("search.moreFilters")}
            {advancedChipCount > 0 && (
              <span
                className="rounded-full bg-accent px-1.5 text-[10px] font-bold leading-4 text-black"
                data-testid="search-filter-count"
              >
                {advancedChipCount}
              </span>
            )}
            <svg
              viewBox="0 0 16 16"
              width="9"
              height="9"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.6"
              strokeLinecap="round"
              strokeLinejoin="round"
              className={`transition-transform ${panelOpen ? "rotate-180" : ""}`}
              aria-hidden="true"
            >
              <path d="M3.5 6l4.5 4.5L12.5 6" />
            </svg>
          </button>

          {/* 已存视图（B1）：点击应用（filters 反解套用）+ 逐项 × 删除确认 */}
          <SmartViewsMenu views={smartViews} onApply={applySmartView} onDelete={(v) => void deleteSmartView(v)} />

          {/* 计数徽标 */}
          <span
            className="ml-auto shrink-0 rounded-full bg-panel px-2 py-0.5 font-mono text-[11px] tabular-nums text-text-secondary"
            data-testid="search-count"
          >
            {semanticMode
              ? t("search.count", { count: semantic.assets.length })
              : t("search.count", { count: totalCount ?? assets.length })}
          </span>

          <div className="shrink-0">
            <TileSizeSwitch value={tileSize} onChange={setTileSize} />
          </div>
        </div>

        <QuickFilterBar inputs={inputs} onPatch={patchFilters} />

        {/* 语义态状态头：查询词 + 结果数 + 退出（本地语义输入框已删，入口唯一=
            TitleBar 全局搜索框；本行保证语义态一眼可识别、可退出） */}
        {semanticMode && (
          <div
            className="flex h-9 shrink-0 items-center gap-3 border-b border-edge/60"
            data-testid="semantic-status-header"
          >
            <span className="min-w-0 truncate text-xs text-text-secondary">
              {semantic.status === "loading" ? (
                t("search.semantic.loadingStatus", { query: semanticQuery })
              ) : (
                t("gallery.semanticStatus", {
                  query: semanticQuery,
                  count: semantic.assets.length,
                })
              )}
            </span>
            <button
              type="button"
              onClick={exitSemanticMode}
              className="shrink-0 rounded-md border border-edge px-2.5 py-1 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
              data-testid="semantic-exit"
            >
              {t("gallery.semanticExit")}
            </button>
          </div>
        )}

        {/* 语义门禁拦截提示（模型未下载/索引未建立）：行内 + 一键跳设置 */}
        {semanticGateNotice && semanticGate.reason !== null && (
          <SemanticGateNotice reason={semanticGate.reason} testId="gallery-semantic-gate" />
        )}

        {/* 筛选面板（默认收起；修改筛选自动退出语义态；保存视图成功后刷新清单） */}
        {panelOpen && <FilterPanel inputs={inputs} onPatch={patchFilters} advancedOnly onSmartViewSaved={refreshSmartViews} />}

        {/* 激活条件 chips */}
        {!semanticMode && (
          <FilterChipsRow
            chips={chips}
            onPatch={(next) => {
              setSemanticGateNotice(false);
              semantic.reset();
              setInputs(next);
            }}
            onClearAll={() => {
              setSemanticGateNotice(false);
              semantic.reset();
              setInputs(EMPTY_INPUTS);
            }}
          />
        )}

        {/* 照片墙（justify 布局）+ 吸顶当前日期 + 年份条 */}
        <div className="relative min-h-0 flex-1">
          {/* 索引建立中：语义态顶部细提示条（完成自动消失；默认/筛选态不弹） */}
          <SemanticIndexingBanner progress={semanticMode ? semanticIndexing : null} />
          {gridEmpty ? (
            semanticMode ? (
              <div className="flex h-full flex-col items-center justify-center gap-2 px-8 text-center" data-testid="semantic-empty">
                <p className="text-sm text-text-secondary">{t("search.semantic.empty")}</p>
                <p className="text-xs text-text-muted">{t("search.semantic.emptyHint")}</p>
              </div>
            ) : filtersActive ? (
              <div className="flex h-full flex-col items-center justify-center gap-2 px-8 text-center" data-testid="search-empty">
                <p className="text-sm text-text-secondary">{t("search.empty")}</p>
                <p className="text-xs text-text-muted">{t("search.emptyHint")}</p>
              </div>
            ) : (
              <div className="flex h-full flex-col items-center justify-center gap-3 px-8 text-center" data-testid="gallery-empty">
                <svg
                  viewBox="0 0 24 24"
                  width="48"
                  height="48"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.2"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  className="text-text-muted"
                  aria-hidden="true"
                >
                  <rect x="3" y="5" width="18" height="14" rx="2" />
                  <path d="M3 15l4.5-4.5 3.5 3.5 3-3L21 17" />
                  <circle cx="8.5" cy="9" r="1.5" />
                </svg>
                <h2 className="text-sm font-semibold text-text-primary">{t("gallery.empty.title")}</h2>
                <p className="max-w-sm text-xs leading-relaxed text-text-muted">{t("gallery.empty.desc")}</p>
                <button
                  type="button"
                  onClick={() => navigate("/import")}
                  className="mt-1 rounded-md bg-accent px-4 py-2 text-sm font-medium text-black transition-colors hover:brightness-110"
                  data-testid="gallery-empty-import"
                >
                  {t("gallery.empty.goImport")}
                </button>
              </div>
            )
          ) : (
            <AssetGrid
              ref={gridRef}
              groups={activeGroups}
              onOpenAsset={openAsset}
              onCtrlClick={ctrlSelect}
              onLongPress={ctrlSelect}
              onCheckClick={ctrlSelect}
              onFavoriteChange={handleFavoriteChange}
              onAssetContextMenu={handleTileContextMenu}
              selection={
                selecting
                  ? { active: true, selected, onToggle: toggleSelected }
                  : undefined
              }
              sentinelRef={semanticMode ? undefined : sentinelRef}
              onViewportChange={handleViewportChange}
              layout="justify"
              tile={GALLERY_JUSTIFY_ROW_PX[tileSize]}
              badges={semanticMode ? undefined : badges}
              scores={semanticMode ? semantic.scores : undefined}
              burstBadges={semanticMode ? undefined : burstBadges}
            />
          )}
          <AnimatePresence initial={false}>
            {showSticky && (
              <motion.div
                key="sticky-date"
                initial={motionInitial(motionOn, { opacity: 0, y: -8 })}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: -8 }}
                transition={{ duration: 0.15, ease: "easeOut" }}
                className="pointer-events-none absolute inset-x-0 top-0 z-10 border-b border-edge/60 bg-bg/85 px-4 py-2 backdrop-blur-sm"
                data-testid="gallery-sticky-date"
              >
                <AnimatePresence mode="wait" initial={false}>
                  <motion.button
                    type="button"
                    onClick={() => gridRef.current?.toggleGroup(currentGroup.key)}
                    key={currentGroup.key}
                    initial={motionInitial(motionOn, { opacity: 0, y: 4 })}
                    animate={{ opacity: 1, y: 0 }}
                    exit={{ opacity: 0, y: -4 }}
                    transition={{ duration: 0.15, ease: "easeOut" }}
                    className="pointer-events-auto cursor-pointer text-[13px] font-semibold text-text-primary hover:text-accent"
                  >
                    {currentGroup.date === null
                      ? t("gallery.unknownDate")
                      : formatDateLabel(currentGroup.date)}
                    <span className="ml-2 text-xs font-normal text-text-muted">
                      {t("gallery.groupCount", { count: currentGroup.assets.length })}
                    </span>
                  </motion.button>
                </AnimatePresence>
              </motion.div>
            )}
          </AnimatePresence>
        </div>

        {/* 底部加载指示（无限滚动补页中） */}
        {loadingMore && !semanticMode && (
          <div className="flex h-7 shrink-0 items-center justify-center text-[11px] text-text-muted" data-testid="gallery-loading-more">
            {t("gallery.loadingMore")}
          </div>
        )}
      </div>

      {/* 多选浮动操作条（已选 N | 收藏/旗标/色标/拒绝/分享/加入相册/反选/移入回收站/取消） */}
      {selecting && (
        <SelectionBar
          count={selectedAssets.length}
          assets={selectedAssets}
          onFavoritesChanged={(items) => items.forEach((asset) => handleFavoriteChange(asset, true))}
          onDone={exitSelection}
          onAddToAlbum={(targets) => setAddToAlbumTargets(targets)}
          onColorLabeled={handleColorLabeled}
          onRejected={handleRejected}
          onTrashRequest={requestTrashMove}
          onLrStaging={(targets) => setLrTargets(targets)}
          windowIds={windowIds}
          onInvert={invertSelection}
        />
      )}

      {/* 瓦片右键菜单（自定义；多选态作用于全部选中；含色标/拒绝/加入相册/移入回收站） */}
      {ctxMenu && (
        <AssetContextMenu
          at={{ x: ctxMenu.x, y: ctxMenu.y }}
          assets={ctxMenu.assets}
          onClose={() => setCtxMenu(null)}
          onAddToAlbum={(targets) => setAddToAlbumTargets(targets)}
          onColorLabeled={handleColorLabeled}
          onRejected={handleRejected}
          onTrashRequest={requestTrashMove}
          onLrStaging={(targets) => setLrTargets(targets)}
        />
      )}

      {/* 「加入相册」选择弹窗（toast 报实际新增数与已在相册数） */}
      {addToAlbumTargets !== null && (
        <AddToAlbumDialog
          assets={addToAlbumTargets}
          onClose={() => setAddToAlbumTargets(null)}
        />
      )}

      {/* 「生成 LR 暂存夹」弹窗（操作条/右键菜单共用；结果含路径+硬链接/复制数） */}
      {lrTargets !== null && (
        <LrStagingDialog assets={lrTargets} onClose={() => setLrTargets(null)} />
      )}

      {/* 「移入回收站」确认一步（多选操作条/右键菜单共用；删除默认先入回收站） */}
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

      <ShortcutsHint />

      {/* 全屏查看器 */}
      {viewer && (
        <ViewerOverlay
          asset={viewer.asset}
          group={viewer.group}
          index={viewer.index}
          onNavigate={navigateTo}
          onClose={closeViewer}
          onAssetPatched={handleAssetPatched}
          onVersionSelect={handleVersionSelect}
        />
      )}
    </div>
  );
}

/** SearchInputs 形（FilterPanel 导出类型的本地别名，避免循环 import 噪音） */
type SearchInputsShim = ReturnType<typeof parseInputs>;
