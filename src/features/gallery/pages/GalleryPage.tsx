import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion } from "motion/react";

import {
  assetGroupDates,
  assetsPage,
  isIpcAvailable,
  type AssetDto,
  type AssetGroupDate,
} from "@/ipc/api";
import { useSettingsStore } from "@/stores/settingsStore";
import {
  groupAssetsByDate,
  groupKeyOfDate,
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
import { useSemanticSearch } from "@/features/ai/useSemanticSearch";
import { recordSemanticQuery } from "@/features/ai/semanticHistory";
import { SemanticQueryInput } from "@/features/ai/SemanticResultsView";
import AssetGrid, { type AssetGridHandle, type ViewportInfo } from "../components/AssetGrid";
import SelectionBar from "../components/SelectionBar";
import TileSizeSwitch from "../components/TileSizeSwitch";
import YearRail from "../components/YearRail";
import ShortcutsHint from "../components/ShortcutsHint";
import ViewerOverlay from "../components/ViewerOverlay";
import {
  FilterChipsRow,
  FilterPanel,
  buildChips,
  buildFilters,
  hasActiveFilters,
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
 * - 日期组头折叠（AssetGrid）；右侧年份吸顶条承担日期跳转（chips 条与日历按钮已删）
 * - 多选（M4.5）：选择按钮 / Ctrl+点击 / 长按进入；浮动操作条（收藏/旗标/分享/取消），
 *   Esc 退出；单选=多选下的 N=1
 * - URL 协议：?mode=semantic&q=…（语义直达）、?kind=photo|raw|video（预置类型）
 * - 快照缓存：仅默认态落盘（筛选/语义态不落）；? 键快捷键速查见 AppShell
 */

const PAGE_LIMIT = 100;
const DEBOUNCE_MS = 300;
/** 组头完全滚出视口后显示吸顶条（低于此偏移视为仍在组头处） */
const STICKY_MIN_SCROLL = 48;

/** 年月选择范围：chips 数据的年份边界 ∪ 当前年（空数据退当前年） */
function yearRange(dates: AssetGroupDate[]): number[] {
  const now = new Date().getFullYear();
  const years = new Set<number>([now]);
  for (const entry of dates) {
    const y = Number(entry.date?.slice(0, 4));
    if (Number.isFinite(y) && y > 0) years.add(y);
  }
  return [...years].sort((a, b) => b - a);
}

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

  function runSemantic(query: string): void {
    recordSemanticQuery(query); // 语义历史（最近 5 条，localStorage）
    void semantic.run(query);
  }

  /** 修改筛选 = 退出语义态回筛选/默认（语义结果与条件筛选互斥） */
  function patchFilters(patch: Partial<SearchInputsShim>): void {
    semantic.reset();
    patchInputs(patch);
  }

  const chips = useMemo(() => buildChips(inputs, t), [inputs, t]);

  // --- URL 协议（全局搜索框 / 类型筛选直达）：?mode=semantic&q= / ?kind= ----------------
  const urlQuery = searchParams.get("q") ?? "";
  const appliedUrlQueryRef = useRef<string | null>(null);
  const appliedUrlKindRef = useRef<string | null>(null);
  useEffect(() => {
    const urlMode = searchParams.get("mode");
    if (urlMode === "semantic" && urlQuery !== "" && appliedUrlQueryRef.current !== urlQuery) {
      appliedUrlQueryRef.current = urlQuery;
      runSemantic(urlQuery);
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
  /** 已加载资产（与 state 同步维护，供补页循环同步读取） */
  const assetsRef = useRef<AssetDto[]>(cachedAtMount?.assets ?? []);
  const [status, setStatus] = useState<"loading" | "ready" | "degraded">(() =>
    cachedAtMount && cachedAtMount.assets.length > 0 ? "ready" : "loading",
  );
  const hasMoreRef = useRef(cachedAtMount?.hasMore ?? true);
  const loadingRef = useRef(false);
  const loadSeqRef = useRef(0);
  const [loadingMore, setLoadingMore] = useState(false);

  const [dates, setDates] = useState<AssetGroupDate[]>(() => cachedAtMount?.dates ?? []);
  const datesRef = useRef<AssetGroupDate[]>(cachedAtMount?.dates ?? []);
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
      dates: datesRef.current,
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
        if (!samePrefix) {
          assetsRef.current = page;
          setAssets(page);
        }
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

  useEffect(() => {
    if (semanticMode) return;
    let cancelled = false;
    void assetGroupDates().then((result) => {
      if (cancelled) return;
      datesRef.current = result;
      setDates(result);
      persistSnapshot();
    });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [semanticMode, persistSnapshot]);

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

  /** 日期跳转（年份条）：组已加载直接滚；未加载顺序补页直到出现（keyset 无随机访问） */
  const [pendingJumpKey, setPendingJumpKey] = useState<string | null>(null);
  useEffect(() => {
    if (pendingJumpKey === null) return;
    setPendingJumpKey(null);
    gridRef.current?.scrollToGroup(pendingJumpKey);
  }, [pendingJumpKey]);

  const jumpToDate = useCallback(
    async (date: string | null) => {
      if (semanticMode) return;
      const key = groupKeyOfDate(date);
      const loaded = () => assetsRef.current.some((a) => groupKeyOfDate(a.capturedAt) === key);
      let guard = 0;
      while (!loaded() && hasMoreRef.current && guard < 200) {
        guard += 1;
        const page = await appendPage();
        if (page.length === 0) break;
      }
      setPendingJumpKey(key);
    },
    [appendPage, semanticMode],
  );

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

  // 三档尺寸（justify 行高）
  const [tileSize, setTileSize] = useGalleryTileSize();
  const years = useMemo(() => yearRange(dates), [dates]);

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
        className="mx-auto flex h-full w-full max-w-[1600px] flex-col px-6"
        data-testid="gallery-content"
      >
        {/* 顶部工具条：语义查询 + 筛选 + 选择 + 计数 + 尺寸 */}
        <div className="flex h-11 shrink-0 items-center gap-2.5 border-b border-edge" data-testid="gallery-toolbar">
          <SemanticQueryInput
            busy={semantic.status === "loading"}
            onRun={runSemantic}
            initialQuery={urlQuery}
          />

          {/* 筛选按钮：展开/收起面板；激活条件计数徽标 */}
          <button
            type="button"
            onClick={() => setPanelOpen((v) => !v)}
            aria-expanded={panelOpen}
            className={`flex shrink-0 items-center gap-1.5 rounded-md border px-2.5 py-1 text-[11px] transition-colors ${
              panelOpen || chips.length > 0
                ? "border-accent text-accent"
                : "border-edge text-text-secondary hover:border-text-muted hover:text-text-primary"
            }`}
            data-testid="search-filter-toggle"
          >
            {t("search.filter")}
            {chips.length > 0 && (
              <span
                className="rounded-full bg-accent px-1.5 text-[10px] font-bold leading-4 text-black"
                data-testid="search-filter-count"
              >
                {chips.length}
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

          {/* 选择按钮（多选开关） */}
          <button
            type="button"
            onClick={() => (selecting ? exitSelection() : setSelecting(true))}
            aria-pressed={selecting}
            className={`flex shrink-0 items-center gap-1.5 rounded-md border px-2.5 py-1 text-[11px] transition-colors ${
              selecting
                ? "border-accent bg-accent/10 text-accent"
                : "border-edge text-text-secondary hover:border-text-muted hover:text-text-primary"
            }`}
            data-testid="gallery-select-toggle"
          >
            <svg
              viewBox="0 0 16 16"
              width="11"
              height="11"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden="true"
            >
              <rect x="2" y="2" width="12" height="12" rx="2" />
              <path d="M5 8.5l2 2 4-4.5" />
            </svg>
            {t("gallery.select")}
          </button>

          {/* 计数徽标 */}
          <span
            className="ml-auto shrink-0 rounded-full bg-panel px-2 py-0.5 font-mono text-[11px] tabular-nums text-text-secondary"
            data-testid="search-count"
          >
            {semanticMode
              ? t("search.count", { count: semantic.assets.length })
              : t("search.count", { count: assets.length })}
          </span>

          <div className="shrink-0">
            <TileSizeSwitch value={tileSize} onChange={setTileSize} />
          </div>
        </div>

        {/* 筛选面板（默认收起；修改筛选自动退出语义态） */}
        {panelOpen && <FilterPanel inputs={inputs} onPatch={patchFilters} />}

        {/* 激活条件 chips */}
        {!semanticMode && (
          <FilterChipsRow
            chips={chips}
            onPatch={(next) => {
              semantic.reset();
              setInputs(next);
            }}
            onClearAll={() => {
              semantic.reset();
              setInputs(EMPTY_INPUTS);
            }}
          />
        )}

        {/* 照片墙（justify 布局）+ 吸顶当前日期 + 年份条 */}
        <div className="relative min-h-0 flex-1">
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
          {/* 右侧年份吸顶条：点击跳该年首个日期组（语义结果态隐藏——库级年份与结果集不一致） */}
          {!semanticMode && (
            <YearRail
              years={years}
              currentYear={
                viewport.group?.date != null ? Number(viewport.group.date.slice(0, 4)) : null
              }
              onJumpYear={(year) => {
                const hit = dates.find((d) => d.date != null && d.date.startsWith(String(year)));
                if (hit) void jumpToDate(hit.date);
              }}
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
                  <motion.span
                    key={currentGroup.key}
                    initial={motionInitial(motionOn, { opacity: 0, y: 4 })}
                    animate={{ opacity: 1, y: 0 }}
                    exit={{ opacity: 0, y: -4 }}
                    transition={{ duration: 0.15, ease: "easeOut" }}
                    className="text-[13px] font-semibold text-text-primary"
                  >
                    {currentGroup.date === null
                      ? t("gallery.unknownDate")
                      : formatDateLabel(currentGroup.date)}
                    <span className="ml-2 text-xs font-normal text-text-muted">
                      {t("gallery.groupCount", { count: currentGroup.assets.length })}
                    </span>
                  </motion.span>
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

      {/* 多选浮动操作条（已选 N | 收藏/旗标/分享/取消） */}
      {selecting && (
        <SelectionBar
          count={selectedAssets.length}
          assets={selectedAssets}
          onDone={exitSelection}
        />
      )}

      {/* 首次进画廊：快捷键一次性提示条 */}
      <ShortcutsHint />

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

/** SearchInputs 形（FilterPanel 导出类型的本地别名，避免循环 import 噪音） */
type SearchInputsShim = ReturnType<typeof parseInputs>;
