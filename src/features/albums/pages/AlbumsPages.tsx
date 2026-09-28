import { usePhotoCards } from "@/features/gallery/lib/usePhotoCards";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { useTranslation } from "react-i18next";
import { getIntlLocale } from "@/i18n";

import SemanticResultsView, {
  SemanticQueryInput,
  SemanticGateNotice,
} from "@/features/ai/SemanticResultsView";
import { useSemanticSearch, useSemanticGate } from "@/features/ai/useSemanticSearch";
import { groupAssetsByDate } from "@/features/gallery/lib/assetGroups";
import { useAssetViewer } from "@/features/gallery/lib/useAssetViewer";
import ViewerOverlay from "@/features/gallery/components/ViewerOverlay";
import ContextMenu, { type ContextMenuEntry } from "@/features/gallery/components/ContextMenu";
import AssetThumb from "@/features/gallery/components/AssetThumb";
import {
  albumAssetsPage,
  albumCoverSet,
  albumCreate,
  albumDelete,
  albumDirRename,
  albumList,
  albumRename,
  type AlbumDto,
  type AssetDto,
} from "@/ipc/api";
import { ALBUM_COVER_THUMB_SIZE, useAlbumCoverAssetIds, useManualAlbumCovers } from "../lib/albumCovers";
import { isUngroupedAlbum } from "../lib/ungroupedAlbum";
import { DEFAULT_SMART_TAGS, loadSmartTags, smartTagLabel } from "../lib/smartTags";
import AlbumDetailPage from "./AlbumDetailPage";

/**
 * 相册页（手工 + 智能同页两区，2026-09 相册定案重构）：
 * - 手工相册 = 纯引用照片组（同一照片可入多相册；删除只删引用）。/albums 手工区：
 *   相册卡片网格（封面=coverAssetId 缩略图，未指定回退列表第一张，空相册占位图形）
 *   + 名称 + 张数；卡片右键/悬浮菜单：重命名 / 更改相册文件夹名（目录化，仅改
 *   磁盘目录名、显示名不动）/ 设为封面 / 删除（红色确认）。
 * - 智能相册 = 预置标签墙（40 个中文，飞牛词表对齐），自动管理的相册：点击进入
 *   该标签的语义结果视图（/albums/:tag，行为照旧）。卡片样式与手工区统一。
 * - 分区样式参考图库「按年分块」写法（区标题 + 内容块）。
 * - /albums/:tag 参数为纯数字 → 手工相册详情页（AlbumDetailPage）。
 * 标签可见性：设置页画廊 tab 多选（localStorage smartphoto.albums.hiddenTags）。
 */

/** v1 预置标签（40 个；M4.5 扩到飞牛词表，含原 11 个；后续可由索引统计生成） */
export const SMART_ALBUM_TAGS: readonly string[] = DEFAULT_SMART_TAGS;

// --- 共享卡片件（手工 / 智能同款样式） ------------------------------------------------

/** 封面块（img / 渐变底+名称占位）；手工与智能相册卡片共用 */
function CardCover({
  url,
  label,
  testIdImg,
  testIdFallback,
  dataTag,
}: {
  url: string | null | undefined;
  label: string;
  testIdImg: string;
  testIdFallback: string;
  dataTag?: string;
}) {
  if (url) {
    return (
      <img
        src={url}
        alt=""
        loading="lazy"
        decoding="async"
        className="h-full w-full object-cover"
        data-testid={testIdImg}
        data-tag={dataTag}
      />
    );
  }
  return (
    <div
      className="flex h-full w-full flex-col items-center justify-center gap-1 bg-gradient-to-br from-panel via-bg to-bg"
      data-testid={testIdFallback}
      data-tag={dataTag}
    >
      <svg
        viewBox="0 0 24 24"
        width="16"
        height="16"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
        className="text-text-muted"
        aria-hidden="true"
      >
        <path d="M4 8.5l4-4 4 4 4-4 4 4" />
        <path d="M4 15.5l4-4 4 4 4-4 4 4" />
      </svg>
      <span className="px-1 text-center text-[10px] leading-tight text-text-muted">{label}</span>
    </div>
  );
}

