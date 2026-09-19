import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { assetsPage, type AssetDto, type AssetFilters, type AssetKind } from "@/ipc/api";
import { groupAssetsByDate } from "@/features/gallery/lib/assetGroups";
import AssetGrid from "@/features/gallery/components/AssetGrid";
import { useAssetViewer } from "@/features/gallery/lib/useAssetViewer";
import ViewerOverlay from "@/features/gallery/components/ViewerOverlay";
import { useDebouncedValue } from "@/lib/useDebouncedValue";

/**
 * 搜索页（M3）：紧凑工业风条件栏 + 结果复用画廊网格。
 * - 条件：类型（全部/照片/RAW/视频）、日期范围 from/to、相机文本；
 *   条件变更即时查询（300ms 防抖，以序列化键防抖避免对象身份抖动）
 * - 查询：assetsPage(0, 100, filters)（camelCase 平铺负载）；结果 keyset 追加加载
 *   （哨兵触发，与画廊同一 AssetGrid/缩略图管线）
 * - 计数徽标显示已加载结果数；空态区分「无匹配」
 */

const PAGE_LIMIT = 100;
const DEBOUNCE_MS = 300;

const KIND_OPTIONS: ReadonlyArray<{ value: "all" | AssetKind; labelKey: string }> = [
  { value: "all", labelKey: "search.kind.all" },
  { value: "photo", labelKey: "search.kind.photo" },
  { value: "raw", labelKey: "search.kind.raw" },
  { value: "video", labelKey: "search.kind.video" },
];

/** 条件 → AssetFilters（全空=无过滤） */
function buildFilters(kind: string, from: string, to: string, camera: string): AssetFilters {
  const filters: AssetFilters = {};
  if (kind !== "all") filters.kind = kind as AssetKind;
  if (from) filters.capturedAfter = from;
  if (to) filters.capturedBefore = to;
  const trimmed = camera.trim();
  if (trimmed) filters.camera = trimmed;
  return filters;
}

/** 序列化键（防抖用，原始值拼接） ↔ 条件互转 */
function serializeInputs(kind: string, from: string, to: string, camera: string): string {
  return JSON.stringify([kind, from, to, camera]);
}
function parseFilters(key: string): AssetFilters {
  const [kind, from, to, camera] = JSON.parse(key) as [string, string, string, string];
  return buildFilters(kind, from, to, camera);
}

