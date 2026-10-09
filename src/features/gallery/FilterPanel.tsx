import DateRangePicker from "./components/DateRangePicker";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Link } from "react-router";
import { useTranslation } from "react-i18next";
import { useAiStore } from "@/stores/aiStore";

import {
  albumList,
  cameraList,
  formatList,
  lensList,
  photoLibraryList,
  type AlbumDto,
  type AssetCameraCount,
  type AssetFilters,
  type AssetFormatCount,
  type AssetLensCount,
  type PhotoLibrary,
} from "@/ipc/api";
import { COLOR_DOT_CLASS, COLOR_DOT_RING, COLOR_LABELS, type ColorLabel } from "./lib/colorLabels";

/**
 * 筛选面板（M4.5 自 SearchPage 抽取的共享组件，画廊合并后唯一消费方）：
 * - 状态：SearchInputs 单对象（数字区间为原始字符串，构建时校验）；序列化键即防抖键
 * - UI：网格——相机/镜头/格式（勾选下拉）、方向/闪光灯/GPS（分段）、
 *   焦段/ISO/光圈/快门/文件大小（min-max）、日期范围+快捷段、相册（单选下拉）、
 *   颜色标签（B1 五色点单选）/已拒绝（B1 三态）
 * - 清单：cameraList/lensList/albumList 面板展开时读取；格式在常用栏或面板挂载时读取
 * - chips：激活条件清单（每个可单独移除 + 一键清空），由 FilterChipsRow 渲染
 */

type OrientationFilter = "all" | "landscape" | "portrait";
type FlashFilter = "all" | "on" | "off" | "unknown";
type GpsFilter = "all" | "yes" | "no";
/** 颜色标签筛选：all=不限；五色单选 */
export type ColorFilter = "all" | ColorLabel;
/** 拒绝旗标三态：all=不限 / yes=仅已拒绝 / no=仅未拒绝 */
export type RejectedFilter = "all" | "yes" | "no";
/** 闭眼风险（C 阶段 AI 选片）：none=不限；closed/maybe 与后端 filters.eyes 单值对应（互斥） */
export type AiEyesFilter = "none" | "closed" | "maybe";

const ORIENTATION_OPTIONS: ReadonlyArray<{ value: OrientationFilter; labelKey: string }> = [
  { value: "all", labelKey: "search.orientation.all" },
  { value: "landscape", labelKey: "search.orientation.landscape" },
  { value: "portrait", labelKey: "search.orientation.portrait" },
];
const FLASH_OPTIONS: ReadonlyArray<{ value: FlashFilter; labelKey: string }> = [
  { value: "all", labelKey: "search.flash.all" },
  { value: "on", labelKey: "search.flash.on" },
  { value: "off", labelKey: "search.flash.off" },
  { value: "unknown", labelKey: "search.flash.unknown" },
];
const GPS_OPTIONS: ReadonlyArray<{ value: GpsFilter; labelKey: string }> = [
  { value: "all", labelKey: "search.gps.all" },
  { value: "yes", labelKey: "search.gps.yes" },
  { value: "no", labelKey: "search.gps.no" },
];
const REJECTED_OPTIONS: ReadonlyArray<{ value: RejectedFilter; labelKey: string }> = [
  { value: "all", labelKey: "search.rejected.all" },
  { value: "yes", labelKey: "search.rejected.yes" },
  { value: "no", labelKey: "search.rejected.no" },
];

/** 筛选面板全部输入 */
export interface SearchInputs {
  favoriteOnly: boolean;
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
  /** 所属相册（单选；name 随行携带供 chips 直接展示，序列化进防抖键） */
  album: { id: number; name: string } | null;
  /** 所属照片库多选（OR，2026-10-09 单库多照片库：画廊默认全局跨库混排，
   *  按需过滤；name 随行携带供 chips/下拉展示） */
  libraries: { id: string; name: string }[];
  /** 颜色标签（B1，LR 五色单选；all=不限） */
  color: ColorFilter;
  /** 拒绝旗标三态（B1；all=不限） */
  rejected: RejectedFilter;
  /** 闭眼风险（C；none=不限） */
  aiEyes: AiEyesFilter;
  /** 疑似失焦（C；false=不限） */
  aiBlur: boolean;
}

export const EMPTY_INPUTS: SearchInputs = {
  favoriteOnly: false,
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
  album: null,
  libraries: [],
  color: "all",
  rejected: "all",
  aiEyes: "none",
  aiBlur: false,
};

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
 * 日期不合法返回 undefined（不进 filters）。
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

