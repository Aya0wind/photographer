import { useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { useTranslation } from "react-i18next";

import SemanticResultsView, {
  SemanticQueryInput,
} from "@/features/ai/SemanticResultsView";
import { useSemanticSearch } from "@/features/ai/useSemanticSearch";
import { groupAssetsByDate } from "@/features/gallery/lib/assetGroups";
import { useAssetViewer } from "@/features/gallery/lib/useAssetViewer";
import ViewerOverlay from "@/features/gallery/components/ViewerOverlay";
import { useAlbumCovers } from "../lib/albumCovers";
import { loadHiddenTags } from "../lib/hiddenTags";

/**
 * 智能相册（M4 v1）：预置标签（硬编码中文）作为语义搜索快捷入口。
 * /albums → 标签网格（封面=该词首条语义命中的缩略图，进页面后台并发 3 预取；
 * 失败/未命中静默占位）；/albums/:tag → 该词语义搜索结果（与搜索页语义模式同管线）。
 * 标签可见性：设置页画廊 tab 多选（localStorage smartphoto.albums.hiddenTags）。
 */

/** v1 预置标签（硬编码中文；后续可由索引统计生成） */
export const SMART_ALBUM_TAGS: readonly string[] = [
  "人像",
  "风景",
  "夜景",
  "美食",
  "建筑",
  "街拍",
  "动物",
  "花卉",
  "雪",
  "日落",
  "黑白",
];

/** 标签封面块（img / 占位） */
function TagCover({ url, tag }: { url: string | null | undefined; tag: string }) {
  if (url) {
    return (
      <img
        src={url}
        alt=""
        loading="lazy"
        decoding="async"
        className="h-full w-full object-cover"
        data-testid="albums-tag-cover-img"
        data-tag={tag}
      />
    );
  }
  return (
    <div
      className="flex h-full w-full items-center justify-center bg-panel/40"
      data-testid="albums-tag-cover-fallback"
      data-tag={tag}
    >
      <svg
        viewBox="0 0 24 24"
        width="18"
        height="18"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
        className="text-text-muted"
        aria-hidden="true"
      >
        <rect x="3.5" y="4.5" width="17" height="15" rx="2" />
        <circle cx="9" cy="10" r="1.8" />
        <path d="M4.5 17l4.5-4.5 3.5 3.5 3-3 4 4" />
      </svg>
    </div>
  );
}

/** /albums：标签快捷入口网格（含封面；进页面后台批量预取） */
export function AlbumsIndexPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  // 可见标签（挂载时读一次隐藏清单；设置页修改后下次进入生效）
  const visibleTags = useMemo(
    () => SMART_ALBUM_TAGS.filter((tag) => !loadHiddenTags().includes(tag)),
    [],
  );
  const covers = useAlbumCovers(visibleTags);

  return (
    <div className="h-full overflow-y-auto" data-testid="albums-page">
      <div className="mx-auto w-full max-w-[1600px] px-6 pt-4">
        <div className="flex shrink-0 items-baseline gap-3">
          <h1 className="text-sm font-semibold text-text-primary">{t("albums.title")}</h1>
          <p className="text-xs text-text-muted">{t("albums.desc")}</p>
        </div>
        {visibleTags.length === 0 ? (
          <p className="mt-6 text-xs text-text-muted" data-testid="albums-all-hidden">
            {t("albums.allHidden")}
          </p>
        ) : (
          <div
            className="mt-4 grid grid-cols-[repeat(auto-fill,minmax(120px,1fr))] gap-2.5 pb-6"
            data-testid="albums-tag-grid"
          >
            {visibleTags.map((tag) => (
              <button
                key={tag}
                type="button"
                onClick={() => navigate(`/albums/${encodeURIComponent(tag)}`)}
                className="overflow-hidden rounded-lg border border-edge bg-surface text-center transition-colors hover:border-accent"
                data-testid="albums-tag"
                data-tag={tag}
              >
                <span className="block h-20 w-full border-b border-edge/60">
                  <TagCover url={covers[tag]} tag={tag} />
                </span>
                <span
                  className="block px-2 py-2 text-sm text-text-secondary transition-colors group-hover:text-accent"
                  data-testid="albums-tag-label"
                >
                  {tag}
                </span>
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

/** /albums/:tag：该标签的语义搜索结果（自动执行；输入框可改词重搜，不回写路由） */
export function AlbumTagPage() {
  const { t } = useTranslation();
  const { tag = "" } = useParams();
  const semantic = useSemanticSearch();
  const [lastQuery, setLastQuery] = useState(tag);
  // 语义结果同样可点开查看器（与画廊/搜索页一致）
  const groups = useMemo(() => groupAssetsByDate(semantic.assets), [semantic.assets]);
  const { viewer, openAsset, closeViewer, navigateTo } = useAssetViewer(groups);

  // 标签变化（含首挂载）→ 自动语义搜索
  useEffect(() => {
    if (tag) {
      setLastQuery(tag);
      void semantic.run(tag);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tag]);

  return (
    <div className="flex h-full flex-col" data-testid="album-tag-page">
      <div className="mx-auto flex h-11 w-full max-w-[1600px] shrink-0 items-center gap-3 border-b border-edge px-6">
        <h1 className="shrink-0 text-sm font-semibold text-text-primary">
          {t("albums.tagTitle", { tag })}
        </h1>
        <SemanticQueryInput
          busy={semantic.status === "loading"}
          onRun={(q) => {
            setLastQuery(q);
            void semantic.run(q);
          }}
        />
      </div>
      <div className="mx-auto min-h-0 w-full max-w-[1600px] flex-1 px-6 pt-3">
        <SemanticResultsView
          status={semantic.status}
          assets={semantic.assets}
          scores={semantic.scores}
          onOpenAsset={openAsset}
          onRetry={() => void semantic.run(lastQuery)}
        />
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