export default function SearchPage() {
  const { t } = useTranslation();

  const [kind, setKind] = useState<"all" | AssetKind>("all");
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [camera, setCamera] = useState("");

  // 防抖键：原始值拼接（字符串，身份稳定）；条件未变不重查
  const rawKey = serializeInputs(kind, from, to, camera);
  const debouncedKey = useDebouncedValue(rawKey, DEBOUNCE_MS);
  const appliedFilters = useMemo<AssetFilters>(() => parseFilters(debouncedKey), [debouncedKey]);
  const appliedKey = debouncedKey;

  const [results, setResults] = useState<AssetDto[]>([]);
  const resultsRef = useRef<AssetDto[]>([]);
  const [queryState, setQueryState] = useState<"loading" | "ready">("loading");
  const hasMoreRef = useRef(true);
  const loadingRef = useRef(false);
  const querySeqRef = useRef(0);
  const appliedKeyRef = useRef(appliedKey);
  appliedKeyRef.current = appliedKey;

  // 条件变化（防抖后）→ 重置查询首页
  useEffect(() => {
    let cancelled = false;
    const seq = ++querySeqRef.current;
    setQueryState("loading");
    hasMoreRef.current = true;
    loadingRef.current = false;
    void assetsPage(0, PAGE_LIMIT, appliedFilters).then((page) => {
      if (cancelled || seq !== querySeqRef.current) return;
      resultsRef.current = page;
      setResults(page);
      hasMoreRef.current = page.length === PAGE_LIMIT;
      setQueryState("ready");
    });
    return () => {
      cancelled = true;
    };
  }, [appliedFilters, appliedKey]);

  /** 追加下一页（沿用当前条件） */
  const appendPage = useCallback(async (): Promise<AssetDto[]> => {
    if (loadingRef.current || !hasMoreRef.current) return [];
    loadingRef.current = true;
    const seq = querySeqRef.current;
    const afterId = resultsRef.current.length > 0
      ? resultsRef.current[resultsRef.current.length - 1].id
      : 0;
    // 以序列化键重建条件（避免闭包过期：哨兵 observer 挂载期间条件可能已切换）
    const page = await assetsPage(afterId, PAGE_LIMIT, parseFilters(appliedKeyRef.current));
    if (seq !== querySeqRef.current) return page;
    if (page.length > 0) {
      const seen = new Set(resultsRef.current.map((a) => a.id));
      const fresh = page.filter((a) => !seen.has(a.id));
      resultsRef.current = [...resultsRef.current, ...fresh];
      setResults(resultsRef.current);
    }
    if (page.length < PAGE_LIMIT) hasMoreRef.current = false;
    loadingRef.current = false;
    return page;
  }, []);

  const sentinelRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const el = sentinelRef.current;
    if (!el || queryState !== "ready" || typeof IntersectionObserver === "undefined") return;
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) void appendPage();
      },
      { rootMargin: "800px 0px" },
    );
    io.observe(el);
    return () => io.disconnect();
  }, [queryState, appendPage, results.length]);

  const groups = useMemo(() => groupAssetsByDate(results), [results]);
  const { viewer, openAsset, closeViewer, navigateTo } = useAssetViewer(groups);

  return (
    <div className="flex h-full flex-col" data-testid="search-page">
      {/* 条件栏（紧凑工业风） */}
      <div className="flex h-11 shrink-0 items-center gap-3 border-b border-edge px-3" data-testid="search-filters">
        {/* 类型分段 */}
        <div
          className="flex items-center rounded-md border border-edge bg-bg p-0.5"
          role="radiogroup"
          aria-label={t("search.kind")}
          data-testid="search-kind"
        >
          {KIND_OPTIONS.map((option) => (
            <button
              key={option.value}
              type="button"
              role="radio"
              aria-checked={kind === option.value}
              onClick={() => setKind(option.value)}
              className={`rounded px-2 py-0.5 text-[11px] font-medium transition-colors ${
                kind === option.value
                  ? "bg-accent text-black"
                  : "text-text-secondary hover:text-text-primary"
              }`}
              data-testid={`search-kind-${option.value}`}
            >
              {t(option.labelKey)}
            </button>
          ))}
        </div>

        {/* 日期范围 */}
        <label className="flex items-center gap-1 text-[11px] text-text-muted">
          {t("search.dateFrom")}
          <input
            type="date"
            value={from}
            onChange={(e) => setFrom(e.target.value)}
            aria-label={t("search.dateFrom")}
            className="rounded border border-edge bg-bg px-1.5 py-0.5 font-mono text-[11px] text-text-primary outline-none transition-colors focus:border-accent"
            data-testid="search-from"
          />
        </label>
        <label className="flex items-center gap-1 text-[11px] text-text-muted">
          {t("search.dateTo")}
          <input
            type="date"
            value={to}
            onChange={(e) => setTo(e.target.value)}
            aria-label={t("search.dateTo")}
            className="rounded border border-edge bg-bg px-1.5 py-0.5 font-mono text-[11px] text-text-primary outline-none transition-colors focus:border-accent"
            data-testid="search-to"
          />
        </label>

        {/* 相机 */}
        <label className="flex min-w-0 flex-1 items-center gap-1 text-[11px] text-text-muted">
          {t("search.camera")}
          <input
            type="text"
            value={camera}
            onChange={(e) => setCamera(e.target.value)}
            placeholder={t("search.cameraPlaceholder")}
            aria-label={t("search.camera")}
            className="min-w-0 flex-1 rounded border border-edge bg-bg px-1.5 py-0.5 text-[11px] text-text-primary outline-none transition-colors focus:border-accent"
            data-testid="search-camera"
          />
        </label>

        {/* 计数徽标 */}
        <span
          className="shrink-0 rounded-full bg-panel px-2 py-0.5 font-mono text-[11px] tabular-nums text-text-secondary"
          data-testid="search-count"
        >
          {queryState === "loading" ? t("search.loading") : t("search.count", { count: results.length })}
        </span>
      </div>

      {/* 结果：复用画廊网格（同一虚拟化 + 缩略图管线 + 查看器） */}
      <div className="relative min-h-0 flex-1">
        {queryState === "loading" && results.length === 0 ? (
          <div className="flex h-full items-center justify-center text-xs text-text-muted" data-testid="search-loading">
            {t("search.loading")}
          </div>
        ) : results.length === 0 ? (
          <div className="flex h-full flex-col items-center justify-center gap-2 px-8 text-center" data-testid="search-empty">
            <p className="text-sm text-text-secondary">{t("search.empty")}</p>
            <p className="text-xs text-text-muted">{t("search.emptyHint")}</p>
          </div>
        ) : (
          <AssetGrid
            groups={groups}
            onOpenAsset={openAsset}
            sentinelRef={sentinelRef}
            scrollTestId="search-grid-scroll"
          />
        )}
      </div>

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