/** 全部输入 → AssetFilters（空条件=无过滤；各条件之间 AND，同字段多选 OR） */
export function buildFilters(inputs: SearchInputs): AssetFilters {
  const filters: AssetFilters = {};
  if (inputs.favoriteOnly) filters.ratingMin = 5;
  const after = dateToRfc3339(inputs.from, false);
  if (inputs.from && after) filters.capturedAfter = after;
  const before = dateToRfc3339(inputs.to, true);
  if (inputs.to && before) filters.capturedBefore = before;
  if (inputs.cameras.length > 0) filters.cameras = inputs.cameras;
  if (inputs.lenses.length > 0) filters.lenses = inputs.lenses;
  if (inputs.formats.length > 0) filters.formats = inputs.formats;
  if (inputs.orientation !== "all") filters.orientation = inputs.orientation;
  if (inputs.flash !== "all") filters.flash = inputs.flash;
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
  if (inputs.album !== null) filters.albumId = inputs.album.id;
  if (inputs.libraries.length > 0) filters.libraryIds = inputs.libraries.map((l) => l.id);
  if (inputs.color !== "all") filters.colorLabel = inputs.color;
  if (inputs.rejected !== "all") filters.rejected = inputs.rejected === "yes";
  if (inputs.aiEyes !== "none") filters.eyes = inputs.aiEyes;
  if (inputs.aiBlur) filters.blur = "soft";
  return filters;
}

/** 输入 → 序列化键（防抖用；字符串身份稳定） */
export function serializeInputs(inputs: SearchInputs): string {
  return JSON.stringify(inputs);
}

/** 序列化键 → 输入（补页以键重建条件，避免闭包过期） */
export function parseInputs(key: string): SearchInputs {
  return { ...EMPTY_INPUTS, ...(JSON.parse(key) as Partial<SearchInputs>) };
}

/** 序列化键 → filters（appendPage 重建负载用） */
export function filtersFromKey(key: string): AssetFilters {
  return buildFilters(parseInputs(key));
}

/** 多选清单切换（chips × 与下拉勾选共用） */
function toggleIn(list: string[], value: string): string[] {
  return list.includes(value) ? list.filter((v) => v !== value) : [...list, value];
}

/** 是否存在激活条件（默认态判定：无任何 chip = 默认全部资产） */
export function hasActiveFilters(inputs: SearchInputs): boolean {
  return serializeInputs(inputs) !== serializeInputs(EMPTY_INPUTS);
}

// --- 面板小组件 ---------------------------------------------------------------------

const INPUT_CLASS =
  "h-7 rounded-md border border-edge bg-panel/55 px-2 font-mono text-[11px] text-text-primary placeholder:text-text-muted/60 outline-none transition-colors hover:border-text-muted/70 focus:border-accent focus:ring-1 focus:ring-accent/20";

/** 分段单选（方向/闪光灯/GPS 共用）；testId 透传到组与各按钮 */
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
      className="flex h-7 shrink-0 items-center rounded-md border border-edge bg-panel/55 p-0.5"
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

/** 颜色标签单选（B1，LR 五色点）：全部(不限) + 五色点，aria-checked 单选语义 */
function ColorSegment({
  value,
  onChange,
  testId,
}: {
  value: ColorFilter;
  onChange: (next: ColorFilter) => void;
  testId: string;
}) {
  const { t } = useTranslation();
  return (
    <div
      className="flex h-7 shrink-0 items-center gap-0.5 rounded-md border border-edge bg-panel/55 p-0.5"
      role="radiogroup"
      aria-label={t("search.color")}
      data-testid={testId}
    >
      <button
        type="button"
        role="radio"
        aria-checked={value === "all"}
        onClick={() => onChange("all")}
        className={`rounded px-2 py-0.5 text-[11px] font-medium transition-colors ${
          value === "all" ? "bg-accent text-black" : "text-text-secondary hover:text-text-primary"
        }`}
        data-testid={`${testId}-all`}
      >
        {t("search.colorAll")}
      </button>
      {COLOR_LABELS.map((label) => (
        <button
          key={label}
          type="button"
          role="radio"
          aria-checked={value === label}
          aria-label={t(`gallery.color.${label}`)}
          title={t(`gallery.color.${label}`)}
          onClick={() => onChange(value === label ? "all" : label)}
          className={`rounded p-1 transition-colors ${
            value === label ? "bg-accent/25" : "hover:bg-panel"
          }`}
          data-testid={`${testId}-${label}`}
        >
          <span
            className={`block h-3 w-3 rounded-full ${COLOR_DOT_CLASS[label]} ${COLOR_DOT_RING} ${
              value === label ? "outline outline-1 outline-accent" : ""
            }`}
            aria-hidden="true"
          />
        </button>
      ))}
    </div>
  );
}

/** 勾选下拉（相机/镜头/格式/照片库共用；计数徽标 + 多选）。
 *  值与展示名不同时传 labelOf（照片库 id → 名称）；缺省展示值本身。 */
