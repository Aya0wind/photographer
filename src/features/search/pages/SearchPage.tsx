import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";

import {
  assetsPage,
  cameraList,
  formatList,
  lensList,
  type AssetCameraCount,
  type AssetDto,
  type AssetFilters,
  type AssetFormatCount,
  type AssetKind,
  type AssetLensCount,
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
import { useSemanticSearch } from "@/features/ai/useSemanticSearch";
import {
  loadSemanticHistory,
  recordSemanticQuery,
} from "@/features/ai/semanticHistory";
import SemanticResultsView, {
  SemanticQueryInput,
} from "@/features/ai/SemanticResultsView";

/**
 * 搜索页（M4 二轮重设计，参考 immich 筛选面板）：
 * - 顶部紧凑筛选条：模式切换（条件/语义）+「筛选」按钮（展开下方筛选面板，默认收起；
 *   有激活条件时按钮带计数徽标）+ 结果计数 + 三档尺寸
 * - 筛选面板（两行网格）：类型/相机/镜头/格式（勾选下拉）、方向/闪光灯/GPS（分段）、
 *   焦段/ISO/光圈/快门/文件大小（min-max 数字输入）、日期范围（原生 date + 快捷段）
 * - 激活条件 chips 显示在筛选条下（每个可单独 ×，一键清空全部）
 * - 条件变更即时查询（300ms 防抖，序列化键防抖）；结果 keyset 追加加载
 * - RAW+JPG 合并展示与画廊同开关；语义模式（输入/历史/角标）保持不变
 */

const PAGE_LIMIT = 100;
const DEBOUNCE_MS = 300;

/** 类型两档（M3 二轮）：RAW 归入「照片」档（RAW 也是照片），不再单列 */
const KIND_OPTIONS: ReadonlyArray<{ value: "all" | "photo" | "video"; labelKey: string }> = [
  { value: "all", labelKey: "search.kind.all" },
  { value: "photo", labelKey: "search.kind.photo" },
  { value: "video", labelKey: "search.kind.video" },
];

type OrientationFilter = "all" | "landscape" | "portrait";
type FlashFilter = "all" | "on" | "off";
type GpsFilter = "all" | "yes" | "no";

const ORIENTATION_OPTIONS: ReadonlyArray<{ value: OrientationFilter; labelKey: string }> = [
  { value: "all", labelKey: "search.orientation.all" },
  { value: "landscape", labelKey: "search.orientation.landscape" },
  { value: "portrait", labelKey: "search.orientation.portrait" },
];
const FLASH_OPTIONS: ReadonlyArray<{ value: FlashFilter; labelKey: string }> = [
  { value: "all", labelKey: "search.flash.all" },
  { value: "on", labelKey: "search.flash.on" },
  { value: "off", labelKey: "search.flash.off" },
];
const GPS_OPTIONS: ReadonlyArray<{ value: GpsFilter; labelKey: string }> = [
  { value: "all", labelKey: "search.gps.all" },
  { value: "yes", labelKey: "search.gps.yes" },
  { value: "no", labelKey: "search.gps.no" },
];

/** 筛选面板全部输入（数字区间为原始字符串，构建 filters 时校验）；序列化键即防抖键 */
export interface SearchInputs {
  kind: "all" | "photo" | "video";
  from: string;
  to: string;
  cameras: string[];
  lenses: string[];
  formats: string[];
  orientation: OrientationFilter;
  flash: FlashFilter;
  gps: GpsFilter;
  focalMin: string;
  focalMax: string;
  isoMin: string;
  isoMax: string;
  apertureMin: string;
  apertureMax: string;
  shutterMin: string;
  shutterMax: string;
  /** MB（构建 filters 时转字节） */
  sizeMin: string;
  sizeMax: string;
}

const EMPTY_INPUTS: SearchInputs = {
  kind: "all",
  from: "",
  to: "",
  cameras: [],
  lenses: [],
  formats: [],
  orientation: "all",
  flash: "all",
  gps: "all",
  focalMin: "",
  focalMax: "",
  isoMin: "",
  isoMax: "",
  apertureMin: "",
  apertureMax: "",
  shutterMin: "",
  shutterMax: "",
  sizeMin: "",
  sizeMax: "",
};

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

/** 数字输入字符串 → number；空串/非法 → undefined（不进 filters） */
function numOrUndefined(value: string): number | undefined {
  if (value.trim() === "") return undefined;
  const n = Number(value);
  return Number.isFinite(n) ? n : undefined;
}

/** MB 字符串 → 字节数（取整）；空/非法 → undefined */
function mbToBytes(value: string): number | undefined {
  const mb = numOrUndefined(value);
  return mb !== undefined ? Math.round(mb * 1024 * 1024) : undefined;
}

/** 全部输入 → AssetFilters（空条件=无过滤；相机仍单值契约传第一个） */
export function buildFilters(inputs: SearchInputs): AssetFilters {
  const filters: AssetFilters = {};
  const kinds = kindsOf(inputs.kind);
  if (kinds) filters.kinds = kinds;
  const after = dateToRfc3339(inputs.from, false);
  if (inputs.from && after) filters.capturedAfter = after;
  const before = dateToRfc3339(inputs.to, true);
  if (inputs.to && before) filters.capturedBefore = before;
  if (inputs.cameras.length > 0) filters.camera = inputs.cameras[0];
  if (inputs.lenses.length > 0) filters.lenses = inputs.lenses;
  if (inputs.formats.length > 0) filters.formats = inputs.formats;
  if (inputs.orientation !== "all") filters.orientation = inputs.orientation;
  if (inputs.flash !== "all") filters.hasFlash = inputs.flash === "on";
  if (inputs.gps !== "all") filters.hasGps = inputs.gps === "yes";
  const focalMin = numOrUndefined(inputs.focalMin);
  if (focalMin !== undefined) filters.focalMin = focalMin;
  const focalMax = numOrUndefined(inputs.focalMax);
  if (focalMax !== undefined) filters.focalMax = focalMax;
  const isoMin = numOrUndefined(inputs.isoMin);
  if (isoMin !== undefined) filters.isoMin = isoMin;
  const isoMax = numOrUndefined(inputs.isoMax);
  if (isoMax !== undefined) filters.isoMax = isoMax;
  const apertureMin = numOrUndefined(inputs.apertureMin);
  if (apertureMin !== undefined) filters.apertureMin = apertureMin;
  const apertureMax = numOrUndefined(inputs.apertureMax);
  if (apertureMax !== undefined) filters.apertureMax = apertureMax;
  const shutterMin = numOrUndefined(inputs.shutterMin);
  if (shutterMin !== undefined) filters.shutterMin = shutterMin;
  const shutterMax = numOrUndefined(inputs.shutterMax);
  if (shutterMax !== undefined) filters.shutterMax = shutterMax;
  const sizeMin = mbToBytes(inputs.sizeMin);
  if (sizeMin !== undefined) filters.sizeMin = sizeMin;
  const sizeMax = mbToBytes(inputs.sizeMax);
  if (sizeMax !== undefined) filters.sizeMax = sizeMax;
  return filters;
}

/** 输入 → 序列化键（防抖用；字符串身份稳定） */
function serializeInputs(inputs: SearchInputs): string {
  return JSON.stringify(inputs);
}

/** 序列化键 → 输入（appendPage 以键重建条件，避免闭包过期） */
function parseFilters(key: string): AssetFilters {
  const inputs = { ...EMPTY_INPUTS, ...(JSON.parse(key) as Partial<SearchInputs>) };
  return buildFilters(inputs);
}

/** 多选清单切换（chips × 与下拉勾选共用） */
function toggleIn(list: string[], value: string): string[] {
  return list.includes(value) ? list.filter((v) => v !== value) : [...list, value];
}

// --- 面板小组件 ---------------------------------------------------------------------

const INPUT_CLASS =
  "rounded border border-edge bg-bg px-1.5 py-0.5 font-mono text-[11px] text-text-primary outline-none transition-colors focus:border-accent";

/** 分段单选（类型/方向/闪光灯/GPS 共用）；testId 透传到组与各按钮（-all/-on/…） */
function Segment<T extends string>({
  ariaLabel,
  value,
  options,
  onChange,
  testId,
}: {
  ariaLabel: string;
  value: T;
  options: ReadonlyArray<{ value: T; labelKey: string }>;
  onChange: (next: T) => void;
  testId: string;
}) {
  const { t } = useTranslation();
  return (
    <div
      className="flex shrink-0 items-center rounded-md border border-edge bg-bg p-0.5"
      role="radiogroup"
      aria-label={ariaLabel}
      data-testid={testId}
    >
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          role="radio"
          aria-checked={value === option.value}
          onClick={() => onChange(option.value)}
          className={`rounded px-2 py-0.5 text-[11px] font-medium transition-colors ${
            value === option.value
              ? "bg-accent text-black"
              : "text-text-secondary hover:text-text-primary"
          }`}
          data-testid={`${testId}-${option.value}`}
        >
          {t(option.labelKey)}
        </button>
      ))}
    </div>
  );
}

