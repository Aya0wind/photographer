import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router";
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
import {
  gallerySnapshot,
  saveGallerySnapshot,
} from "../lib/galleryCache";
import {
  GALLERY_JUSTIFY_ROW_PX,
  useGalleryTileSize,
} from "../lib/useGalleryTileSize";
import { useAssetViewer } from "../lib/useAssetViewer";
import AssetGrid, { type AssetGridHandle, type ViewportInfo } from "../components/AssetGrid";
import DateChipBar from "../components/DateChipBar";
import TileSizeSwitch from "../components/TileSizeSwitch";
import YearRail from "../components/YearRail";
import ViewerOverlay from "../components/ViewerOverlay";

/**
 * 画廊页（B 风格 Google Photos 深色沉浸，M3 主体；M4.5 A4 justify 网格）：
 * - 日期分组照片墙（AssetGrid justify：统一行高按宽高比分配宽、无尺寸 4:3 兜底、
 *   虚拟化 + 缩略图管线）；组头=日期+数量，滚动时当前组日期覆盖条吸顶；
 *   右侧年份吸顶条（滚动联动高亮、点击跳年首个日期组）
 * - 无限滚动：尾部哨兵 IntersectionObserver（提前 800px）触发 assetsPage 下一页
 *   （keyset afterId=已加载最后一条 id，limit 100）
 * - 顶部：日期 chips（含「未知」组）+ 日历按钮（年月下拉跳转，v1 不做整月历——
 *   chips 覆盖已加载组的快速跳转，日历覆盖任意月份的补页跳转）+ 三档尺寸切换
 * - RAW+JPG 合并展示（设置 gallery.mergeRawJpg，默认开）：pairId 成对的合并为
 *   一张卡（代表=JPG），角标「RAW+JPG」；点击进查看器即 JPG 版
 * - 状态：首屏骨架行 / 空库引导（去导入）/ IPC 不可用降级提示（可重试）
 * - 内容区水平居中 + 对称 padding（max-w 容器），chips 条与网格同宽对齐
 * - 查看器：/gallery?asset=<id>（组件不卸载，Esc 返回后滚动位置保留）
 */

const PAGE_LIMIT = 100;
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