function FilterDropdown({
  label,
  allLabel,
  emptyLabel,
  options,
  selected,
  onToggle,
  onClear,
  testId,
  labelOf,
}: {
  label: string;
  allLabel: string;
  emptyLabel: string;
  options: Array<{ value: string; count: number }>;
  selected: string[];
  onToggle: (value: string) => void;
  onClear: () => void;
  testId: string;
  labelOf?: (value: string) => string;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!open) return;
    const closeOutside = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    window.addEventListener("pointerdown", closeOutside);
    window.addEventListener("keydown", closeOnEscape);
    return () => {
      window.removeEventListener("pointerdown", closeOutside);
      window.removeEventListener("keydown", closeOnEscape);
    };
  }, [open]);
  return (
    <div ref={rootRef} className="relative min-w-0">
      <div
        className={`inline-flex h-7 min-w-32 items-stretch overflow-hidden rounded-md border bg-panel/55 transition-colors ${
          selected.length > 0 || open
            ? "border-accent bg-accent/10 text-accent"
            : "border-edge text-text-secondary hover:border-text-muted hover:text-text-primary"
        }`}
      >
        <button
          type="button"
          onClick={() => setOpen((v) => !v)}
          aria-expanded={open}
          aria-label={label}
          className="flex min-w-0 flex-1 items-center justify-between gap-1.5 px-2 text-[11px]"
          data-testid={`${testId}-button`}
        >
          <span className="max-w-[116px] truncate">
            {selected.length > 0 ? labelOf?.(selected[0]) ?? selected[0] : allLabel}
          </span>
          {selected.length > 1 && <span className="rounded bg-accent/15 px-1 font-mono">+{selected.length - 1}</span>}
          <svg viewBox="0 0 16 16" width="9" height="9" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M3.5 6l4.5 4.5L12.5 6" />
          </svg>
        </button>
        {selected.length > 0 && (
          <button
            type="button"
            onClick={onClear}
            aria-label={`${label} ×`}
            title={`${label} ×`}
            className="flex w-7 shrink-0 items-center justify-center border-l border-accent/25 text-sm text-text-muted transition-colors hover:bg-accent/15 hover:text-accent"
            data-testid={`${testId}-clear`}
          >
            ×
          </button>
        )}
      </div>
      {open && (
        <div
          className="sp-scroll absolute left-0 top-8 z-20 max-h-72 w-64 overflow-y-auto rounded-xl border border-edge bg-surface p-1.5 shadow-2xl shadow-black/40"
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
                  className="peer sr-only"
                />
                <span
                  className={`flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded border transition-colors ${
                    selected.includes(option.value)
                      ? "border-accent bg-accent text-black"
                      : "border-text-muted/70 bg-bg"
                  }`}
                  aria-hidden="true"
                >
                  {selected.includes(option.value) && (
                    <svg viewBox="0 0 12 12" width="9" height="9" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
                      <path d="M2.2 6.2l2.2 2.2 5.2-5.1" />
                    </svg>
                  )}
                </span>
                <span
                  className="min-w-0 flex-1 truncate text-[11px] text-text-secondary"
                  title={labelOf?.(option.value) ?? option.value}
                >
                  {labelOf?.(option.value) ?? option.value}
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

/** 相册单选下拉（所属相册维度；FilterDropdown 的单选版） */
function AlbumDropdown({
  albums,
  selected,
  onSelect,
  testId,
}: {
  albums: AlbumDto[];
  selected: { id: number; name: string } | null;
  onSelect: (next: { id: number; name: string } | null) => void;
  testId: string;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!open) return;
    const closeOutside = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    window.addEventListener("pointerdown", closeOutside);
    window.addEventListener("keydown", closeOnEscape);
    return () => {
      window.removeEventListener("pointerdown", closeOutside);
      window.removeEventListener("keydown", closeOnEscape);
    };
  }, [open]);
  return (
    <div ref={rootRef} className="relative min-w-0">
      <div
        className={`inline-flex h-7 min-w-32 items-stretch overflow-hidden rounded-md border bg-panel/55 transition-colors ${
          selected !== null || open
            ? "border-accent bg-accent/10 text-accent"
            : "border-edge text-text-secondary hover:border-text-muted hover:text-text-primary"
        }`}
      >
        <button
          type="button"
          onClick={() => setOpen((v) => !v)}
          aria-expanded={open}
          className="flex min-w-0 flex-1 items-center justify-between gap-1.5 px-2 text-[11px]"
          data-testid={`${testId}-button`}
        >
          <span className="max-w-[116px] truncate">{selected ? selected.name : t("search.albumAll")}</span>
          <svg viewBox="0 0 16 16" width="9" height="9" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M3.5 6l4.5 4.5L12.5 6" />
          </svg>
        </button>
        {selected !== null && (
          <button
            type="button"
            onClick={() => onSelect(null)}
            aria-label={`${t("search.album")} ×`}
            title={`${t("search.album")} ×`}
            className="flex w-7 shrink-0 items-center justify-center border-l border-accent/25 text-sm text-text-muted transition-colors hover:bg-accent/15 hover:text-accent"
            data-testid={`${testId}-clear`}
          >
            ×
          </button>
        )}
      </div>
      {open && (
        <div
          className="sp-scroll absolute left-0 top-8 z-20 max-h-72 w-64 overflow-y-auto rounded-xl border border-edge bg-surface p-1.5 shadow-2xl shadow-black/40"
          data-testid={`${testId}-menu`}
        >
          {albums.length === 0 ? (
            <p className="px-2 py-2 text-[11px] leading-relaxed text-text-muted">
              {t("search.albumEmpty")}
            </p>
          ) : (
            albums.map((album) => {
              const active = selected?.id === album.id;
              return (
                <button
                  key={album.id}
                  type="button"
                  onClick={() => onSelect(active ? null : { id: album.id, name: album.name })}
                  className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left transition-colors hover:bg-panel/40"
                  data-testid={`${testId}-option`}
                  data-album-id={album.id}
                  data-selected={active}
                >
                  <span
                    className={`flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded-full border transition-colors ${
                      active ? "border-accent bg-accent" : "border-text-muted/70"
                    }`}
                    aria-hidden="true"
                  >
                    {active && <span className="h-1.5 w-1.5 rounded-full bg-black" />}
                  </span>
                  <span className="min-w-0 flex-1 truncate text-[11px] text-text-secondary" title={album.name}>
                    {album.name}
                  </span>
                  <span className="shrink-0 rounded bg-panel px-1.5 py-0.5 font-mono text-[10px] tabular-nums text-text-muted">
                    {album.itemCount}
                  </span>
                </button>
              );
            })
          )}
        </div>
      )}
    </div>
  );
}

/** min-max 数字区间输入（焦段/ISO/光圈/快门/文件大小共用） */function RangeField({
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
  unit?: string;
  min: string;
  max: string;
  onMinChange: (v: string) => void;
  onMaxChange: (v: string) => void;
  minTestId: string;
  maxTestId: string;
  step?: number;
}) {
  const { t } = useTranslation();
  const updateNumeric = (value: string, update: (next: string) => void) => {
    if (/^\d*(?:\.\d*)?$/.test(value)) update(value);
  };
  return (
    <label className="flex min-w-0 items-center gap-1.5 text-[11px] text-text-muted">
      <span className="sr-only">{label}</span>
      <input
        type="text"
        inputMode="decimal"
        data-step={step}
        value={min}
        onChange={(e) => updateNumeric(e.target.value, onMinChange)}
        placeholder={t("search.rangeMin")}
        aria-label={`${label} ${t("search.rangeMin")}`}
        className={`${INPUT_CLASS} w-[68px] text-right tabular-nums`}
        data-testid={minTestId}
      />
      <span className="shrink-0">–</span>
      <input
        type="text"
        inputMode="decimal"
        data-step={step}
        value={max}
        onChange={(e) => updateNumeric(e.target.value, onMaxChange)}
        placeholder={t("search.rangeMax")}
        aria-label={`${label} ${t("search.rangeMax")}`}
        className={`${INPUT_CLASS} w-[68px] text-right tabular-nums`}
        data-testid={maxTestId}
      />
      {unit && <span className="shrink-0">{unit}</span>}
    </label>
  );
}

/** 面板内单个控件的行布局：左侧固定宽标签 + 右控件 */
function FieldRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex min-h-9 min-w-0 items-center gap-2 rounded-lg border border-transparent px-2 transition-colors hover:border-edge/60 hover:bg-panel/25">
      <span className="w-14 shrink-0 text-right text-[11px] font-medium text-text-muted">{label}</span>
      {children}
    </div>
  );
}

