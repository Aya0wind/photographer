import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { gearStats, type GearLabelBucket, type GearNameCount, type GearStats } from "@/ipc/api";

/**
 * 器材统计（M7 F9，/gear）：机身/镜头 TOP 榜 + 焦段/ISO/光圈/快门四分布柱图。
 * - 数据 gearStats() 进页拉一次；null（无 EXIF/后端未就绪）或全 0 → 空态
 * - TOP 榜横条 = 纯 CSS 宽度百分比（静态；遵守动画体系只许 transform/opacity——
 *   宽度不做补间，直接落位）
 * - 分布柱图 = 纯 CSS 纵向柱（flex items-end + 高度百分比），不引图表库；
 *   最高柱 accent 色，其余 accent/30；柱列 title 提示「label · count」
 */

/** TOP 榜每卡最多行数（后端已按 count 降序，取前若干即可读） */
const TOP_LIMIT = 6;

function TopCard({
  title,
  items,
  testId,
  emptyKey,
}: {
  title: string;
  items: GearNameCount[];
  testId: string;
  emptyKey: string;
}) {
  const { t } = useTranslation();
  const max = items.length > 0 ? items[0].count : 0;
  return (
    <div
      className="min-w-0 flex-1 rounded-lg border border-edge bg-surface p-4"
      data-testid={testId}
    >
      <h2 className="pb-3 text-xs font-semibold text-text-primary">{title}</h2>
      {items.length === 0 ? (
        <p className="py-4 text-center text-[11px] text-text-muted">{t(emptyKey)}</p>
      ) : (
        <div className="flex flex-col gap-2.5">
          {items.map((item) => (
            <div
              key={item.name}
              className="flex items-center gap-3"
              data-testid="gear-top-row"
              data-name={item.name}
              data-count={item.count}
              title={`${item.name} · ${item.count}`}
            >
              <span className="w-40 shrink-0 truncate text-xs text-text-secondary" title={item.name}>
                {item.name}
              </span>
              <div className="h-1.5 min-w-0 flex-1 overflow-hidden rounded bg-panel">
                <div
                  className="h-full rounded bg-accent"
                  style={{ width: `${max > 0 ? Math.max(2, (item.count / max) * 100) : 0}%` }}
                  data-testid="gear-top-bar"
                />
              </div>
              <span className="w-12 shrink-0 text-right font-mono text-[11px] tabular-nums text-text-muted">
                {item.count}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function ChartCard({
  title,
  buckets,
  testId,
}: {
  title: string;
  buckets: GearLabelBucket[];
  testId: string;
}) {
  const max = buckets.reduce((m, b) => Math.max(m, b.count), 0);
  return (
    <div
      className="rounded-lg border border-edge bg-surface p-4"
      data-testid={testId}
    >
      <h2 className="pb-3 text-xs font-semibold text-text-primary">{title}</h2>
      <div className="flex h-36 items-end justify-between gap-1.5">
        {buckets.map((bucket, i) => {
          const isMax = bucket.count > 0 && bucket.count === max;
          return (
            <div
              key={`${bucket.label}-${i}`}
              className="flex h-full min-w-0 flex-1 flex-col items-center justify-end gap-1"
              data-testid="gear-chart-bar"
              data-label={bucket.label}
              data-count={bucket.count}
              data-max={isMax ? "true" : undefined}
              title={`${bucket.label} · ${bucket.count}`}
            >
              <span className="font-mono text-[10px] tabular-nums text-text-muted">
                {bucket.count}
              </span>
              <div
                className={`w-full max-w-10 rounded-t ${isMax ? "bg-accent" : "bg-accent/30"}`}
                style={{ height: `${max > 0 ? Math.max(3, (bucket.count / max) * 100) : 3}%` }}
              />
              <span className="w-full truncate text-center text-[10px] text-text-muted" title={bucket.label}>
                {bucket.label}
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** 是否有任何可用数据（全 0/全空也按空态处理） */
function hasAnyData(stats: GearStats): boolean {
  return (
    stats.cameras.some((c) => c.count > 0) ||
    stats.lenses.some((c) => c.count > 0) ||
    stats.focalBuckets.some((b) => b.count > 0) ||
    stats.isoBuckets.some((b) => b.count > 0) ||
    stats.apertureBuckets.some((b) => b.count > 0) ||
    stats.shutterBuckets.some((b) => b.count > 0)
  );
}

export default function GearPage() {
  const { t } = useTranslation();

  const [stats, setStats] = useState<GearStats | null>(null);
  const [status, setStatus] = useState<"loading" | "ready">("loading");

  useEffect(() => {
    let cancelled = false;
    void gearStats().then((result) => {
      if (cancelled) return;
      setStats(result);
      setStatus("ready");
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const tops = useMemo(
    () => ({
      cameras: stats ? stats.cameras.filter((c) => c.count > 0).slice(0, TOP_LIMIT) : [],
      lenses: stats ? stats.lenses.filter((c) => c.count > 0).slice(0, TOP_LIMIT) : [],
    }),
    [stats],
  );
  const empty = status === "ready" && (stats === null || !hasAnyData(stats));

  return (
    <div className="h-full" data-testid="gear-page">
      <div
        className="sp-scroll mx-auto h-full w-full max-w-[1600px] overflow-y-auto px-6"
        data-testid="gear-content"
      >
        {/* 头部 */}
        <div className="flex h-11 shrink-0 items-center gap-3 border-b border-edge">
          <h1 className="text-sm font-semibold text-text-primary">{t("gear.title")}</h1>
          <p className="text-xs text-text-muted">{t("gear.desc")}</p>
        </div>

        {status === "loading" ? (
          <div
            className="flex h-full items-center justify-center text-xs text-text-muted"
            data-testid="gear-loading"
          >
            {t("gear.loading")}
          </div>
        ) : empty ? (
          <div
            className="flex h-full flex-col items-center justify-center gap-2 px-8 text-center"
            data-testid="gear-empty"
          >
            <p className="text-sm text-text-secondary">{t("gear.empty")}</p>
            <p className="text-xs text-text-muted">{t("gear.emptyHint")}</p>
          </div>
        ) : stats !== null ? (
          <div className="flex flex-col gap-4 py-5">
            {/* 顶部：机身/镜头 TOP 两并列卡 */}
            <div className="flex gap-4">
              <TopCard
                title={t("gear.topCameras")}
                items={tops.cameras}
                testId="gear-top-cameras"
                emptyKey="gear.topEmpty"
              />
              <TopCard
                title={t("gear.topLenses")}
                items={tops.lenses}
                testId="gear-top-lenses"
                emptyKey="gear.topEmpty"
              />
            </div>

            {/* 中部：四分布柱图 */}
            <div className="grid grid-cols-2 gap-4">
              <ChartCard title={t("gear.chart.focal")} buckets={stats.focalBuckets} testId="gear-chart-focal" />
              <ChartCard title={t("gear.chart.iso")} buckets={stats.isoBuckets} testId="gear-chart-iso" />
              <ChartCard
                title={t("gear.chart.aperture")}
                buckets={stats.apertureBuckets}
                testId="gear-chart-aperture"
              />
              <ChartCard
                title={t("gear.chart.shutter")}
                buckets={stats.shutterBuckets}
                testId="gear-chart-shutter"
              />
            </div>
          </div>
        ) : null}
      </div>
    </div>
  );
}
