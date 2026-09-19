import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  assetsPage,
  cameraList,
  type AssetCameraCount,
  type AssetDto,
  type AssetFilters,
  type AssetKind,
} from "@/ipc/api";
import { useSettingsStore } from "@/stores/settingsStore";
import { groupAssetsByDate } from "@/features/gallery/lib/assetGroups";
import { mergeRawJpgCards } from "@/features/gallery/lib/mergeRawJpg";
import { GALLERY_TILE_PX, useGalleryTileSize } from "@/features/gallery/lib/useGalleryTileSize";
import AssetGrid from "@/features/gallery/components/AssetGrid";
import TileSizeSwitch from "@/features/gallery/components/TileSizeSwitch";
import { useAssetViewer } from "@/features/gallery/lib/useAssetViewer";
import ViewerOverlay from "@/features/gallery/components/ViewerOverlay";
import { useDebouncedValue } from "@/lib/useDebouncedValue";

/**
 * 搜索页（M3+）：紧凑工业风条件栏 + 结果复用画廊网格。
 * - 条件：类型（全部/照片/RAW/视频）、日期范围（原生 date input 深色适配 +
 *   快捷段 近7天/近30天/今年/去年）、相机勾选下拉（cameraList 计数清单；
 *   多选 OR 语义——后端 camera 参数当前单值，先传第一个，数组契约到位后切全量）；
 *   条件变更即时查询（300ms 防抖，以序列化键防抖）
 * - RAW+JPG 合并展示与画廊同开关（gallery.mergeRawJpg）
 * - 查询：assetsPage(0, 100, filters)（camelCase 平铺负载）；结果 keyset 追加加载
 * - 三档方格尺寸与画廊共享（smartphoto.gallery.tileSize）；内容区居中对称 padding
 */

const PAGE_LIMIT = 100;
const DEBOUNCE_MS = 300;

/** 类型两档（M3 二轮）：RAW 归入「照片」档（RAW 也是照片），不再单列 */
const KIND_OPTIONS: ReadonlyArray<{ value: "all" | "photo" | "video"; labelKey: string }> = [
  { value: "all", labelKey: "search.kind.all" },
  { value: "photo", labelKey: "search.kind.photo" },
  { value: "video", labelKey: "search.kind.video" },
];

/** UI 档位 → filters.kinds（照片=photo+raw；视频=video；全部=不传） */
function kindsOf(kind: "all" | "photo" | "video"): AssetKind[] | undefined {
  if (kind === "photo") return ["photo", "raw"];
  if (kind === "video") return ["video"];
  return undefined;
}

type QuickRangeKey = "recent7" | "recent30" | "thisYear" | "lastYear";