// --- 面板与 chips 行 ----------------------------------------------------------------

/** 按库内实际格式筛选，允许同时选择多个格式。 */
function FormatFilter({ inputs, onPatch }: { inputs: SearchInputs; onPatch: (patch: Partial<SearchInputs>) => void }) {
  const { t } = useTranslation();
  const [options, setOptions] = useState<AssetFormatCount[]>([]);
  useEffect(() => {
    let cancelled = false;
    void formatList().then((list) => { if (!cancelled) setOptions(list); });
    return () => { cancelled = true; };
  }, []);
  return <FilterDropdown label={t("search.format")} allLabel={t("search.formatAll")} emptyLabel={t("search.formatEmpty")}
    options={options.map((o) => ({ value: o.format, count: o.count }))} selected={inputs.formats}
    onToggle={(format) => onPatch({ formats: toggleIn(inputs.formats, format) })}
    onClear={() => onPatch({ formats: [] })} testId="search-format" />;
}

/** 按照片库筛选（多选 OR，2026-10-09 单库多照片库）：画廊默认全局跨库混排，
 *  选中后 filters.libraryIds 收窄到所选库。清单 photo_library_list（失败回退
 *  空态文案——后端未连接时自然降级）；名称展示 = 清单 ∪ inputs 随行名
 *  （清单未就位时已选项也能显示库名而非裸 id）。 */
