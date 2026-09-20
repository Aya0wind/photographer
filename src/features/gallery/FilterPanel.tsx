import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";

import {
  cameraList,
  formatList,
  lensList,
  type AssetCameraCount,
  type AssetFilters,
  type AssetFormatCount,
  type AssetKind,
  type AssetLensCount,
} from "@/ipc/api";

/**
 * 筛选面板（M4.5 自 SearchPage 抽取的共享组件，画廊合并后唯一消费方）：
 * - 状态：SearchInputs 单对象（数字区间为原始字符串，构建时校验）；序列化键即防抖键
 * - UI：两行网格——类型/相机/镜头/格式（分段+勾选下拉）、方向/闪光灯/GPS（分段）、
 *   焦段/ISO/光圈/快门/文件大小（min-max）、日期范围+快捷段
 * - 清单：cameraList 挂载拉一次；lensList/formatList 面板首次展开才拉（少打 IPC）
 * - chips：激活条件清单（每个可单独移除 + 一键清空），由 FilterChipsRow 渲染
 */

export type KindFilter = "all" | "photo" | "raw" | "video";
type OrientationFilter = "all" | "landscape" | "portrait";
type FlashFilter = "all" | "on" | "off" | "unknown";
type GpsFilter = "all" | "yes" | "no";

const KIND_OPTIONS: ReadonlyArray<{ value: KindFilter; labelKey: string }> = [
  { value: "all", labelKey: "search.kind.all" },
  { value: "photo", labelKey: "search.kind.photo" },
  { value: "raw", labelKey: "search.kind.raw" },
  { value: "video", labelKey: "search.kind.video" },
];
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

/** 筛选面板全部输入 */
export interface SearchInputs {
  kind: KindFilter;
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

export const EMPTY_INPUTS: SearchInputs = {
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

/** UI 档位 → filters.kinds（照片=photo+raw；RAW=单列；视频=video；全部=不传） */
function kindsOf(kind: KindFilter): AssetKind[] | undefined {
  if (kind === "photo") return ["photo", "raw"];
  if (kind === "raw") return ["raw"];
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
  "rounded border border-edge bg-bg px-1.5 py-0.5 font-mono text-[11px] text-text-primary outline-none transition-colors focus:border-accent";

/** 分段单选（类型/方向/闪光灯/GPS 共用）；testId 透传到组与各按钮 */
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

// --- 面板与 chips 行 ----------------------------------------------------------------

/** 筛选面板（受控：inputs/onPatch 由调用方持有；清单自取） */
export function FilterPanel({
  inputs,
  onPatch,
}: {
  inputs: SearchInputs;
  onPatch: (patch: Partial<SearchInputs>) => void;
}) {
  const { t } = useTranslation();

  // 相机清单挂载拉一次；镜头/格式首次渲染面板才拉（面板即「首次展开」）
  const [cameraOptions, setCameraOptions] = useState<AssetCameraCount[]>([]);
  const [lensOptions, setLensOptions] = useState<AssetLensCount[]>([]);
  const [formatOptions, setFormatOptions] = useState<AssetFormatCount[]>([]);
  useEffect(() => {
    let cancelled = false;
    void cameraList().then((list) => {
      if (!cancelled) setCameraOptions(list);
    });
    void lensList().then((list) => {
      if (!cancelled) setLensOptions(list);
    });
    void formatList().then((list) => {
      if (!cancelled) setFormatOptions(list);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  function applyQuickRange(key: QuickRangeKey): void {
    const [qFrom, qTo] = quickRange(key);
    onPatch({ from: qFrom, to: qTo });
  }

  return (
    <div className="shrink-0 border-b border-edge bg-bg/40 px-2 py-3" data-testid="search-filter-panel">
      <div className="grid grid-cols-1 items-center gap-x-6 gap-y-2.5 md:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-4">
        <FieldRow label={t("search.kind")}>
          <Segment
            ariaLabel={t("search.kind")}
            value={inputs.kind}
            options={KIND_OPTIONS}
            onChange={(kind) => onPatch({ kind })}
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
            onToggle={(camera) => onPatch({ cameras: toggleIn(inputs.cameras, camera) })}
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
            onToggle={(format) => onPatch({ formats: toggleIn(inputs.formats, format) })}
            testId="search-format"
          />
        </FieldRow>
        <FieldRow label={t("search.orientation")}>
          <Segment
            ariaLabel={t("search.orientation")}
            value={inputs.orientation}
            options={ORIENTATION_OPTIONS}
            onChange={(orientation) => onPatch({ orientation })}
            testId="search-orientation"
          />
        </FieldRow>
        <FieldRow label={t("search.flash")}>
          <Segment
            ariaLabel={t("search.flash")}
            value={inputs.flash}
            options={FLASH_OPTIONS}
            onChange={(flash) => onPatch({ flash })}
            testId="search-flash"
          />
        </FieldRow>
        <FieldRow label={t("search.gps")}>
          <Segment
            ariaLabel={t("search.gps")}
            value={inputs.gps}
            options={GPS_OPTIONS}
            onChange={(gps) => onPatch({ gps })}
            testId="search-gps"
          />
        </FieldRow>
        <FieldRow label={t("search.focal")}>
          <RangeField
            label=""
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
            label=""
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
            label=""
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
            label=""
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
            label=""
            unit="MB"
            min={inputs.sizeMin}
            max={inputs.sizeMax}
            onMinChange={(v) => onPatch({ sizeMin: v })}
            onMaxChange={(v) => onPatch({ sizeMax: v })}
            minTestId="search-size-min"
            maxTestId="search-size-max"
          />
        </FieldRow>
        <FieldRow label={t("search.dateFrom")}>
          <input
            type="date"
            value={inputs.from}
            onChange={(e) => onPatch({ from: e.target.value })}
            aria-label={t("search.dateFrom")}
            className={`${INPUT_CLASS} [color-scheme:dark]`}
            data-testid="search-from"
          />
        </FieldRow>
        <FieldRow label={t("search.dateTo")}>
          <input
            type="date"
            value={inputs.to}
            onChange={(e) => onPatch({ to: e.target.value })}
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
  const { t } = useTranslation();
  if (chips.length === 0) return null;
  return (
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
            onClick={() => onPatch(chip.patch)}
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
        onClick={onClearAll}
        className="ml-1 shrink-0 rounded border border-edge px-1.5 py-0.5 text-[11px] text-text-muted transition-colors hover:border-red-400 hover:text-red-400"
        data-testid="search-clear-all"
      >
        {t("search.chipsClearAll")}
      </button>
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