export default function GalleryPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();

  // 会话快照（模块级，见 galleryCache.ts）：重挂载立即渲染缓存内容，
  // 后台静默 revalidate——此前每次挂载从零拉数据，切页回来全体资产回到加载态
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

  /** 快照落盘（加载/补页/日期更新后调用；滚动位置由视口回调实时维护） */
  const persistSnapshot = useCallback(() => {
    saveGallerySnapshot({
      assets: assetsRef.current,
      dates: datesRef.current,
      hasMore: hasMoreRef.current,
      scrollTop: scrollTopRef.current,
      savedAt: Date.now(),
    });
  }, []);

  // RAW+JPG 合并展示（设置项；旧 settings 数据缺 gallery 分组时兜底为开）
  const mergeEnabled = useSettingsStore((s) => s.settings.gallery?.mergeRawJpg ?? true);
  const { cards, badges } = useMemo(
    () => mergeRawJpgCards(assets, mergeEnabled),
    [assets, mergeEnabled],
  );
  const groups = useMemo(() => groupAssetsByDate(cards), [cards]);

  // 三档方格尺寸（localStorage 全局共享，画廊与搜索一致）
  const [tileSize, setTileSize] = useGalleryTileSize();

  /** 追加下一页（keyset）；返回本页结果供补页循环判断 */
  const appendPage = useCallback(async (): Promise<AssetDto[]> => {
    if (loadingRef.current || !hasMoreRef.current) return [];
    loadingRef.current = true;
    setLoadingMore(true);
    const seq = ++loadSeqRef.current;
    const afterId = assetsRef.current.length > 0
      ? assetsRef.current[assetsRef.current.length - 1].id
      : 0;
    const page = await assetsPage(afterId, PAGE_LIMIT);
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

  /** 首屏（重试共用）：有会话快照则静默 revalidate——首页与缓存前缀一致
   *  （库未变）保留已加载全量，首页变化（新导入/删除）才重置；无快照常规 loading。 */
  const initialLoad = useCallback(async () => {
    const cached = gallerySnapshot();
    if (!cached) setStatus("loading");
    hasMoreRef.current = true;
    loadingRef.current = false;
    const seq = ++loadSeqRef.current;
    const page = await assetsPage(0, PAGE_LIMIT);
    if (seq !== loadSeqRef.current) return;
    const samePrefix =
      cached !== null &&
      cached.assets.length >= page.length &&
      page.every((a, i) => cached.assets[i].id === a.id);
    if (!samePrefix) {
      assetsRef.current = page;
      setAssets(page);
    }
    if (page.length < PAGE_LIMIT) hasMoreRef.current = false;
    setStatus(page.length === 0 && !isIpcAvailable() ? "degraded" : "ready");
    persistSnapshot();
  }, [persistSnapshot]);

  useEffect(() => {
    let cancelled = false;
    void initialLoad();
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
  }, [initialLoad, persistSnapshot]);

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

  /** 日期跳转：组已加载直接滚；未加载顺序补页直到出现（keyset 下无法随机访问）。
   *  判定用 assetsRef（appendPage 内同步更新）；滚动经 pendingJumpKey 在渲染提交后
   *  执行——补页刚 setState 时 AssetGrid 的 rows/测量还是旧值，直接命令式滚动会 miss。 */
  const [pendingJumpKey, setPendingJumpKey] = useState<string | null>(null);
  useEffect(() => {
    if (pendingJumpKey === null) return;
    setPendingJumpKey(null);
    gridRef.current?.scrollToGroup(pendingJumpKey);
  }, [pendingJumpKey]);

  const jumpToDate = useCallback(
    async (date: string | null) => {
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
    [appendPage],
  );

  // 重挂载滚动恢复：快照有位置且首次 ready（数据已渲染）后立即还原
  const scrollRestoredRef = useRef(false);
  useEffect(() => {
    if (scrollRestoredRef.current || status !== "ready") return;
    const snap = gallerySnapshot();
    if (snap && snap.scrollTop > 0) {
      scrollRestoredRef.current = true;
      gridRef.current?.restoreScroll(snap.scrollTop);
    }
  }, [status, assets.length]);

  /** 视口回调：上报吸顶/日期之外，滚动位置实时记入快照（切页保留） */
  const handleViewportChange = useCallback((info: ViewportInfo) => {
    scrollTopRef.current = info.scrollTop;
    setViewport(info);
  }, []);

  /** 月份跳转（日历面板）：补页直到该月有资产，滚到该月最新一天所在组 */
  const jumpToMonth = useCallback(
    async (year: number, month: number) => {
      const prefix = `${year}-${String(month).padStart(2, "0")}`;
      const inMonth = () => assetsRef.current.find((a) => (a.capturedAt ?? "").startsWith(prefix));
      let guard = 0;
      let hit = inMonth();
      while (!hit && hasMoreRef.current && guard < 200) {
        guard += 1;
        const page = await appendPage();
        if (page.length === 0) break;
        hit = inMonth();
      }
      if (hit) setPendingJumpKey(groupKeyOfDate(hit.capturedAt));
    },
    [appendPage],
  );

  // 日历弹层：年/月下拉（v1 取年月跳转，不做整月历栅格）
  const [calendarOpen, setCalendarOpen] = useState(false);
  const [calYear, setCalYear] = useState(new Date().getFullYear());
  const [calMonth, setCalMonth] = useState(new Date().getMonth() + 1);
  const years = useMemo(() => yearRange(dates), [dates]);

  const { viewer, openAsset, closeViewer, navigateTo } = useAssetViewer(groups);

  const currentGroup = viewport.group;
  const showSticky =
    currentGroup !== null && viewport.scrollTop > STICKY_MIN_SCROLL && viewer === null;

  if (status === "loading") {
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

  if (status === "degraded") {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 px-8 text-center" data-testid="gallery-degraded">
        <p className="text-sm text-text-secondary">{t("gallery.ipcUnavailable")}</p>
        <button
          type="button"
          onClick={() => void initialLoad()}
          className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
        >
          {t("gallery.retry")}
        </button>
      </div>
    );
  }

  if (assets.length === 0) {
    return (
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
    );
  }

  return (
    <div className="h-full" data-testid="gallery-page">
      {/* 内容区：水平居中 + 左右对称 padding（修复贴导航边起排/首卡被切） */}
      <div
        className="mx-auto flex h-full w-full max-w-[1600px] flex-col px-6"
        data-testid="gallery-content"
      >
        {/* 顶部：日期 chips + 日历跳转 + 尺寸切换 */}
        <div className="flex h-11 shrink-0 items-center gap-2 border-b border-edge">
          <span className="shrink-0 text-xs font-medium text-text-secondary">{t("gallery.title")}</span>
          {dates.length > 0 && (
            <DateChipBar dates={dates} currentKey={viewport.group?.key ?? null} onJump={(d) => void jumpToDate(d)} />
          )}
          {/* 日历按钮 → 年月跳转弹层 */}
          <div className="relative shrink-0">
            <button
              type="button"
              onClick={() => setCalendarOpen((v) => !v)}
              aria-label={t("gallery.calendar.button")}
              aria-expanded={calendarOpen}
              className={`rounded-md border p-1 transition-colors ${
                calendarOpen
                  ? "border-accent text-accent"
                  : "border-edge text-text-secondary hover:border-text-muted hover:text-text-primary"
              }`}
              data-testid="gallery-calendar-button"
            >
              <svg
                viewBox="0 0 16 16"
                width="14"
                height="14"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.3"
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden="true"
              >
                <rect x="2" y="3" width="12" height="11" rx="1.5" />
                <path d="M2 6.5h12M5.5 1.5v3M10.5 1.5v3" />
              </svg>
            </button>
            {calendarOpen && (
              <div
                className="absolute right-0 top-9 z-20 flex items-center gap-1.5 rounded-lg border border-edge bg-surface p-2 shadow-lg"
                data-testid="gallery-calendar-panel"
              >
                <select
                  value={calYear}
                  onChange={(e) => setCalYear(Number(e.target.value))}
                  aria-label={t("gallery.calendar.year")}
                  className="rounded border border-edge bg-bg px-1.5 py-1 font-mono text-[11px] text-text-primary outline-none focus:border-accent"
                  data-testid="gallery-calendar-year"
                >
                  {years.map((y) => (
                    <option key={y} value={y}>{y}</option>
                  ))}
                </select>
                <select
                  value={calMonth}
                  onChange={(e) => setCalMonth(Number(e.target.value))}
                  aria-label={t("gallery.calendar.month")}
                  className="rounded border border-edge bg-bg px-1.5 py-1 font-mono text-[11px] text-text-primary outline-none focus:border-accent"
                  data-testid="gallery-calendar-month"
                >
                  {Array.from({ length: 12 }, (_, i) => i + 1).map((m) => (
                    <option key={m} value={m}>{t("gallery.calendar.monthN", { month: m })}</option>
                  ))}
                </select>
                <button
                  type="button"
                  onClick={() => {
                    setCalendarOpen(false);
                    void jumpToMonth(calYear, calMonth);
                  }}
                  className="rounded bg-accent px-2 py-1 text-[11px] font-medium text-black transition-colors hover:brightness-110"
                  data-testid="gallery-calendar-jump"
                >
                  {t("gallery.calendar.jump")}
                </button>
              </div>
            )}
          </div>
          <div className="ml-auto shrink-0">
            <TileSizeSwitch value={tileSize} onChange={setTileSize} />
          </div>
        </div>

        {/* 照片墙（justify 布局：统一行高、按宽高比分配宽）+ 吸顶当前日期 + 年份条 */}
        <div className="relative min-h-0 flex-1">
          <AssetGrid
            ref={gridRef}
            groups={groups}
            onOpenAsset={openAsset}
            sentinelRef={sentinelRef}
            onViewportChange={handleViewportChange}
            layout="justify"
            tile={GALLERY_JUSTIFY_ROW_PX[tileSize]}
            badges={badges}
          />
          {/* 右侧年份吸顶条：点击跳该年首个日期组 */}
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
          <AnimatePresence initial={false}>
            {showSticky && (
              <motion.div
                key="sticky-date"
                initial={{ opacity: 0, y: -8 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: -8 }}
                transition={{ duration: 0.15, ease: "easeOut" }}
                className="pointer-events-none absolute inset-x-0 top-0 z-10 border-b border-edge/60 bg-bg/85 px-4 py-2 backdrop-blur-sm"
                data-testid="gallery-sticky-date"
              >
                <AnimatePresence mode="wait" initial={false}>
                  <motion.span
                    key={currentGroup.key}
                    initial={{ opacity: 0, y: 4 }}
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
        {loadingMore && (
          <div className="flex h-7 shrink-0 items-center justify-center text-[11px] text-text-muted" data-testid="gallery-loading-more">
            {t("gallery.loadingMore")}
          </div>
        )}
      </div>

      {/* 全屏查看器（/gallery?asset=<id>） */}
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