function LibraryFilter({ inputs, onPatch }: { inputs: SearchInputs; onPatch: (patch: Partial<SearchInputs>) => void }) {
  const { t } = useTranslation();
  const [libraries, setLibraries] = useState<PhotoLibrary[]>([]);
  useEffect(() => {
    let cancelled = false;
    void photoLibraryList().then((list) => { if (!cancelled) setLibraries(list); });
    return () => { cancelled = true; };
  }, []);
  const names = new Map<string, string>([
    ...inputs.libraries.map((l) => [l.id, l.name] as const),
    ...libraries.map((l) => [l.id, l.name] as const),
  ]);
  return <FilterDropdown
    label={t("search.library")}
    allLabel={t("search.libraryAll")}
    emptyLabel={t("search.libraryEmpty")}
    options={libraries.map((l) => ({ value: l.id, count: l.assetCount }))}
    selected={inputs.libraries.map((l) => l.id)}
    labelOf={(id) => names.get(id) ?? id}
    onToggle={(id) => {
      const has = inputs.libraries.some((l) => l.id === id);
      onPatch({
        libraries: has
          ? inputs.libraries.filter((l) => l.id !== id)
          : [...inputs.libraries, { id, name: names.get(id) ?? id }],
      });
    }}
    onClear={() => onPatch({ libraries: [] })}
    testId="search-library"
  />;
}

function DateFilter({ inputs, onPatch }: { inputs: SearchInputs; onPatch: (patch: Partial<SearchInputs>) => void }) {
  const { t }=useTranslation();
  const ranges=()=>(["recent7","recent30","thisYear","lastYear"] as const).map(key=>{
    const [from,to]=quickRange(key);return {key,from,to,label:t(`search.quick.${key}`)};
  });
  return <DateRangePicker from={inputs.from} to={inputs.to} onApply={onPatch} ranges={ranges}/>;
}

export function ClearFiltersButton({ onClear, disabled=false, testId="search-reset-filters" }: {
  onClear: () => void; disabled?: boolean; testId?: string;
}) {
  const {t}=useTranslation();
  return <button type="button" disabled={disabled} onClick={onClear} data-testid={testId}
    className="h-8 shrink-0 rounded-md border border-accent/40 bg-accent/10 px-3 text-xs font-medium text-accent transition-colors hover:bg-accent/20 disabled:cursor-default disabled:opacity-40">
    {t("search.chipsClearAll")}
  </button>;
}

/** 图库常用条件常驻工具栏，面板只展示其余条件。 */
export function QuickFilterBar({
  inputs,
  onPatch,
}: {
  inputs: SearchInputs;
  onPatch: (patch: Partial<SearchInputs>) => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex min-h-11 shrink-0 flex-wrap items-center gap-x-3 gap-y-1.5 border-b border-edge/60 py-1.5" data-testid="gallery-quick-filters">
      <span className="shrink-0 text-[11px] font-medium text-text-muted">{t("search.format")}</span>
      <FormatFilter inputs={inputs} onPatch={onPatch} />
      <span className="h-5 w-px shrink-0 bg-edge" aria-hidden="true" />
      <span className="shrink-0 text-[11px] font-medium text-text-muted">{t("search.orientation")}</span>
      <Segment ariaLabel={t("search.orientation")} value={inputs.orientation} options={ORIENTATION_OPTIONS} onChange={(orientation) => onPatch({ orientation })} testId="search-orientation" />
      <span className="h-5 w-px shrink-0 bg-edge" aria-hidden="true" />
      <span className="shrink-0 text-[11px] font-medium text-text-muted">{t("search.gps")}</span>
      <Segment ariaLabel={t("search.gps")} value={inputs.gps} options={GPS_OPTIONS} onChange={(gps) => onPatch({ gps })} testId="search-gps" />
      <span className="h-5 w-px shrink-0 bg-edge" aria-hidden="true" />
      <button
        type="button"
        onClick={() => onPatch({ favoriteOnly: !inputs.favoriteOnly })}
        aria-pressed={inputs.favoriteOnly}
        className={`h-7 shrink-0 rounded-md border px-2.5 text-[11px] font-medium transition-colors ${inputs.favoriteOnly ? "border-amber-400 bg-amber-400/15 text-amber-400" : "border-edge text-text-secondary hover:border-amber-400 hover:text-amber-400"}`}
        data-testid="gallery-favorite-filter"
      >
        {t("gallery.favoriteOnly")}
      </button>

      <span className="shrink-0 text-[11px] font-medium text-text-muted">{t("search.date")}</span>
      <DateFilter inputs={inputs} onPatch={onPatch}/>
    </div>
  );
}

/** AI 标签区（C 阶段，AI 辅助选片）：闭眼两档（互斥，对应 filters.eyes 单值）+
 *  疑似失焦单档（filters.blur）。eyes 依赖「选片辅助」模型——未装置灰并提示去
 *  设置下载（?tab=ai 门禁跳转模式）；blur 算法内置恒可用。
 *  「分析中」提示：AI 索引通道有未完成项时沿用「结果可能不全」提示模式。 */
