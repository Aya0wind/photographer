import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { useTranslation } from "react-i18next";

import SemanticResultsView, {
  SemanticQueryInput,
} from "@/features/ai/SemanticResultsView";
import { useSemanticSearch } from "@/features/ai/useSemanticSearch";

/**
 * 智能相册（M4 v1）：预置标签（硬编码中文）作为语义搜索快捷入口。
 * /albums → 标签网格；/albums/:tag → 该词语义搜索结果（与搜索页语义模式同管线）。
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

/** /albums：标签快捷入口网格 */
export function AlbumsIndexPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();

  return (
    <div className="h-full overflow-y-auto" data-testid="albums-page">
      <div className="mx-auto w-full max-w-[1600px] px-6 pt-4">
        <div className="flex shrink-0 items-baseline gap-3">
          <h1 className="text-sm font-semibold text-text-primary">{t("albums.title")}</h1>
          <p className="text-xs text-text-muted">{t("albums.desc")}</p>
        </div>
        <div
          className="mt-4 grid grid-cols-[repeat(auto-fill,minmax(120px,1fr))] gap-2.5 pb-6"
          data-testid="albums-tag-grid"
        >
          {SMART_ALBUM_TAGS.map((tag) => (
            <button
              key={tag}
              type="button"
              onClick={() => navigate(`/albums/${encodeURIComponent(tag)}`)}
              className="rounded-lg border border-edge bg-surface px-3 py-4 text-center text-sm text-text-secondary transition-colors hover:border-accent hover:text-accent"
              data-testid="albums-tag"
              data-tag={tag}
            >
              {tag}
            </button>
          ))}
        </div>
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
          onRetry={() => void semantic.run(lastQuery)}
        />
      </div>
    </div>
  );
}