/** 智能相册封面：搜索期间和缩略图生成/解码期间均显示共享骨架动画。 */
function SmartAlbumCover({ assetId, tag }: { assetId: number | null | undefined; tag: string }) {
  if (assetId === undefined) {
    return (
      <div
        className="sp-skeleton h-full w-full"
        data-testid="albums-tag-cover-loading"
        data-tag={tag}
      />
    );
  }
  if (assetId === null) {
    return (
      <CardCover
        url={null}
        label={smartTagLabel(tag)}
        testIdImg="albums-tag-cover-img"
        testIdFallback="albums-tag-cover-fallback"
        dataTag={tag}
      />
    );
  }
  return (
    <AssetThumb
      asset={{ id: assetId, kind: "photo", name: tag }}
      size={ALBUM_COVER_THUMB_SIZE}
      alt=""
      className="h-full w-full"
      testId="albums-tag-cover"
    />
  );
}

/** 区块头（参考图库按年分块：标题 + 计数） */
function SectionHeader({
  title,
  count,
  testId,
  expanded,
  onToggle,
}: {
  title: string;
  count: number;
  testId: string;
  expanded: boolean;
  onToggle: () => void;
}) {
  return (
    <header data-testid={testId}>
      <button type="button" onClick={onToggle} aria-expanded={expanded} className="flex h-10 w-full items-center gap-2 text-left">
        <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" strokeWidth="1.8" className={`text-text-muted transition-transform ${expanded ? "rotate-90" : ""}`} aria-hidden="true"><path d="m6 3 5 5-5 5" /></svg>
        <h2 className="text-[13px] font-semibold text-text-primary">{title}</h2>
        <span className="text-xs text-text-muted">{count}</span>
      </button>
    </header>
  );
}

/** 通用小弹窗外壳（重命名/删除确认/封面选择共用）：居中模态，点背景关闭 */
function ModalShell({
  title,
  onClose,
  testId,
  children,
  footer,
}: {
  title: string;
  onClose: () => void;
  testId: string;
  children: React.ReactNode;
  footer?: React.ReactNode;
}) {
  return (
    <div
      className="fixed inset-0 z-[70] flex items-center justify-center bg-black/45"
      onClick={onClose}
      data-testid={`${testId}-overlay`}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={title}
        className="flex max-h-[70vh] w-[380px] flex-col overflow-hidden rounded-xl border border-edge bg-surface shadow-2xl"
        onClick={(e) => e.stopPropagation()}
        data-testid={testId}
      >
        <div className="flex shrink-0 items-center justify-between border-b border-edge px-4 py-3">
          <h2 className="text-sm font-semibold text-text-primary">{title}</h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="close"
            className="rounded px-1.5 text-lg leading-none text-text-muted transition-colors hover:text-text-primary"
            data-testid={`${testId}-close`}
          >
            ×
          </button>
        </div>
        <div className="sp-scroll min-h-0 flex-1 overflow-y-auto p-4">{children}</div>
        {footer !== undefined && (
          <div className="flex shrink-0 items-center justify-end gap-2 border-t border-edge px-4 py-3">
            {footer}
          </div>
        )}
      </div>
    </div>
  );
}