function AiTagsField({
  aiEyes,
  aiBlur,
  onPatch,
}: {
  aiEyes: AiEyesFilter;
  aiBlur: boolean;
  onPatch: (patch: Partial<SearchInputs>) => void;
}) {
  const { t } = useTranslation();
  const models = useAiStore((s) => s.models);
  const indexStatus = useAiStore((s) => s.indexStatus);
  const refreshIndexStatus = useAiStore((s) => s.refreshIndexStatus);

  // 模型清单（eyes 门禁）与索引账（分析中提示）：挂载拉一次；分析进度经
  // store 事件流自更新（indexTaskProgress 由 initAi 转发）
  const modelsLoaded = useAiStore((s) => s.modelsLoaded);
  const refresh = useAiStore((s) => s.refresh);
  useEffect(() => {
    if (!modelsLoaded) void refresh();
    void refreshIndexStatus();
  }, [modelsLoaded, refresh, refreshIndexStatus]);

  const eyesInstalled = models.some(
    (m) => m.feature === "selection" && (m.state === "done" || m.installed),
  );
  const aiRunning =
    indexStatus !== null && (indexStatus.ai.pending > 0 || indexStatus.ai.running > 0);

  const chipClass = (active: boolean, disabled = false): string =>
    `rounded-md border px-2 py-1 text-[11px] transition-colors ${
      active ? "border-accent bg-accent/10 text-accent" : "border-edge text-text-secondary hover:border-text-muted"
    } ${disabled ? "cursor-not-allowed opacity-40" : ""}`;

  return (
    <div className="flex min-w-0 flex-col gap-1">
      <div className="flex flex-wrap items-center gap-1.5" data-testid="search-ai-tags">
        <button
          type="button"
          aria-pressed={aiEyes === "closed"}
          disabled={!eyesInstalled}
          title={eyesInstalled ? undefined : t("search.ai.modelHint")}
          onClick={() => onPatch({ aiEyes: aiEyes === "closed" ? "none" : "closed" })}
          className={chipClass(aiEyes === "closed", !eyesInstalled)}
          data-testid="search-ai-eyes-closed"
        >
          {t("search.ai.eyesClosed")}
        </button>
        <button
          type="button"
          aria-pressed={aiEyes === "maybe"}
          disabled={!eyesInstalled}
          title={eyesInstalled ? undefined : t("search.ai.modelHint")}
          onClick={() => onPatch({ aiEyes: aiEyes === "maybe" ? "none" : "maybe" })}
          className={chipClass(aiEyes === "maybe", !eyesInstalled)}
          data-testid="search-ai-eyes-maybe"
        >
          {t("search.ai.eyesMaybe")}
        </button>
        <button
          type="button"
          aria-pressed={aiBlur}
          onClick={() => onPatch({ aiBlur: !aiBlur })}
          className={chipClass(aiBlur)}
          data-testid="search-ai-blur"
        >
          {t("search.ai.blurSoft")}
        </button>
      </div>
      {!eyesInstalled && (
        <p className="flex items-center gap-1.5 text-[11px] text-text-muted" data-testid="search-ai-model-hint">
          {t("search.ai.modelHint")}
          <Link
            to="/settings?tab=ai"
            className="shrink-0 text-accent underline-offset-2 transition-colors hover:underline"
            data-testid="search-ai-model-link"
          >
            {t("search.ai.modelLink")}
          </Link>
        </p>
      )}
      {aiRunning && (
        <p className="text-[11px] text-amber-400" data-testid="search-ai-running-hint" role="status">
          {t("search.ai.running")}
        </p>
      )}
    </div>
  );
}

/** 筛选面板（受控：inputs/onPatch 由调用方持有；清单自取）。
 *  hideAlbum=相册详情页传入：隐藏「所属相册」维度（详情页本身已在相册上下文内）。
 */
