import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router";
import { useTranslation } from "react-i18next";

import {
  assetsPage,
  formatList,
  kindFromName,
  type AssetDto,
  type AssetFormatCount,
  type AssetKind,
} from "@/ipc/api";
import AssetThumb from "@/features/gallery/components/AssetThumb";

/**
 * 媒体类型页（M4.5，/media）：照片 / RAW / 视频三大卡。
 * - 计数：formatList 的格式计数按 kindFromName 归并（照片=JPG 等非 RAW 非视频、
 *   RAW=NEF+ARW…、视频=MP4…）——后端不可用时计数 0，卡片仍渲染
 * - 封面：assetsPage(0, 1, {kinds}) 取该类首张 → AssetThumb 模糊背景（blur-2xl
 *   毛玻璃）+ 图标 + 计数；无资产时渐变底
 * - 点击 → /gallery?kind=photo|raw|video（画廊 URL 协议预置 kinds 筛选）
 */

/** 三大卡的类型定义与展示序 */
const KIND_CARDS: ReadonlyArray<{ kind: AssetKind; labelKey: string }> = [
  { kind: "photo", labelKey: "media.photo" },
  { kind: "raw", labelKey: "media.raw" },
  { kind: "video", labelKey: "media.video" },
];

/** 格式计数 → 按大类归并（未知格式归照片档，与 kindFromName 口径一致） */
export function countsByKind(
  formats: AssetFormatCount[],
): Record<AssetKind, number> {
  const counts: Record<AssetKind, number> = { photo: 0, raw: 0, video: 0 };
  for (const entry of formats) {
    const kind = kindFromName(`a.${entry.format}`);
    if (kind === "raw") counts.raw += entry.count;
    else if (kind === "video") counts.video += entry.count;
    else if (kind === "photo") counts.photo += entry.count;
    // other：不归并（格式清单不该出现，防御）
  }
  return counts;
}

/** 类型图标（与画廊 kind 占位同风格） */
function KindGlyph({ kind }: { kind: AssetKind }) {
  if (kind === "video") {
    return (
      <svg viewBox="0 0 24 24" width="40" height="40" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <rect x="3" y="5" width="18" height="14" rx="2" />
        <path d="M10 9.5l5 2.5-5 2.5v-5z" />
      </svg>
    );
  }
  if (kind === "raw") {
    return (
      <svg viewBox="0 0 24 24" width="40" height="40" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <rect x="3.5" y="5" width="17" height="14" rx="2" />
        <path d="M3.5 15l4.5-4.5 3.5 3.5 3-3L20.5 17" />
        <circle cx="8.5" cy="9" r="1.5" />
      </svg>
    );
  }
  return (
    <svg viewBox="0 0 24 24" width="40" height="40" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <rect x="3.5" y="4.5" width="17" height="15" rx="2" />
      <circle cx="9" cy="10" r="1.8" />
      <path d="M4.5 17l4.5-4.5 3.5 3.5 3-3 4 4" />
    </svg>
  );
}

/** 毛玻璃封面：该类首张缩略图模糊背景 + 深色渐变 + 图标/计数前景 */
function MediaCard({
  kind,
  label,
  count,
  cover,
  onClick,
}: {
  kind: AssetKind;
  label: string;
  count: number;
  cover: AssetDto | null;
  onClick: () => void;
}) {
  const { t } = useTranslation();
  return (
    <button
      type="button"
      onClick={onClick}
      className="group relative h-44 overflow-hidden rounded-xl border border-edge text-left transition-colors hover:border-accent"
      data-testid="media-card"
      data-kind={kind}
      data-count={count}
    >
      {/* 背景：首张缩略图 blur-2xl 毛玻璃（无图渐变底） */}
      <div className="absolute inset-0" data-testid={`media-cover-${kind}`}>
        {cover !== null ? (
          <div className="h-full w-full scale-125 blur-2xl">
            <AssetThumb asset={cover} size={240} className="h-full w-full" skeleton={false} />
          </div>
        ) : (
          <div
            className={`h-full w-full ${
              kind === "raw"
                ? "bg-gradient-to-br from-sky-900/60 to-bg"
                : kind === "video"
                  ? "bg-gradient-to-br from-violet-900/60 to-bg"
                  : "bg-gradient-to-br from-accent/20 to-bg"
            }`}
            data-testid={`media-cover-fallback-${kind}`}
          />
        )}
      </div>
      {/* 前景：深色渐变 + 图标 + 名称 + 计数 */}
      <div className="absolute inset-0 flex flex-col justify-end gap-1 bg-gradient-to-t from-black/70 via-black/30 to-transparent p-4">
        <span className="text-text-secondary/90">
          <KindGlyph kind={kind} />
        </span>
        <h2 className="text-base font-semibold text-white">{label}</h2>
        <p className="font-mono text-xs tabular-nums text-white/75">
          {t("media.count", { count })}
        </p>
      </div>
    </button>
  );
}

export default function MediaPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [counts, setCounts] = useState<Record<AssetKind, number>>({
    photo: 0,
    raw: 0,
    video: 0,
  });
  const [covers, setCovers] = useState<Partial<Record<AssetKind, AssetDto | null>>>({});

  // 计数：formatList 归并一次；封面：每类取首张（limit=1）
  useEffect(() => {
    let cancelled = false;
    void formatList()
      .then((formats) => {
        if (!cancelled) setCounts(countsByKind(formats));
      })
      .catch(() => {
        if (!cancelled) setCounts({ photo: 0, raw: 0, video: 0 });
      });
    for (const { kind } of KIND_CARDS) {
      void assetsPage(0, 1, { kinds: [kind] })
        .then((page) => {
          if (cancelled) return;
          setCovers((prev) => ({ ...prev, [kind]: page[0] ?? null }));
        })
        .catch(() => {
          if (!cancelled) setCovers((prev) => ({ ...prev, [kind]: null }));
        });
    }
    return () => {
      cancelled = true;
    };
  }, []);

  const total = useMemo(() => counts.photo + counts.raw + counts.video, [counts]);

  return (
    <div className="h-full overflow-y-auto" data-testid="media-page">
      <div className="mx-auto w-full max-w-[1600px] px-6 pt-4">
        <div className="flex shrink-0 items-baseline gap-3">
          <h1 className="text-sm font-semibold text-text-primary">{t("media.title")}</h1>
          <p className="text-xs text-text-muted">{t("media.desc")}</p>
        </div>

        {total === 0 && (
          <div
            className="mt-4 rounded-xl border border-edge bg-surface/50 px-6 py-3 text-center"
            data-testid="media-empty"
          >
            <p className="text-xs text-text-muted">{t("media.empty")}</p>
          </div>
        )}

        <div
          className="mt-4 grid grid-cols-[repeat(auto-fill,minmax(280px,1fr))] gap-4 pb-6"
          data-testid="media-grid"
        >
          {KIND_CARDS.map(({ kind, labelKey }) => (
            <MediaCard
              key={kind}
              kind={kind}
              label={t(labelKey)}
              count={counts[kind]}
              cover={covers[kind] ?? null}
              onClick={() => navigate(`/gallery?kind=${kind}`)}
            />
          ))}
        </div>
      </div>
    </div>
  );
}