/** 手工相册卡片：封面 + 名称 + 张数；点击进详情；右键/悬浮 ⋯ 打开卡片菜单 */
function ManualAlbumCard({
  album,
  cover,
  onOpen,
  onMenu,
}: {
  album: AlbumDto;
  cover: string | null | undefined;
  onOpen: (album: AlbumDto) => void;
  onMenu: (at: { x: number; y: number }, album: AlbumDto) => void;
}) {
  const { t } = useTranslation();
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={() => onOpen(album)}
      onKeyDown={(e) => {
        if (e.key === "Enter") onOpen(album);
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        onMenu({ x: e.clientX, y: e.clientY }, album);
      }}
      className="group relative cursor-pointer select-none overflow-hidden rounded-lg border border-edge bg-surface text-center transition-colors hover:border-accent"
      data-testid="albums-manual-card"
      data-album-id={album.id}
    >
      <span className="relative block h-20 w-full border-b border-edge/60">
        <CardCover url={cover} label={album.name} testIdImg="albums-manual-cover" testIdFallback="albums-manual-cover-fallback" />
        <span className="absolute right-1.5 top-1.5 rounded bg-black/65 px-1.5 py-0.5 text-[10px] font-medium tabular-nums text-white" data-testid="albums-manual-count">{t("albums.itemCountBadge", { count: album.itemCount })}</span>
        {isUngroupedAlbum(album) && <span className="absolute bottom-1.5 right-1.5 rounded bg-black/65 px-1.5 py-0.5 text-[10px] text-white" data-testid="albums-manual-badge-ungrouped">{t("albums.ungroupedBadge")}</span>}
      </span>
      <span className="block px-2 py-2 text-sm text-text-secondary">
        <span className="block truncate" data-testid="albums-manual-name" title={album.name}>
          {album.name}
        </span>
        <span className="mt-1 block text-[11px] text-text-muted" data-testid="albums-manual-created">
          {t("albums.createdDate", { date: Number.isNaN(new Date(album.createdAt).getTime()) ? "–" : new Date(album.createdAt).toLocaleDateString(getIntlLocale(), { year: "numeric", month: "2-digit", day: "2-digit" }) })}
        </span>
      </span>
      {/* 悬浮 ⋯ 菜单钮（与右键同一菜单） */}
      <button
        type="button"
        aria-label="menu"
        onClick={(e) => {
          e.stopPropagation();
          const rect = e.currentTarget.getBoundingClientRect();
          onMenu({ x: rect.left, y: rect.bottom + 2 }, album);
        }}
        className="absolute left-1 top-1 flex h-5 w-5 items-center justify-center rounded bg-black/45 text-xs leading-none text-white opacity-0 transition-opacity hover:bg-black/70 group-hover:opacity-100"
        data-testid="albums-card-menu"
        data-album-id={album.id}
      >
        ⋯
      </button>
    </div>
  );
}

/** 设为封面选择弹窗：相册第一页照片网格，点选即设为封面；「恢复自动封面」= 清除指定 */
function CoverPickerDialog({
  album,
  onClose,
  onPicked,
}: {
  album: AlbumDto;
  onClose: () => void;
  onPicked: (assetId: number | null) => void;
}) {
  const { t } = useTranslation();
  const [assets, setAssets] = useState<AssetDto[] | null>(null);
  useEffect(() => {
    let cancelled = false;
    void albumAssetsPage(album.id, 0, 24).then((list) => {
      if (!cancelled) setAssets(list);
    });
    return () => {
      cancelled = true;
    };
  }, [album.id]);

  return (
    <ModalShell title={t("albums.coverPickTitle", { name: album.name })} onClose={onClose} testId="album-cover-dialog">
      {assets === null ? (
        <p className="text-xs text-text-muted" data-testid="album-cover-loading">
          {t("recent.loading")}
        </p>
      ) : assets.length === 0 ? (
        <p className="text-xs text-text-muted" data-testid="album-cover-empty">
          {t("albums.coverPickEmpty")}
        </p>
      ) : (
        <>
          <div className="grid grid-cols-4 gap-1.5" data-testid="album-cover-grid">
            {assets.map((asset) => (
              <button
                key={asset.id}
                type="button"
                onClick={() => onPicked(asset.id)}
                className="aspect-square overflow-hidden rounded-md border border-edge bg-panel/40 transition-colors hover:border-accent"
                data-testid="album-cover-option"
                data-asset-id={asset.id}
              >
                <AssetThumb asset={asset} size={240} className="h-full w-full" skeleton={false} />
              </button>
            ))}
          </div>
          <button
            type="button"
            onClick={() => onPicked(null)}
            className="mt-3 w-full rounded-md border border-edge px-3 py-1.5 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
            data-testid="album-cover-auto"
          >
            {t("albums.coverAuto")}
          </button>
        </>
      )}
    </ModalShell>
  );
}