export function FilterPanel({
  inputs,
  onPatch,
  hideAlbum = false,
  advancedOnly = false,
  onPurgeMissing,
}: {
  inputs: SearchInputs;
  onPatch: (patch: Partial<SearchInputs>) => void;
  hideAlbum?: boolean;
  advancedOnly?: boolean;
  /** 「清理源缺失照片」入口（画廊传入；不传不显示——其他调用方无此动作） */
  onPurgeMissing?: () => void;
}) {
  const { t } = useTranslation();

  // 相机/相册清单挂载拉一次；镜头/格式首次渲染面板才拉（面板即「首次展开」）
  const [cameraOptions, setCameraOptions] = useState<AssetCameraCount[]>([]);
  const [lensOptions, setLensOptions] = useState<AssetLensCount[]>([]);
  const [albumOptions, setAlbumOptions] = useState<AlbumDto[]>([]);
  useEffect(() => {
    let cancelled = false;
    void cameraList().then((list) => {
      if (!cancelled) setCameraOptions(list);
    });
    void lensList().then((list) => {
      if (!cancelled) setLensOptions(list);
    });
    if (!hideAlbum) {
      void albumList().then((list) => {
        if (!cancelled) setAlbumOptions(list);
      });
    }
    return () => {
      cancelled = true;
    };
  }, [hideAlbum]);

  return (
    <div className="shrink-0 border-b border-edge bg-bg/40 px-2 py-3" data-testid="search-filter-panel">
      <div className="grid grid-cols-1 items-center gap-x-3 gap-y-1 rounded-xl border border-edge/70 bg-surface/70 p-2 shadow-inner shadow-black/20 md:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-4">
        <FieldRow label={t("search.camera")}>
          <FilterDropdown
            label={t("search.camera")}
            allLabel={t("search.cameraAll")}
            emptyLabel={t("search.cameraEmpty")}
            options={cameraOptions.map((o) => ({ value: o.camera, count: o.count }))}
            selected={inputs.cameras}
            onToggle={(camera) => onPatch({ cameras: toggleIn(inputs.cameras, camera) })}
            onClear={() => onPatch({ cameras: [] })}
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
            onToggle={(lens) => onPatch({ lenses: toggleIn(inputs.lenses, lens) })}
            onClear={() => onPatch({ lenses: [] })}
            testId="search-lens"
          />
        </FieldRow>
        {!advancedOnly && <FieldRow label={t("search.format")}><FormatFilter inputs={inputs} onPatch={onPatch} /></FieldRow>}
        {!advancedOnly && <FieldRow label={t("search.orientation")}>
          <Segment
            ariaLabel={t("search.orientation")}
            value={inputs.orientation}
            options={ORIENTATION_OPTIONS}
            onChange={(orientation) => onPatch({ orientation })}
            testId="search-orientation"
          />
        </FieldRow>}
        <FieldRow label={t("search.flash")}>
          <Segment
            ariaLabel={t("search.flash")}
            value={inputs.flash}
            options={FLASH_OPTIONS}
            onChange={(flash) => onPatch({ flash })}
            testId="search-flash"
          />
        </FieldRow>
        {!advancedOnly && <FieldRow label={t("search.gps")}>
          <Segment
            ariaLabel={t("search.gps")}
            value={inputs.gps}
            options={GPS_OPTIONS}
            onChange={(gps) => onPatch({ gps })}
            testId="search-gps"
          />
        </FieldRow>}
        <FieldRow label={t("search.ai")}>
          <AiTagsField
            aiEyes={inputs.aiEyes}
            aiBlur={inputs.aiBlur}
            onPatch={onPatch}
          />
        </FieldRow>
        <FieldRow label={t("search.color")}>
          <ColorSegment
            value={inputs.color}
            onChange={(color) => onPatch({ color })}
            testId="search-color"
          />
        </FieldRow>
        <FieldRow label={t("search.rejected")}>
          <Segment
            ariaLabel={t("search.rejected")}
            value={inputs.rejected}
            options={REJECTED_OPTIONS}
            onChange={(rejected) => onPatch({ rejected })}
            testId="search-rejected"
          />
        </FieldRow>
        {!hideAlbum && (
          <FieldRow label={t("search.album")}>
            <AlbumDropdown
              albums={albumOptions}
              selected={inputs.album}
              onSelect={(album) => onPatch({ album })}
              testId="search-album"
            />
          </FieldRow>
        )}
        <FieldRow label={t("search.library")}>
          <LibraryFilter inputs={inputs} onPatch={onPatch} />
        </FieldRow>
        <FieldRow label={t("search.focal")}>
          <RangeField
            label={t("search.focal")}
            unit="mm"
            min={inputs.focalMin}
            max={inputs.focalMax}
            onMinChange={(v) => onPatch({ focalMin: v })}
            onMaxChange={(v) => onPatch({ focalMax: v })}
            minTestId="search-focal-min"
            maxTestId="search-focal-max"
          />
        </FieldRow>
        <FieldRow label={t("search.iso")}>
          <RangeField
            label={t("search.iso")}
            min={inputs.isoMin}
            max={inputs.isoMax}
            onMinChange={(v) => onPatch({ isoMin: v })}
            onMaxChange={(v) => onPatch({ isoMax: v })}
            minTestId="search-iso-min"
            maxTestId="search-iso-max"
          />
        </FieldRow>
        <FieldRow label={t("search.aperture")}>
          <RangeField
            label={t("search.aperture")}
            min={inputs.apertureMin}
            max={inputs.apertureMax}
            step={0.1}
            onMinChange={(v) => onPatch({ apertureMin: v })}
            onMaxChange={(v) => onPatch({ apertureMax: v })}
            minTestId="search-aperture-min"
            maxTestId="search-aperture-max"
          />
        </FieldRow>
        <FieldRow label={t("search.shutter")}>
          <RangeField
            label={t("search.shutter")}
            unit="s"
            step={0.001}
            min={inputs.shutterMin}
            max={inputs.shutterMax}
            onMinChange={(v) => onPatch({ shutterMin: v })}
            onMaxChange={(v) => onPatch({ shutterMax: v })}
            minTestId="search-shutter-min"
            maxTestId="search-shutter-max"
          />
        </FieldRow>
        <FieldRow label={t("search.fileSize")}>
          <RangeField
            label={t("search.fileSize")}
            unit="MB"
            min={inputs.sizeMin}
            max={inputs.sizeMax}
            onMinChange={(v) => onPatch({ sizeMin: v })}
            onMaxChange={(v) => onPatch({ sizeMax: v })}
            minTestId="search-size-min"
            maxTestId="search-size-max"
          />
        </FieldRow>
        {!advancedOnly && <FieldRow label={t("search.date")}><DateFilter inputs={inputs} onPatch={onPatch}/></FieldRow>}
      </div>
      {/* 智能视图（B1）：有激活条件才出现保存入口（默认全部资产无保存意义） */}
          {onPurgeMissing !== undefined && (
        <div className="mt-2 flex items-center justify-end gap-2 px-2" data-testid="search-filter-actions">
          <button
            type="button"
            onClick={onPurgeMissing}
            className="rounded-md border border-edge px-2 py-1 text-[11px] text-text-muted transition-colors hover:border-red-400/60 hover:text-red-400"
            data-testid="search-purge-missing"
          >
            {t("gallery.purgeMissing")}
          </button>
        </div>
      )}
</div>
  );
}

