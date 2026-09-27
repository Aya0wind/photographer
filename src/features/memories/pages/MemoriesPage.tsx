import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { onThisDay, type AssetDto } from "@/ipc/api";
import type { AssetGroup } from "@/features/gallery/lib/assetGroups";
import AssetThumb from "@/features/gallery/components/AssetThumb";
import ViewerOverlay from "@/features/gallery/components/ViewerOverlay";
import { useAssetViewer } from "@/features/gallery/lib/useAssetViewer";

/**
 * 那年今天（M7 F6，/memories）：历年同月日资产按年份分块横滚。
 * - 数据 onThisDay() 进页拉一次（后端按今天的月-日查历年；无历史返回 []）
 * - 年份块降序——今年今天的块天然置顶并标「今天」，其余块头「N 年前」
 * - 每块 = 一个 AssetGroup（查看器组内导航/胶片条不跨年），复用 useAssetViewer
 *   的 ?asset= URL 协议与浏览打点
 * - 空态：今天在库里没有历史记录文案
 */

/** 横滚卡缩略图名义边长（后端 snap 256 档，与网格同缓存） */
const CARD_THUMB_PX = 240;

interface YearBlock extends AssetGroup {
  /** 四位年份 */
  year: number;
}

function buildYearBlocks(assets: AssetDto[], currentYear: number): YearBlock[] {
  // 同月日查询不可能命中 capturedAt=null，防御性跳过（无年份可归）
  const byYear = new Map<number, AssetDto[]>();
  for (const asset of assets) {
    const year = Number(asset.capturedAt?.slice(0, 4));
    if (!Number.isFinite(year) || year <= 0 || year >= currentYear) continue;
    const list = byYear.get(year);
    if (list) list.push(asset);
    else byYear.set(year, [asset]);
  }
  return [...byYear.entries()]
    .sort((a, b) => b[0] - a[0])
    .map(([year, list]) => ({
      key: `year-${year}`,
      date: list[0]?.capturedAt?.slice(0, 10) ?? null,
      assets: list,
      year,
    }));
}

export default function MemoriesPage() {
  const { t } = useTranslation();

  const [assets, setAssets] = useState<AssetDto[]>([]);
  const [status, setStatus] = useState<"loading" | "ready">("loading");

  useEffect(() => {
    let cancelled = false;
    void onThisDay().then((list) => {
      if (cancelled) return;
      setAssets(list);
      setStatus("ready");
    });
    return () => {
      cancelled = true;
    };
  }, []);

  // 以本地时区今天的年份分块（契约：后端按本地月-日匹配）
  const currentYear = new Date().getFullYear();
  const blocks = useMemo(() => buildYearBlocks(assets, currentYear), [assets, currentYear]);
  const { viewer, openAsset, closeViewer, navigateTo } = useAssetViewer(blocks);

  return (
    <div className="h-full" data-testid="memories-page">
      <div className="flex h-full w-full flex-col px-4">
        {/* 头部 */}
        <div className="flex h-11 shrink-0 items-center gap-3 border-b border-edge">
          <h1 className="text-sm font-semibold text-text-primary">{t("memories.title")}</h1>
          <p className="text-xs text-text-muted">{t("memories.desc")}</p>
        </div>

        {/* 年份分块 / 空态 */}
        <div className="sp-scroll min-h-0 flex-1 overflow-y-auto" data-testid="memories-content">
          {status === "loading" ? (
            <div
              className="flex h-full items-center justify-center text-xs text-text-muted"
              data-testid="memories-loading"
            >
              {t("memories.loading")}
            </div>
          ) : blocks.length === 0 ? (
            <div
              className="flex h-full flex-col items-center justify-center gap-2 px-8 text-center"
              data-testid="memories-empty"
            >
              <p className="text-sm text-text-secondary">{t("memories.empty")}</p>
              <p className="text-xs text-text-muted">{t("memories.emptyHint")}</p>
            </div>
          ) : (
            <div className="flex flex-col gap-6 py-5">
              {blocks.map((block) => (
                <section key={block.key} data-testid="memories-block" data-year={block.year}>
                  <div className="flex items-baseline gap-2 pb-2">
                    <h2 className="text-sm font-semibold text-text-primary">
                      {t("memories.yearsAgo", { count: currentYear - block.year })}
                    </h2>
                    <span className="font-mono text-[11px] tabular-nums text-text-muted">
                      {t("memories.blockCount", { count: block.assets.length })}
                    </span>
                    <span className="font-mono text-[11px] text-text-muted/70">{block.year}</span>
                  </div>
                  <div className="grid grid-cols-[repeat(auto-fill,minmax(176px,1fr))] gap-3 pb-2">
                    {block.assets.map((asset) => (
                      <button
                        key={asset.id}
                        type="button"
                        onClick={() => openAsset(asset)}
                        className="group relative h-32 min-w-0 overflow-hidden rounded-md border border-edge/60 bg-panel/40 transition-colors hover:border-accent/60"
                        data-testid="memories-card"
                        data-asset-id={asset.id}
                        title={asset.name}
                      >
                        <AssetThumb asset={asset} size={CARD_THUMB_PX} className="h-full w-full" />
                      </button>
                    ))}
                  </div>
                </section>
              ))}
            </div>
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