/** /albums：手工相册区 + 智能相册区（标签墙） */
export function AlbumsIndexPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  // 语义门禁：智能相册即语义查询，模型未齐/索引未建时不进入（行内提示 + 跳设置）
  const gate = useSemanticGate();
  const [gateNotice, setGateNotice] = useState(false);

  // --- 手工相册清单 ---------------------------------------------------------------
  const [albums, setAlbums] = useState<AlbumDto[]>([]);
  const refreshAlbums = useCallback(async () => {
    setAlbums(await albumList());
  }, []);
  useEffect(() => {
    void refreshAlbums();
  }, [refreshAlbums]);
  const covers = useManualAlbumCovers(albums);

  // --- 新建相册（工具条内联输入；重名错误行内提示） -----------------------------------
  const [creating, setCreating] = useState(false);
  const [newName, setNewName] = useState("");
  const [createError, setCreateError] = useState<string | null>(null);
  const [createBusy, setCreateBusy] = useState(false);

  async function submitCreate(): Promise<void> {
    const name = newName.trim();
    if (name === "" || createBusy) return;
    setCreateBusy(true);
    setCreateError(null);
    const result = await albumCreate(name);
    setCreateBusy(false);
    if (!result.ok) {
      // 重名等后端业务错误原文透传；invoke 不可用给通用文案
      setCreateError(result.error ?? t("albums.createFailed"));
      return;
    }
    setAlbums((prev) => [result.album, ...prev]);
    setNewName("");
    setCreating(false);
    setCreateError(null);
  }

  // --- 卡片菜单（右键/悬浮 ⋯）与三个弹层 --------------------------------------------
  const [cardMenu, setCardMenu] = useState<{ x: number; y: number; album: AlbumDto } | null>(null);
  const [renaming, setRenaming] = useState<AlbumDto | null>(null);
  const [renameName, setRenameName] = useState("");
  const [renameError, setRenameError] = useState<string | null>(null);
  // 更改相册文件夹名（目录化：仅改磁盘主目录名，显示名不动）
  const [dirRenaming, setDirRenaming] = useState<AlbumDto | null>(null);
  const [dirRenameName, setDirRenameName] = useState("");
  const [dirRenameError, setDirRenameError] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<AlbumDto | null>(null);
  const [coverPicking, setCoverPicking] = useState<AlbumDto | null>(null);

  async function submitRename(): Promise<void> {
    if (!renaming) return;
    const name = renameName.trim();
    if (name === "") return;
    const result = await albumRename(renaming.id, name);
    if (!result.ok) {
      setRenameError(result.error ?? t("albums.renameFailed"));
      return;
    }
    setAlbums((prev) => prev.map((a) => (a.id === renaming.id ? { ...a, name } : a)));
    setRenaming(null);
  }

  async function submitDirRename(): Promise<void> {
    if (!dirRenaming) return;
    const dirName = dirRenameName.trim();
    if (dirName === "") return;
    const result = await albumDirRename(dirRenaming.id, dirName);
    if (!result.ok) {
      setDirRenameError(result.error ?? t("albums.dirRenameFailed"));
      return;
    }
    setAlbums((prev) => prev.map((a) => (a.id === dirRenaming.id ? { ...a, dirName } : a)));
    setDirRenaming(null);
  }

  async function submitDelete(): Promise<void> {
    if (!deleting) return;
    const ok = await albumDelete(deleting.id);
    if (ok) setAlbums((prev) => prev.filter((a) => a.id !== deleting.id));
    setDeleting(null);
  }

  async function submitCover(album: AlbumDto, assetId: number | null): Promise<void> {
    await albumCoverSet(album.id, assetId);
    setCoverPicking(null);
    // 本地即刻反映（covers 钩子按 coverAssetId 变化自动重取）
    setAlbums((prev) =>
      prev.map((a) => (a.id === album.id ? { ...a, coverAssetId: assetId } : a)),
    );
  }

  function cardMenuEntries(album: AlbumDto): ContextMenuEntry[] {
    // 系统保底相册「未分组」：禁改名（隐藏重命名入口）、禁删（删除项禁用）
    const ungrouped = isUngroupedAlbum(album);
    const entries: ContextMenuEntry[] = [];
    if (!ungrouped) {
      entries.push({
        key: "rename",
        label: t("albums.rename"),
        onSelect: () => {
          setRenaming(album);
          setRenameName(album.name);
          setRenameError(null);
        },
      });
    }
    entries.push({
      key: "dir-rename",
        label: t("albums.dirRename"),
        onSelect: () => {
          setDirRenaming(album);
          setDirRenameName(album.dirName ?? album.name);
          setDirRenameError(null);
        },
      });
    entries.push({
      key: "cover",
      label: t("albums.setCover"),
      onSelect: () => setCoverPicking(album),
    });
    entries.push({
      key: "delete",
      label: t("albums.delete"),
      onSelect: () => setDeleting(album),
      disabled: ungrouped,
      danger: true,
    });
    return entries;
  }

  // --- 智能相册（标签墙，行为照旧） ---------------------------------------------------
  const [visibleTags, setVisibleTags] = useState<string[]>(loadSmartTags);
  const [manualExpanded, setManualExpanded] = useState(true);
  const [smartExpanded, setSmartExpanded] = useState(true);
  useEffect(() => {
    const refresh = () => setVisibleTags(loadSmartTags());
    window.addEventListener("smartphoto:tags-changed", refresh);
    return () => window.removeEventListener("smartphoto:tags-changed", refresh);
  }, []);
  const tagCoverAssetIds = useAlbumCoverAssetIds(visibleTags);

  return (
    <div className="h-full overflow-y-auto" data-testid="albums-page">
      <div className="w-full px-4 pt-4 pb-8">
        {/* 顶部工具条：标题 + 新建相册 */}
        <div className="flex shrink-0 items-center gap-3" data-testid="albums-toolbar">
          <h1 className="text-sm font-semibold text-text-primary">{t("albums.title")}</h1>
          <p className="text-xs text-text-muted">{t("albums.desc")}</p>
          <div className="ml-auto shrink-0" data-testid="albums-create-area">
            {creating ? (
              <div className="flex flex-col items-end gap-1">
                <div className="flex items-center gap-1.5">
                  <input
                    autoFocus
                    type="text"
                    value={newName}
                    onChange={(e) => {
                      setNewName(e.target.value);
                      setCreateError(null);
                    }}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        e.preventDefault();
                        void submitCreate();
                      }
                      if (e.key === "Escape") setCreating(false);
                    }}
                    placeholder={t("albums.newNamePlaceholder")}
                    aria-label={t("albums.createAlbum")}
                    className="h-7 w-44 rounded-md border border-edge bg-panel/55 px-2 text-xs text-text-primary outline-none transition-colors placeholder:text-text-muted/60 focus:border-accent"
                    data-testid="albums-new-name"
                  />
                  <button
                    type="button"
                    onClick={() => void submitCreate()}
                    disabled={createBusy || newName.trim() === ""}
                    className="h-7 rounded-md bg-accent px-3 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:opacity-40"
                    data-testid="albums-new-submit"
                  >
                    {t("albums.createAlbum")}
                  </button>
                  <button
                    type="button"
                    onClick={() => {
                      setCreating(false);
                      setCreateError(null);
                    }}
                    className="h-7 rounded-md border border-edge px-2.5 text-xs text-text-secondary transition-colors hover:border-text-muted"
                    data-testid="albums-new-cancel"
                  >
                    {t("common.cancel")}
                  </button>
                </div>
                {createError !== null && (
                  <p className="text-[11px] text-red-400" role="alert" data-testid="albums-new-error">
                    {createError}
                  </p>
                )}
              </div>
            ) : (
              <button
                type="button"
                onClick={() => {
                  setCreating(true);
                  setNewName("");
                  setCreateError(null);
                }}
                className="flex h-8 items-center gap-2 rounded-md bg-accent px-3 text-xs font-semibold text-black shadow-sm transition-colors hover:brightness-110 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
                data-testid="albums-new-button"
              >
                <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true">
                  <path d="M8 3.5v9M3.5 8h9" />
                </svg>
                {t("albums.createAlbum")}
              </button>
            )}
          </div>
        </div>

        {gateNotice && gate.reason !== null && (
          <div className="mt-3">
            <SemanticGateNotice reason={gate.reason} testId="albums-gate-notice" />
          </div>
        )}

        {/* 手工相册区（区标题样式参考图库按年分块头） */}
        <section className="mt-5" data-testid="albums-manual-section">
          <SectionHeader
            title={t("albums.manualTitle")}
            count={albums.length}
            testId="albums-manual-header"
            expanded={manualExpanded}
            onToggle={() => setManualExpanded((value) => !value)}
          />
          {manualExpanded && (albums.length === 0 ? (
            <p className="py-4 text-xs text-text-muted" data-testid="albums-manual-empty">
              {t("albums.manualEmpty")}
            </p>
          ) : (
            <div
              className="grid grid-cols-[repeat(auto-fill,minmax(120px,1fr))] gap-2.5"
              data-testid="albums-manual-grid"
            >
              {albums.map((album) => (
                <ManualAlbumCard
                  key={album.id}
                  album={album}
                  cover={covers[album.id]}
                  onOpen={(a) => navigate(`/albums/${a.id}`)}
                  onMenu={(at, a) => setCardMenu({ ...at, album: a })}
                />
              ))}
            </div>
          ))}
        </section>

        {/* 智能相册区 */}
        <section className="mt-6" data-testid="albums-tags-section">
          <SectionHeader
            title={t("albums.smartTitle")}
            count={visibleTags.length}
            testId="albums-smart-header"
            expanded={smartExpanded}
            onToggle={() => setSmartExpanded((value) => !value)}
          />
          {smartExpanded && (visibleTags.length === 0 ? (
            <p className="py-4 text-xs text-text-muted" data-testid="albums-all-hidden">
              {t("albums.allHidden")}
            </p>
          ) : (
            <div
              className="grid grid-cols-[repeat(auto-fill,minmax(120px,1fr))] gap-2.5 pb-6"
              data-testid="albums-tag-grid"
            >
              {visibleTags.map((tag) => (
                <button
                  key={tag}
                  type="button"
                  onClick={() => {
                    // 语义门禁：智能相册即语义查询，被拦时不导航（行内提示 + 跳设置）
                    if (gate.blocked) {
                      setGateNotice(true);
                      return;
                    }
                    navigate(`/albums/${encodeURIComponent(tag)}`);
                  }}
                  className="overflow-hidden rounded-lg border border-edge bg-surface text-center transition-colors hover:border-accent"
                  data-testid="albums-tag"
                  data-tag={tag}
                >
                  <span className="block h-20 w-full border-b border-edge/60">
                    <SmartAlbumCover assetId={tagCoverAssetIds[tag]} tag={tag} />
                  </span>
                  <span
                    className="block px-2 py-2 text-sm text-text-secondary transition-colors group-hover:text-accent"
                    data-testid="albums-tag-label"
                  >
                    {smartTagLabel(tag)}
                  </span>
                </button>
              ))}
            </div>
          ))}
        </section>
      </div>

      {/* 卡片右键/悬浮菜单 */}
      {cardMenu && (
        <ContextMenu
          at={{ x: cardMenu.x, y: cardMenu.y }}
          entries={cardMenuEntries(cardMenu.album)}
          onClose={() => setCardMenu(null)}
          testId="albums-card-context-menu"
        />
      )}

      {/* 重命名对话框（重名错误行内提示） */}
      {renaming && (
        <ModalShell
          title={t("albums.renameTitle")}
          onClose={() => setRenaming(null)}
          testId="album-rename-dialog"
          footer={
            <>
              <button
                type="button"
                onClick={() => setRenaming(null)}
                className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
                data-testid="album-rename-cancel"
              >
                {t("common.cancel")}
              </button>
              <button
                type="button"
                onClick={() => void submitRename()}
                disabled={renameName.trim() === "" || renameName.trim() === renaming.name}
                className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
                data-testid="album-rename-confirm"
              >
                {t("albums.renameConfirm")}
              </button>
            </>
          }
        >
          <input
            autoFocus
            type="text"
            value={renameName}
            onChange={(e) => {
              setRenameName(e.target.value);
              setRenameError(null);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void submitRename();
              }
            }}
            aria-label={t("albums.renameTitle")}
            className="h-8 w-full rounded-md border border-edge bg-bg px-2.5 text-xs text-text-primary outline-none transition-colors focus:border-accent"
            data-testid="album-rename-input"
          />
          {renameError !== null && (
            <p className="mt-2 text-[11px] text-red-400" role="alert" data-testid="album-rename-error">
              {renameError}
            </p>
          )}
        </ModalShell>
      )}

      {/* 更改相册文件夹名（目录化：仅改磁盘主目录名，显示名不受影响） */}
      {dirRenaming && (
        <ModalShell
          title={t("albums.dirRenameTitle", { name: dirRenaming.name })}
          onClose={() => setDirRenaming(null)}
          testId="album-dir-rename-dialog"
          footer={
            <>
              <button
                type="button"
                onClick={() => setDirRenaming(null)}
                className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
                data-testid="album-dir-rename-cancel"
              >
                {t("common.cancel")}
              </button>
              <button
                type="button"
                onClick={() => void submitDirRename()}
                disabled={
                  dirRenameName.trim() === "" ||
                  dirRenameName.trim() === (dirRenaming.dirName ?? dirRenaming.name)
                }
                className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
                data-testid="album-dir-rename-confirm"
              >
                {t("albums.dirRenameConfirm")}
              </button>
            </>
          }
        >
          <p className="mb-2 text-[11px] leading-relaxed text-text-muted" data-testid="album-dir-rename-hint">
            {t("albums.dirRenameHint")}
          </p>
          <input
            autoFocus
            type="text"
            value={dirRenameName}
            onChange={(e) => {
              setDirRenameName(e.target.value);
              setDirRenameError(null);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void submitDirRename();
              }
            }}
            aria-label={t("albums.dirRename")}
            className="h-8 w-full rounded-md border border-edge bg-bg px-2.5 text-xs text-text-primary outline-none transition-colors focus:border-accent"
            data-testid="album-dir-rename-input"
          />
          {dirRenameError !== null && (
            <p className="mt-2 text-[11px] text-red-400" role="alert" data-testid="album-dir-rename-error">
              {dirRenameError}
            </p>
          )}
        </ModalShell>
      )}

      {/* 删除确认（红色；仅移除引用，照片保留在图库） */}
      {deleting && (
        <ModalShell
          title={t("albums.deleteTitle", { name: deleting.name })}
          onClose={() => setDeleting(null)}
          testId="album-delete-dialog"
          footer={
            <>
              <button
                type="button"
                onClick={() => setDeleting(null)}
                className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
                data-testid="album-delete-cancel"
              >
                {t("common.cancel")}
              </button>
              <button
                type="button"
                onClick={() => void submitDelete()}
                className="rounded-md bg-red-500/90 px-4 py-1.5 text-xs font-medium text-white transition-colors hover:bg-red-500"
                data-testid="album-delete-confirm"
              >
                {t("albums.deleteConfirm")}
              </button>
            </>
          }
        >
          <p className="text-xs leading-relaxed text-text-secondary" data-testid="album-delete-hint">
            {t("albums.deleteHint", { count: deleting.itemCount })}
          </p>
        </ModalShell>
      )}

      {/* 设为封面选择 */}
      {coverPicking && (
        <CoverPickerDialog
          album={coverPicking}
          onClose={() => setCoverPicking(null)}
          onPicked={(assetId) => void submitCover(coverPicking, assetId)}
        />
      )}
    </div>
  );
}