/** 激活条件 chip（key 唯一；patch = 移除该条件后的完整输入） */
export interface ActiveChip {
  key: string;
  label: string;
  patch: SearchInputs;
}

/** 输入 → 激活条件 chips（数字区间拆为 ≥/≤ 两枚，可单独移除） */
export function buildChips(inputs: SearchInputs, t: (key: string) => string): ActiveChip[] {
  const chips: ActiveChip[] = [];
  if (inputs.favoriteOnly) {
    chips.push({ key: "favorite", label: t("nav.favorites"), patch: { ...inputs, favoriteOnly: false } });
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
  if (inputs.color !== "all") {
    chips.push({
      key: "color",
      label: `${t("search.color")} ${t(`gallery.color.${inputs.color}`)}`,
      patch: { ...inputs, color: "all" },
    });
  }
  if (inputs.rejected !== "all") {
    chips.push({
      key: "rejected",
      label: t(`search.rejected.${inputs.rejected}`),
      patch: { ...inputs, rejected: "all" },
    });
  }
  if (inputs.aiEyes !== "none") {
    chips.push({
      key: "ai-eyes",
      label: t(inputs.aiEyes === "closed" ? "search.ai.eyesClosed" : "search.ai.eyesMaybe"),
      patch: { ...inputs, aiEyes: "none" },
    });
  }
  if (inputs.aiBlur) {
    chips.push({
      key: "ai-blur",
      label: t("search.ai.blurSoft"),
      patch: { ...inputs, aiBlur: false },
    });
  }
  if (inputs.album !== null) {
    chips.push({ key: "album", label: inputs.album.name, patch: { ...inputs, album: null } });
  }
  for (const library of inputs.libraries) {
    chips.push({
      key: `library:${library.id}`,
      label: library.name,
      patch: { ...inputs, libraries: inputs.libraries.filter((l) => l.id !== library.id) },
    });
  }
  return chips;
}

/** 激活条件 chips 行（每个可单独 ×，一键清空全部） */
export function FilterChipsRow({
  chips,
  onPatch,
  onClearAll,
}: {
  chips: ActiveChip[];
  onPatch: (next: SearchInputs) => void;
  onClearAll: () => void;
}) {
  if (chips.length === 0) return null;
  return (
    <div
      className="sp-scroll flex shrink-0 flex-wrap items-center gap-1.5 overflow-x-auto border-b border-edge/60 py-1.5"
      data-testid="search-filter-chips"
    >
      <ClearFiltersButton onClear={onClearAll} testId="search-clear-all"/>
      {chips.map((chip) => (
        <span
          key={chip.key}
          className="flex max-w-[220px] items-center gap-1 rounded-full border border-accent/35 bg-accent/8 py-0.5 pl-2.5 pr-1 text-[11px] text-text-secondary"
          data-testid="search-chip"
          data-chip={chip.key}
        >
          <span className="truncate" title={chip.label}>
            {chip.label}
          </span>
          <button
            type="button"
            onClick={() => onPatch(chip.patch)}
            aria-label={`${chip.label} ×`}
            className="flex h-4 w-4 items-center justify-center rounded-full text-text-muted transition-colors hover:bg-accent/15 hover:text-accent"
            data-testid="search-chip-remove"
          >
            ×
          </button>
        </span>
      ))}

    </div>
  );
}

/** 筛选输入状态 hook（inputs + patch + 序列化键；防抖由调用方接 useDebouncedValue） */
export function useFilterInputs(): {
  inputs: SearchInputs;
  patchInputs: (patch: Partial<SearchInputs>) => void;
  setInputs: (next: SearchInputs) => void;
  key: string;
} {
  const [inputs, setInputs] = useState<SearchInputs>(EMPTY_INPUTS);
  const patchInputs = useCallback((patch: Partial<SearchInputs>) => {
    setInputs((prev) => ({ ...prev, ...patch }));
  }, []);
  const key = useMemo(() => serializeInputs(inputs), [inputs]);
  return { inputs, patchInputs, setInputs, key };
}