function ymd(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** 快捷日期段 → [from, to]（"YYYY-MM-DD"，含端点；本地时区） */
export function quickRange(key: QuickRangeKey, now = new Date()): [string, string] {
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  if (key === "recent7") {
    const from = new Date(today);
    from.setDate(from.getDate() - 6);
    return [ymd(from), ymd(today)];
  }
  if (key === "recent30") {
    const from = new Date(today);
    from.setDate(from.getDate() - 29);
    return [ymd(from), ymd(today)];
  }
  if (key === "thisYear") return [`${today.getFullYear()}-01-01`, ymd(today)];
  return [`${today.getFullYear() - 1}-01-01`, `${today.getFullYear() - 1}-12-31`];
}

/**
 * "YYYY-MM-DD" → RFC3339（后端 parse_from_rfc3339 归一为 UTC 绝对时间比较）。
 * 起点取本地当日 00:00:00.000，终点取本地当日 23:59:59.999（含当日，本地日界）。
 * 日期不合法返回 undefined（不进 filters，避免后端 400/Err 导致整页空结果）。
 */
export function dateToRfc3339(dateOnly: string, endOfDay = false): string | undefined {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(dateOnly);
  if (!m) return undefined;
  const date = new Date(
    Number(m[1]),
    Number(m[2]) - 1,
    Number(m[3]),
    endOfDay ? 23 : 0,
    endOfDay ? 59 : 0,
    endOfDay ? 59 : 0,
    endOfDay ? 999 : 0,
  );
  if (Number.isNaN(date.getTime())) return undefined;
  return date.toISOString();
}

/** 条件 → AssetFilters（全空=无过滤；相机多选 OR——单值契约期只传第一个） */
function buildFilters(
  kind: "all" | "photo" | "video",
  from: string,
  to: string,
  cameras: string[],
): AssetFilters {
  const filters: AssetFilters = {};
  const kinds = kindsOf(kind);
  if (kinds) filters.kinds = kinds;
  // 日期转 RFC3339：后端按绝对时间比较；无效日期不传（空条件语义）
  const after = dateToRfc3339(from, false);
  if (from && after) filters.capturedAfter = after;
  const before = dateToRfc3339(to, true);
  if (to && before) filters.capturedBefore = before;
  if (cameras.length > 0) filters.camera = cameras[0];
  return filters;
}

/** 序列化键（防抖用，原始值拼接） ↔ 条件互转 */
function serializeInputs(
  kind: "all" | "photo" | "video",
  from: string,
  to: string,
  cameras: string[],
): string {
  return JSON.stringify([kind, from, to, cameras.join("\u0000")]);
}
function parseFilters(key: string): AssetFilters {
  const [kind, from, to, cameraBlob] = JSON.parse(key) as [
    "all" | "photo" | "video",
    string,
    string,
    string,
  ];
  const cameras = cameraBlob === "" ? [] : cameraBlob.split("\u0000");
  return buildFilters(kind, from, to, cameras);
}

export default function SearchPage() {
  const { t } = useTranslation();

  const [kind, setKind] = useState<"all" | "photo" | "video">("all");
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [cameras, setCameras] = useState<string[]>([]);

  // 防抖键：原始值拼接（字符串，身份稳定）；条件未变不重查
  const rawKey = serializeInputs(kind, from, to, cameras);
  const debouncedKey = useDebouncedValue(rawKey, DEBOUNCE_MS);
  const appliedFilters = useMemo<AssetFilters>(() => parseFilters(debouncedKey), [debouncedKey]);
  const appliedKey = debouncedKey;

  // 相机清单（打开页面即拉一次；失败静默空清单）
  const [cameraOptions, setCameraOptions] = useState<AssetCameraCount[]>([]);
  const [cameraOpen, setCameraOpen] = useState(false);
  useEffect(() => {
    let cancelled = false;
    void cameraList().then((list) => {
      if (!cancelled) setCameraOptions(list);
    });
    return () => {
      cancelled = true;
    };
  }, []);

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

  // RAW+JPG 合并（与画廊同开关）+ 日期分组
  const mergeEnabled = useSettingsStore((s) => s.settings.gallery?.mergeRawJpg ?? true);
  const { cards, badges } = useMemo(
    () => mergeRawJpgCards(results, mergeEnabled),
    [results, mergeEnabled],
  );
  const groups = useMemo(() => groupAssetsByDate(cards), [cards]);
  const { viewer, openAsset, closeViewer, navigateTo } = useAssetViewer(groups);

  // 三档方格尺寸（与画廊共享）
  const [tileSize, setTileSize] = useGalleryTileSize();

  function toggleCamera(camera: string): void {
    setCameras((prev) =>
      prev.includes(camera) ? prev.filter((c) => c !== camera) : [...prev, camera],
    );
  }

  function applyQuickRange(key: QuickRangeKey): void {
    const [qFrom, qTo] = quickRange(key);
    setFrom(qFrom);
    setTo(qTo);
  }

  return (
    <div className="h-full" data-testid="search-page">
      {/* 内容区：水平居中 + 对称 padding（与画廊一致） */}
      <div
        className="mx-auto flex h-full w-full max-w-[1600px] flex-col px-6"
        data-testid="search-content"
      >
        {/* 条件栏（紧凑工业风） */}
        <div className="flex h-11 shrink-0 items-center gap-3 border-b border-edge" data-testid="search-filters">
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

          {/* 日期范围：原生 date input（深色适配）+ 快捷段 */}
          <label className="flex items-center gap-1 text-[11px] text-text-muted">
            {t("search.dateFrom")}
            <input
              type="date"
              value={from}
              onChange={(e) => setFrom(e.target.value)}
              aria-label={t("search.dateFrom")}
              className="rounded border border-edge bg-bg px-1.5 py-0.5 font-mono text-[11px] text-text-primary outline-none transition-colors focus:border-accent [color-scheme:dark]"
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
              className="rounded border border-edge bg-bg px-1.5 py-0.5 font-mono text-[11px] text-text-primary outline-none transition-colors focus:border-accent [color-scheme:dark]"
              data-testid="search-to"
            />
          </label>
          <div className="flex shrink-0 items-center gap-0.5" data-testid="search-quick-ranges">
            {(["recent7", "recent30", "thisYear", "lastYear"] as const).map((key) => (
              <button
                key={key}
                type="button"
                onClick={() => applyQuickRange(key)}
                className="rounded border border-edge px-1.5 py-0.5 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
                data-testid={`search-quick-${key}`}
              >
                {t(`search.quick.${key}`)}
              </button>
            ))}
          </div>

          {/* 相机：勾选下拉（cameraList 计数清单；「全部」为默认态） */}
          <div className="relative min-w-0 shrink-0">
            <button
              type="button"
              onClick={() => setCameraOpen((v) => !v)}
              aria-expanded={cameraOpen}
              aria-label={t("search.camera")}
              className={`flex items-center gap-1 rounded-md border px-2 py-0.5 text-[11px] transition-colors ${
                cameras.length > 0 || cameraOpen
                  ? "border-accent text-accent"
                  : "border-edge text-text-secondary hover:border-text-muted hover:text-text-primary"
              }`}
              data-testid="search-camera-button"
            >
              {cameras.length > 0 ? cameras[0] : t("search.cameraAll")}
              {cameras.length > 1 && <span className="font-mono">+{cameras.length - 1}</span>}
              <svg viewBox="0 0 16 16" width="9" height="9" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <path d="M3.5 6l4.5 4.5L12.5 6" />
              </svg>
            </button>
            {cameraOpen && (
              <div
                className="sp-scroll absolute left-0 top-7 z-20 max-h-64 w-56 overflow-y-auto rounded-lg border border-edge bg-surface p-1 shadow-lg"
                data-testid="search-camera-menu"
              >
                {cameraOptions.length === 0 ? (
                  <p className="px-2 py-2 text-[11px] leading-relaxed text-text-muted">
                    {t("search.cameraEmpty")}
                  </p>
                ) : (
                  cameraOptions.map((option) => (
                    <label
                      key={option.camera}
                      className="flex cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-left transition-colors hover:bg-panel/40"
                      data-testid="search-camera-option"
                      data-camera={option.camera}
                      data-checked={cameras.includes(option.camera)}
                    >
                      <input
                        type="checkbox"
                        checked={cameras.includes(option.camera)}
                        onChange={() => toggleCamera(option.camera)}
                        className="h-3 w-3 accent-[#F0A83C]"
                      />
                      <span className="min-w-0 flex-1 truncate text-[11px] text-text-secondary" title={option.camera}>
                        {option.camera}
                      </span>
                      <span className="shrink-0 rounded bg-panel px-1.5 py-0.5 font-mono text-[10px] tabular-nums text-text-muted">
                        {option.count}
                      </span>
                    </label>
                  ))
                )}
              </div>
            )}
          </div>

          {/* 计数徽标 */}
          <span
            className="ml-auto shrink-0 rounded-full bg-panel px-2 py-0.5 font-mono text-[11px] tabular-nums text-text-secondary"
            data-testid="search-count"
          >
            {queryState === "loading" ? t("search.loading") : t("search.count", { count: results.length })}
          </span>

          {/* 三档尺寸（与画廊共享） */}
          <TileSizeSwitch value={tileSize} onChange={setTileSize} />
        </div>

        {/* 结果：复用画廊网格（同一虚拟化 + 缩略图管线 + 查看器 + 合并展示） */}
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
              tile={GALLERY_TILE_PX[tileSize]}
              badges={badges}
              scrollTestId="search-grid-scroll"
            />
          )}
        </div>
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
