import { useTranslation } from "react-i18next";

/**
 * 右侧年份吸顶条（M4.5 A4，画廊专用）：竖排年份（降序），滚动联动高亮当前年，
 * 点击跳该年首个日期组（跳转逻辑由调用方经 jumpToDate 补页执行）。
 * 半透明底 + 细边，悬在网格右缘（不占布局流），多年份时自身可滚动。
 */
export default function YearRail({
  years,
  currentYear,
  onJumpYear,
}: {
  /** 年份降序（未知日期组不含年份，由调用方排除） */
  years: number[];
  /** 当前视口所在年（视口在未知组/顶部时为 null——无高亮） */
  currentYear: number | null;
  onJumpYear: (year: number) => void;
}) {
  const { t } = useTranslation();
  if (years.length === 0) return null;

  return (
    <div
      className="sp-scroll absolute right-1 top-0 z-10 flex max-h-full flex-col items-end gap-0.5 overflow-y-auto py-3"
      data-testid="gallery-year-rail"
      aria-label={t("gallery.yearRail")}
    >
      {years.map((year) => {
        const active = year === currentYear;
        return (
          <button
            key={year}
            type="button"
            onClick={() => onJumpYear(year)}
            aria-current={active}
            className={`rounded-l-md border border-r-0 py-0.5 pl-1.5 pr-1 font-mono text-[10px] leading-4 tabular-nums transition-colors ${
              active
                ? "border-edge bg-surface font-semibold text-accent"
                : "border-transparent text-text-muted/80 hover:border-edge/60 hover:bg-surface/80 hover:text-text-primary"
            }`}
            data-testid="gallery-year"
            data-year={year}
            data-active={active}
          >
            {year}
          </button>
        );
      })}
    </div>
  );
}