/** 勾选下拉（相机/镜头/格式共用；计数徽标 + 多选） */
function FilterDropdown({
  label,
  allLabel,
  emptyLabel,
  options,
  selected,
  onToggle,
  testId,
}: {
  label: string;
  allLabel: string;
  emptyLabel: string;
  options: Array<{ value: string; count: number }>;
  selected: string[];
  onToggle: (value: string) => void;
  testId: string;
}) {
  const [open, setOpen] = useState(false);
  return (
    <div className="relative min-w-0">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        aria-label={label}
        className={`flex items-center gap-1 rounded-md border px-2 py-0.5 text-[11px] transition-colors ${
          selected.length > 0 || open
            ? "border-accent text-accent"
            : "border-edge text-text-secondary hover:border-text-muted hover:text-text-primary"
        }`}
        data-testid={`${testId}-button`}
      >
        <span className="max-w-[110px] truncate">{selected.length > 0 ? selected[0] : allLabel}</span>
        {selected.length > 1 && <span className="font-mono">+{selected.length - 1}</span>}
        <svg
          viewBox="0 0 16 16"
          width="9"
          height="9"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.6"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          <path d="M3.5 6l4.5 4.5L12.5 6" />
        </svg>
      </button>
      {open && (
        <div
          className="sp-scroll absolute left-0 top-7 z-20 max-h-64 w-56 overflow-y-auto rounded-lg border border-edge bg-surface p-1 shadow-lg"
          data-testid={`${testId}-menu`}
        >
          {options.length === 0 ? (
            <p className="px-2 py-2 text-[11px] leading-relaxed text-text-muted">{emptyLabel}</p>
          ) : (
            options.map((option) => (
              <label
                key={option.value}
                className="flex cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-left transition-colors hover:bg-panel/40"
                data-testid={`${testId}-option`}
                data-value={option.value}
                data-checked={selected.includes(option.value)}
              >
                <input
                  type="checkbox"
                  checked={selected.includes(option.value)}
                  onChange={() => onToggle(option.value)}
                  className="h-3 w-3 accent-[#F0A83C]"
                />
                <span
                  className="min-w-0 flex-1 truncate text-[11px] text-text-secondary"
                  title={option.value}
                >
                  {option.value}
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
  );
}

/** min-max 数字区间输入（焦段/ISO/光圈/快门/文件大小共用） */
function RangeField({
  label,
  unit,
  min,
  max,
  onMinChange,
  onMaxChange,
  minTestId,
  maxTestId,
  step,
}: {
  label: string;
  /** 后缀单位（mm/s/MB；光圈等单位并入 label 时省略） */
  unit?: string;
  min: string;
  max: string;
  onMinChange: (v: string) => void;
  onMaxChange: (v: string) => void;
  minTestId: string;
  maxTestId: string;
  step?: number;
}) {
  return (
    <label className="flex min-w-0 items-center gap-1.5 text-[11px] text-text-muted">
      <span className="shrink-0">{label}</span>
      <input
        type="number"
        step={step}
        min={0}
        value={min}
        onChange={(e) => onMinChange(e.target.value)}
        placeholder="min"
        aria-label={`${label} min`}
        className={`${INPUT_CLASS} w-16`}
        data-testid={minTestId}
      />
      <span className="shrink-0">–</span>
      <input
        type="number"
        step={step}
        min={0}
        value={max}
        onChange={(e) => onMaxChange(e.target.value)}
        placeholder="max"
        aria-label={`${label} max`}
        className={`${INPUT_CLASS} w-16`}
        data-testid={maxTestId}
      />
      {unit && <span className="shrink-0">{unit}</span>}
    </label>
  );
}

/** 面板内单个控件的行布局：左侧固定宽标签 + 右控件 */
function FieldRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex min-w-0 items-center gap-2">
      <span className="w-14 shrink-0 text-right text-[11px] text-text-muted">{label}</span>
      {children}
    </div>
  );
}

// --- 页面 ---------------------------------------------------------------------------

/** 激活条件 chip（key 唯一；patch = 移除该条件后的完整输入） */
interface ActiveChip {
  key: string;
  label: string;
  patch: SearchInputs;
}

/** 输入 → 激活条件 chips（数字区间拆为 ≥/≤ 两枚，可单独移除） */
function buildChips(inputs: SearchInputs, t: (key: string) => string): ActiveChip[] {
  const chips: ActiveChip[] = [];
  if (inputs.kind !== "all") {
    chips.push({ key: "kind", label: t(`search.kind.${inputs.kind}`), patch: { ...inputs, kind: "all" } });
  }
  if (inputs.from) {
    chips.push({ key: "from", label: `${t("search.dateFrom")} ${inputs.from}`, patch: { ...inputs, from: "" } });
  }
  if (inputs.to) {
    chips.push({ key: "to", label: `${t("search.dateTo")} ${inputs.to}`, patch: { ...inputs, to: "" } });
  }
  for (const camera of inputs.cameras) {
    chips.push({
      key: `camera:${camera}`,
      label: camera,
      patch: { ...inputs, cameras: inputs.cameras.filter((c) => c !== camera) },
    });
  }
  for (const lens of inputs.lenses) {
    chips.push({
      key: `lens:${lens}`,
      label: lens,
      patch: { ...inputs, lenses: inputs.lenses.filter((l) => l !== lens) },
    });
  }
  for (const format of inputs.formats) {
    chips.push({
      key: `format:${format}`,
      label: format,
      patch: { ...inputs, formats: inputs.formats.filter((f) => f !== format) },
    });
  }
  if (inputs.orientation !== "all") {
    chips.push({
      key: "orientation",
      label: t(`search.orientation.${inputs.orientation}`),
      patch: { ...inputs, orientation: "all" },
    });
  }
  if (inputs.flash !== "all") {
    chips.push({
      key: "flash",
      label: `${t("search.flash")} ${t(`search.flash.${inputs.flash}`)}`,
      patch: { ...inputs, flash: "all" },
    });
  }
  if (inputs.gps !== "all") {
    chips.push({
      key: "gps",
      label: `GPS ${t(`search.gps.${inputs.gps}`)}`,
      patch: { ...inputs, gps: "all" },
    });
  }
  const rangeChip = (
    key: string,
    label: string,
    min: string,
    max: string,
    minField: keyof SearchInputs,
    maxField: keyof SearchInputs,
  ) => {
    if (min) {
      chips.push({
        key: `${key}:min`,
        label: `≥${label}${min}`,
        patch: { ...inputs, [minField]: "" } as SearchInputs,
      });
    }
    if (max) {
      chips.push({
        key: `${key}:max`,
        label: `≤${label}${max}`,
        patch: { ...inputs, [maxField]: "" } as SearchInputs,
      });
    }
  };
  rangeChip("focal", "", inputs.focalMin, inputs.focalMax, "focalMin", "focalMax");
  if (inputs.isoMin) {
    chips.push({ key: "iso:min", label: `ISO ≥${inputs.isoMin}`, patch: { ...inputs, isoMin: "" } });
  }
  if (inputs.isoMax) {
    chips.push({ key: "iso:max", label: `ISO ≤${inputs.isoMax}`, patch: { ...inputs, isoMax: "" } });
  }
  rangeChip("aperture", "f/", inputs.apertureMin, inputs.apertureMax, "apertureMin", "apertureMax");
  rangeChip("shutter", "", inputs.shutterMin, inputs.shutterMax, "shutterMin", "shutterMax");
  rangeChip("size", "MB", inputs.sizeMin, inputs.sizeMax, "sizeMin", "sizeMax");
  return chips;
}

export default function SearchPage() {
  const { t } = useTranslation();

  // 模式：条件（元数据筛选）/ 语义（自然语言）
  const [mode, setMode] = useState<"filters" | "semantic">("filters");
  const semantic = useSemanticSearch();
  const [lastQuery, setLastQuery] = useState("");
  // 语义查询历史（最近 5 条，localStorage；run 即记录——去重置顶）
  const [history, setHistory] = useState<string[]>(() => loadSemanticHistory());

  function runSemantic(query: string): void {
    setLastQuery(query);
    setHistory(recordSemanticQuery(query));
    void semantic.run(query);
  }

  // 全部筛选输入（单对象；序列化键防抖）+ 面板开合
  const [inputs, setInputs] = useState<SearchInputs>(EMPTY_INPUTS);
  const patchInputs = useCallback((patch: Partial<SearchInputs>) => {
    setInputs((prev) => ({ ...prev, ...patch }));
  }, []);
  const [panelOpen, setPanelOpen] = useState(false);

  // 防抖键：原始值拼接（字符串，身份稳定）；条件未变不重查
  const rawKey = serializeInputs(inputs);
  const debouncedKey = useDebouncedValue(rawKey, DEBOUNCE_MS);
  const appliedFilters = useMemo<AssetFilters>(() => parseFilters(debouncedKey), [debouncedKey]);
  const appliedKey = debouncedKey;

  // 相机清单打开页面即拉一次；镜头/格式清单在筛选面板首次展开时拉（少打 IPC）
  const [cameraOptions, setCameraOptions] = useState<AssetCameraCount[]>([]);
  const [lensOptions, setLensOptions] = useState<AssetLensCount[]>([]);
  const [formatOptions, setFormatOptions] = useState<AssetFormatCount[]>([]);
  const listsFetchedRef = useRef(false);
  useEffect(() => {
    let cancelled = false;
    void cameraList().then((list) => {
      if (!cancelled) setCameraOptions(list);
    });
    return () => {
      cancelled = true;
    };
  }, []);
  const togglePanel = useCallback(() => {
    setPanelOpen((open) => {
      if (!open && !listsFetchedRef.current) {
        listsFetchedRef.current = true;
        void lensList().then(setLensOptions);
        void formatList().then(setFormatOptions);
      }
      return !open;
    });
  }, []);

  const chips = useMemo(() => buildChips(inputs, t), [inputs, t]);

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

  // RAW+JPG 合并（与画廊同开关）+ 日期分组；语义模式结果同样分组供查看器导航
  const mergeEnabled = useSettingsStore((s) => s.settings.gallery?.mergeRawJpg ?? true);
  const { cards, badges } = useMemo(
    () => mergeRawJpgCards(results, mergeEnabled),
    [results, mergeEnabled],
  );
  const groups = useMemo(() => groupAssetsByDate(cards), [cards]);
  const semanticGroups = useMemo(() => groupAssetsByDate(semantic.assets), [semantic.assets]);
  const { viewer, openAsset, closeViewer, navigateTo } = useAssetViewer(
    mode === "semantic" ? semanticGroups : groups,
  );

  // 三档方格尺寸（与画廊共享）
  const [tileSize, setTileSize] = useGalleryTileSize();

  function applyQuickRange(key: QuickRangeKey): void {
    const [qFrom, qTo] = quickRange(key);
    patchInputs({ from: qFrom, to: qTo });
  }

  return (
    <div className="h-full" data-testid="search-page">
      {/* 内容区：水平居中 + 对称 padding（与画廊一致） */}
      <div
        className="mx-auto flex h-full w-full max-w-[1600px] flex-col px-6"
        data-testid="search-content"
      >
        {/* 顶部筛选条：模式切换 + 筛选按钮（条件模式）/ 语义输入（语义模式） */}
        <div className="flex h-11 shrink-0 items-center gap-3 border-b border-edge" data-testid="search-filters">
          {/* 模式分段：条件 / 语义 */}
          <div
            className="flex shrink-0 items-center rounded-md border border-edge bg-bg p-0.5"
            role="radiogroup"
            aria-label={t("search.mode")}
            data-testid="search-mode"
          >
            {(["filters", "semantic"] as const).map((option) => (
              <button
                key={option}
                type="button"
                role="radio"
                aria-checked={mode === option}
                onClick={() => setMode(option)}
                className={`rounded px-2 py-0.5 text-[11px] font-medium transition-colors ${
                  mode === option
                    ? "bg-accent text-black"
                    : "text-text-secondary hover:text-text-primary"
                }`}
                data-testid={`search-mode-${option}`}
              >
                {t(`search.mode.${option}`)}
              </button>
            ))}
          </div>

          {mode === "semantic" ? (
            <>
              <SemanticQueryInput
                busy={semantic.status === "loading"}
                onRun={runSemantic}
              />
              <span
                className="shrink-0 rounded-full bg-panel px-2 py-0.5 font-mono text-[11px] tabular-nums text-text-secondary"
                data-testid="search-count"
              >
                {semantic.status === "loading"
                  ? t("search.loading")
                  : t("search.count", { count: semantic.assets.length })}
              </span>
            </>
          ) : (
            <>
              {/* 筛选按钮：展开/收起面板；激活条件计数徽标 */}
              <button
                type="button"
                onClick={togglePanel}
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

              {/* 计数徽标 */}
              <span
                className="ml-auto shrink-0 rounded-full bg-panel px-2 py-0.5 font-mono text-[11px] tabular-nums text-text-secondary"
                data-testid="search-count"
              >
                {queryState === "loading" ? t("search.loading") : t("search.count", { count: results.length })}
              </span>

              {/* 三档尺寸（与画廊共享） */}
              <TileSizeSwitch value={tileSize} onChange={setTileSize} />
            </>
          )}
        </div>

        {/* 筛选面板（默认收起；两行网格：下拉/分段 + 数值区间 + 日期） */}
        {mode === "filters" && panelOpen && (
          <div
            className="shrink-0 border-b border-edge bg-bg/40 px-2 py-3"
            data-testid="search-filter-panel"
          >
            <div className="grid grid-cols-1 items-center gap-x-6 gap-y-2.5 md:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-4">
              <FieldRow label={t("search.kind")}>
                <Segment
                  ariaLabel={t("search.kind")}
                  value={inputs.kind}
                  options={KIND_OPTIONS}
                  onChange={(kind) => patchInputs({ kind })}
                  testId="search-kind"
                />
              </FieldRow>
              <FieldRow label={t("search.camera")}>
                <FilterDropdown
                  label={t("search.camera")}
                  allLabel={t("search.cameraAll")}
                  emptyLabel={t("search.cameraEmpty")}
                  options={cameraOptions.map((o) => ({ value: o.camera, count: o.count }))}
                  selected={inputs.cameras}
                  onToggle={(camera) => patchInputs({ cameras: toggleIn(inputs.cameras, camera) })}
                  testId="search-camera"
                />
              </FieldRow>
              <FieldRow label={t("search.lens")}>
                <FilterDropdown
                  label={t("search.lens")}
                  allLabel={t("search.lensAll")}
                  emptyLabel={t("search.lensEmpty")}
                  options={lensOptions.map((o) => ({ value: o.lens, count: o.count }))}
                  selected={inputs.lenses}
                  onToggle={(lens) => patchInputs({ lenses: toggleIn(inputs.lenses, lens) })}
                  testId="search-lens"
                />
              </FieldRow>
              <FieldRow label={t("search.format")}>
                <FilterDropdown
                  label={t("search.format")}
                  allLabel={t("search.formatAll")}
                  emptyLabel={t("search.formatEmpty")}
                  options={formatOptions.map((o) => ({ value: o.format, count: o.count }))}
                  selected={inputs.formats}
                  onToggle={(format) => patchInputs({ formats: toggleIn(inputs.formats, format) })}
                  testId="search-format"
                />
              </FieldRow>
              <FieldRow label={t("search.orientation")}>
                <Segment
                  ariaLabel={t("search.orientation")}
                  value={inputs.orientation}
                  options={ORIENTATION_OPTIONS}
                  onChange={(orientation) => patchInputs({ orientation })}
                  testId="search-orientation"
                />
              </FieldRow>
              <FieldRow label={t("search.flash")}>
                <Segment
                  ariaLabel={t("search.flash")}
                  value={inputs.flash}
                  options={FLASH_OPTIONS}
                  onChange={(flash) => patchInputs({ flash })}
                  testId="search-flash"
                />
              </FieldRow>
              <FieldRow label={t("search.gps")}>
                <Segment
                  ariaLabel={t("search.gps")}
                  value={inputs.gps}
                  options={GPS_OPTIONS}
                  onChange={(gps) => patchInputs({ gps })}
                  testId="search-gps"
                />
              </FieldRow>
              <FieldRow label={t("search.focal")}>
                <RangeField
                  label=""
                  unit="mm"
                  min={inputs.focalMin}
                  max={inputs.focalMax}
                  onMinChange={(v) => patchInputs({ focalMin: v })}
                  onMaxChange={(v) => patchInputs({ focalMax: v })}
                  minTestId="search-focal-min"
                  maxTestId="search-focal-max"
                />
              </FieldRow>
              <FieldRow label={t("search.iso")}>
                <RangeField
                  label=""
                  min={inputs.isoMin}
                  max={inputs.isoMax}
                  onMinChange={(v) => patchInputs({ isoMin: v })}
                  onMaxChange={(v) => patchInputs({ isoMax: v })}
                  minTestId="search-iso-min"
                  maxTestId="search-iso-max"
                />
              </FieldRow>
              <FieldRow label={t("search.aperture")}>
                <RangeField
                  label=""
                  min={inputs.apertureMin}
                  max={inputs.apertureMax}
                  step={0.1}
                  onMinChange={(v) => patchInputs({ apertureMin: v })}
                  onMaxChange={(v) => patchInputs({ apertureMax: v })}
                  minTestId="search-aperture-min"
                  maxTestId="search-aperture-max"
                />
              </FieldRow>
              <FieldRow label={t("search.shutter")}>
                <RangeField
                  label=""
                  unit="s"
                  step={0.001}
                  min={inputs.shutterMin}
                  max={inputs.shutterMax}
                  onMinChange={(v) => patchInputs({ shutterMin: v })}
                  onMaxChange={(v) => patchInputs({ shutterMax: v })}
                  minTestId="search-shutter-min"
                  maxTestId="search-shutter-max"
                />
              </FieldRow>
              <FieldRow label={t("search.fileSize")}>
                <RangeField
                  label=""
                  unit="MB"
                  min={inputs.sizeMin}
                  max={inputs.sizeMax}
                  onMinChange={(v) => patchInputs({ sizeMin: v })}
                  onMaxChange={(v) => patchInputs({ sizeMax: v })}
                  minTestId="search-size-min"
                  maxTestId="search-size-max"
                />
              </FieldRow>
              {/* 日期范围：原生 date input（深色适配）+ 快捷段 */}
              <FieldRow label={t("search.dateFrom")}>
                <input
                  type="date"
                  value={inputs.from}
                  onChange={(e) => patchInputs({ from: e.target.value })}
                  aria-label={t("search.dateFrom")}
                  className={`${INPUT_CLASS} [color-scheme:dark]`}
                  data-testid="search-from"
                />
              </FieldRow>
              <FieldRow label={t("search.dateTo")}>
                <input
                  type="date"
                  value={inputs.to}
                  onChange={(e) => patchInputs({ to: e.target.value })}
                  aria-label={t("search.dateTo")}
                  className={`${INPUT_CLASS} [color-scheme:dark]`}
                  data-testid="search-to"
                />
              </FieldRow>
              <div className="flex items-center gap-0.5" data-testid="search-quick-ranges">
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
            </div>
          </div>
        )}

        {/* 激活条件 chips（面板收起也可见；单个 × 移除 / 一键清空） */}
        {mode === "filters" && chips.length > 0 && (
          <div
            className="sp-scroll flex shrink-0 flex-wrap items-center gap-1.5 overflow-x-auto border-b border-edge/60 py-1.5"
            data-testid="search-filter-chips"
          >
            {chips.map((chip) => (
              <span
                key={chip.key}
                className="flex max-w-[220px] items-center gap-1 rounded-full border border-edge bg-surface pl-2 pr-1 text-[11px] text-text-secondary"
                data-testid="search-chip"
                data-chip={chip.key}
              >
                <span className="truncate" title={chip.label}>
                  {chip.label}
                </span>
                <button
                  type="button"
                  onClick={() => setInputs(chip.patch)}
                  aria-label={`${chip.label} ×`}
                  className="rounded-full px-1 text-text-muted transition-colors hover:bg-panel hover:text-text-primary"
                  data-testid="search-chip-remove"
                >
                  ×
                </button>
              </span>
            ))}
            <button
              type="button"
              onClick={() => setInputs(EMPTY_INPUTS)}
              className="ml-1 shrink-0 rounded border border-edge px-1.5 py-0.5 text-[11px] text-text-muted transition-colors hover:border-red-400 hover:text-red-400"
              data-testid="search-clear-all"
            >
              {t("search.chipsClearAll")}
            </button>
          </div>
        )}

        {/* 语义查询历史（最近 5 条）：点击重搜（记录去重置顶见 recordSemanticQuery） */}
        {mode === "semantic" && history.length > 0 && (
          <div
            className="sp-scroll flex h-8 shrink-0 items-center gap-1.5 overflow-x-auto border-b border-edge/60"
            data-testid="semantic-history"
          >
            <span className="shrink-0 text-[11px] text-text-muted">
              {t("search.semantic.history")}
            </span>
            {history.map((q) => (
              <button
                key={q}
                type="button"
                onClick={() => runSemantic(q)}
                title={q}
                className="max-w-[160px] shrink-0 truncate rounded-full border border-edge px-2 py-0.5 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
                data-testid="semantic-history-item"
                data-query={q}
              >
                {q}
              </button>
            ))}
          </div>
        )}

        {/* 结果：复用画廊网格（同一虚拟化 + 缩略图管线 + 查看器 + 合并展示）；
            语义模式走 SemanticResultsView（进度/未就绪引导/相似度角标） */}
        <div className="relative min-h-0 flex-1">
          {mode === "semantic" ? (
            <SemanticResultsView
              status={semantic.status}
              assets={semantic.assets}
              scores={semantic.scores}
              onOpenAsset={openAsset}
              onRetry={() => void semantic.run(lastQuery)}
            />
          ) : queryState === "loading" && results.length === 0 ? (
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
