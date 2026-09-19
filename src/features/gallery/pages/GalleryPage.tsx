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
import {
  groupAssetsByDate,
  groupKeyOfDate,
  formatDateLabel,
} from "../lib/assetGroups";
import { useAssetViewer } from "../lib/useAssetViewer";
import AssetGrid, { type AssetGridHandle, type ViewportInfo } from "../components/AssetGrid";
import DateChipBar from "../components/DateChipBar";
import ViewerOverlay from "../components/ViewerOverlay";

/**
 * 画廊页（B 风格 Google Photos 深色沉浸，M3 主体）：
 * - 日期分组照片墙（AssetGrid：虚拟化等宽方格 + 缩略图管线）；
 *   组头=日期+数量，滚动时当前组日期以覆盖条吸顶（150ms 切换动画——虚拟行内做
 *   CSS sticky 需要 transform 对齐，覆盖条是虚拟化下更稳的形态）
 * - 无限滚动：尾部哨兵 IntersectionObserver（提前 800px）触发 assetsPage 下一页
 *   （keyset afterId=已加载最后一条 id，limit 100）
 * - 顶部日期 chips（assetGroupDates，含「未知」组）点击滚动到组；目标组未加载时
 *   顺序补页直到出现
 * - 状态：首屏骨架行 / 空库引导（去导入）/ IPC 不可用降级提示（可重试）
 * - 查看器：/gallery?asset=<id>（组件不卸载，Esc 返回后滚动位置保留——见 useAssetViewer）
 */

const PAGE_LIMIT = 100;
/** 组头完全滚出视口后显示吸顶条（低于此偏移视为仍在组头处） */
const STICKY_MIN_SCROLL = 48;

export default function GalleryPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();

  const [assets, setAssets] = useState<AssetDto[]>([]);
  /** 已加载资产（与 state 同步维护，供补页循环同步读取） */
  const assetsRef = useRef<AssetDto[]>([]);
  const [status, setStatus] = useState<"loading" | "ready" | "degraded">("loading");
  const hasMoreRef = useRef(true);
  const loadingRef = useRef(false);
  const loadSeqRef = useRef(0);
  const [loadingMore, setLoadingMore] = useState(false);

  const [dates, setDates] = useState<AssetGroupDate[]>([]);
  const gridRef = useRef<AssetGridHandle | null>(null);
  const sentinelRef = useRef<HTMLDivElement | null>(null);
  const [viewport, setViewport] = useState<ViewportInfo>({ scrollTop: 0, group: null });

  const groups = useMemo(() => groupAssetsByDate(assets), [assets]);

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
    return page;
  }, []);

  /** 首屏（重试共用）：空结果且 IPC 不可用 → 降级态 */
  const initialLoad = useCallback(async () => {
    setStatus("loading");
    hasMoreRef.current = true;
    loadingRef.current = false;
    const seq = ++loadSeqRef.current;
    const page = await assetsPage(0, PAGE_LIMIT);
    if (seq !== loadSeqRef.current) return;
    assetsRef.current = page;
    setAssets(page);
    if (page.length < PAGE_LIMIT) hasMoreRef.current = false;
    setStatus(page.length === 0 && !isIpcAvailable() ? "degraded" : "ready");
  }, []);

  useEffect(() => {
    let cancelled = false;
    void initialLoad();
    void assetGroupDates().then((result) => {
      if (!cancelled) setDates(result);
    });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

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
    <div className="relative flex h-full flex-col" data-testid="gallery-page">
      {/* 顶部：日期 chips 条 */}
      <div className="flex h-11 shrink-0 items-center gap-2 border-b border-edge px-3">
        <span className="shrink-0 text-xs font-medium text-text-secondary">{t("gallery.title")}</span>
        {dates.length > 0 && (
          <DateChipBar dates={dates} currentKey={viewport.group?.key ?? null} onJump={(d) => void jumpToDate(d)} />
        )}
      </div>

      {/* 照片墙 + 吸顶当前日期 */}
      <div className="relative min-h-0 flex-1">
        <AssetGrid
          ref={gridRef}
          groups={groups}
          onOpenAsset={openAsset}
          sentinelRef={sentinelRef}
          onViewportChange={setViewport}
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