/** /albums/:tag 参数分发：纯数字 = 手工相册详情；其余 = 智能标签语义结果 */
export function AlbumEntryPage() {
  const { tag = "" } = useParams();
  if (/^\d+$/.test(tag)) return <AlbumDetailPage />;
  return <AlbumTagPage />;
}

/** /albums/:tag：该标签的语义搜索结果（自动执行；输入框可改词重搜，不回写路由） */
export function AlbumTagPage() {
  const { t } = useTranslation();
  const { tag = "" } = useParams();
  const semantic = useSemanticSearch();
  // 语义门禁：被拦时不自动执行、输入框回车不发查询（文字保留）
  const gate = useSemanticGate();
  const [lastQuery, setLastQuery] = useState(tag);
  // 标签结果同样可点开查看器（与画廊/搜索页一致）
  const { cards } = usePhotoCards(semantic.assets);
  const groups = useMemo(() => groupAssetsByDate(cards), [cards]);
  const { viewer, openAsset, closeViewer, navigateTo, selectVersion } = useAssetViewer(groups, semantic.assets);

  // 标签变化（含首挂载）→ 自动语义搜索（门禁未过不发——直接展示拦截提示）
  useEffect(() => {
    if (tag) {
      setLastQuery(tag);
      if (!gate.blocked) void semantic.run(tag);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tag]);

  return (
    <div className="flex h-full flex-col" data-testid="album-tag-page">
      <div className="flex h-11 w-full shrink-0 items-center gap-3 border-b border-edge px-4">
        <h1 className="shrink-0 text-sm font-semibold text-text-primary">
          {t("albums.tagTitle", { tag: smartTagLabel(tag) })}
        </h1>
        <SemanticQueryInput
          busy={semantic.status === "loading"}
          onRun={(q) => {
            if (gate.blocked) return; // 门禁：不发查询；输入文字保留
            setLastQuery(q);
            void semantic.run(q);
          }}
        />
      </div>
      <div className="min-h-0 w-full flex-1 px-4 pt-3">
        {gate.reason !== null && (
          <div className="mb-3">
            <SemanticGateNotice reason={gate.reason} testId="album-tag-gate-notice" />
          </div>
        )}
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
          onVersionSelect={selectVersion}
          onNavigate={navigateTo}
          onClose={closeViewer}
        />
      )}
    </div>
  );
}
