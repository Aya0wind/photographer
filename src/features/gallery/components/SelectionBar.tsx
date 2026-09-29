import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

import {
  assetFlagSet,
  assetLabelSet,
  assetRatingSet,
  assetRejectSet,
  revealInExplorer,
  type AssetDto,
} from "@/ipc/api";
import { COLOR_DOT_CLASS, COLOR_DOT_RING, COLOR_LABELS, type ColorLabel } from "../lib/colorLabels";
import { subgroupSuggestions } from "@/features/albums/lib/ungroupedAlbum";

/**
 * 多选浮动操作条（M4.5，画廊选择模式）：底部居中浮条——已选 N 张 |
 * 收藏（星标=rating 5）/ 旗标 / 颜色标签（LR 五色）/ 拒绝旗标 / 分享
 * （在资源管理器中显示 = opener reveal、复制文件路径）/ 加入相册（③ 全局
 * 入口；弹窗由上层挂载）/ 全选（再点=取消全选）/ 反选（当前数据窗口取补集）/
 * 移入回收站（红色，上层确认一步）/ 取消。
 * 动作对全部选中资产批量调用；失败静默（乐观 UI）。
 * 全部动作均为「切换」语义：再点一次 = 撤销（收藏↔取消、旗标↔取消、
 * 全选↔取消全选、同色色标↔清除；拒绝原本就是智能切换）。
 * 浮条默认画面底部居中，grip 可拖动（位置持久化 localStorage），
 * 弹层一律向上展开。
 * 相册上下文（相册详情页）：额外多一项「从相册移除」——只删引用，照片保留图库。
 */


/** 浮条拖动位置持久化键（视口左上像素坐标）。 */
const BAR_POS_KEY = "selectionbar.pos";

function loadBarPos(): { x: number; y: number } | null {
  try {
    const raw = window.localStorage.getItem(BAR_POS_KEY);
    if (raw === null) return null;
    const p = JSON.parse(raw) as { x?: unknown; y?: unknown };
    if (typeof p.x !== "number" || typeof p.y !== "number") return null;
    // 视口可能变小：夹回范围内，避免拖到再也找不到的角落
    const x = Math.min(Math.max(0, p.x), Math.max(0, window.innerWidth - 80));
    const y = Math.min(Math.max(0, p.y), Math.max(0, window.innerHeight - 40));
    return { x, y };
  } catch {
    return null;
  }
}

function GlyphStar({ filled }: { filled: boolean }) {
  return (
    <svg
      viewBox="0 0 16 16"
      width="12"
      height="12"
      fill={filled ? "currentColor" : "none"}
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M8 1.8l1.8 3.7 4 .6-2.9 2.8.7 4L8 11l-3.6 1.9.7-4L2.2 6.1l4-.6z" />
    </svg>
  );
}

function GlyphFlag() {
  return (
    <svg
      viewBox="0 0 16 16"
      width="12"
      height="12"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M3.5 14V2.5M3.5 3h7l-1 2.5 1 2.5h-7" />
    </svg>
  );
}

function GlyphShare() {
  return (
    <svg
      viewBox="0 0 16 16"
      width="12"
      height="12"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M8 10V2.5M5.5 5L8 2.5 10.5 5" />
      <path d="M3 8.5v4a1.5 1.5 0 0 0 1.5 1.5h7a1.5 1.5 0 0 0 1.5-1.5v-4" />
    </svg>
  );
}

function GlyphAlbum() {
  return (
    <svg
      viewBox="0 0 16 16"
      width="12"
      height="12"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <rect x="1.5" y="3" width="13" height="10.5" rx="1.5" />
      <path d="M1.5 6h13M5 1.5h6" />
    </svg>
  );
}

function GlyphTrash() {
  return (
    <svg
      viewBox="0 0 16 16"
      width="12"
      height="12"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M2.5 4h11M6.5 2h3M4 4l.7 9a1.5 1.5 0 0 0 1.5 1.4h3.6a1.5 1.5 0 0 0 1.5-1.4L12 4" />
    </svg>
  );
}


function GlyphInvert() {
  return (
    <svg
      viewBox="0 0 16 16"
      width="12"
      height="12"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M13.5 8a5.5 5.5 0 1 1-5.5-5.5v11z" fill="currentColor" stroke="none" />
      <circle cx="8" cy="8" r="5.5" />
    </svg>
  );
}

/** 色点（popup 选项/清除行共用） */
function ColorDot({ label }: { label: ColorLabel }) {
  return (
    <span
      className={`h-3.5 w-3.5 rounded-full ${COLOR_DOT_CLASS[label]} ${COLOR_DOT_RING}`}
      aria-hidden="true"
    />
  );
}

export default function SelectionBar({
  count,
  assets,
  onDone,
  onAddToAlbum,
  onFavoritesChanged,
  subgroup,
  onColorLabeled,
  onRejected,
  onTrashRequest,
  windowIds,
  onSelectAll,
  onInvert,
}: {
  count: number;
  assets: AssetDto[];
  onDone: () => void;
  /** 「加入相册」入口回调（弹窗由上层挂载）；不传则不显示该按钮 */
  onAddToAlbum?: (assets: AssetDto[]) => void;
  /** 收藏切换完成（IPC 后同步本地列表态）；favorite = 本轮切换到的目标态 */
  onFavoritesChanged?: (assets: AssetDto[], favorite: boolean) => void;
  /** 子分组上下文（B4，相册详情页传入）：多选操作条「移到子分组…/移到相册根」 */
  subgroup?: {
    /** 当前视图所在子分组名；null = 相册根 */
    current: string | null;
    /** 相册已有子分组名（弹窗内选择/提示） */
    names: string[];
    /** 移组（target=null 移回根）；返回成功与否供提示 */
    onMove: (target: string | null, assets: AssetDto[]) => Promise<boolean>;
  };
  /** 颜色标签设置完成（IPC 后同步本地列表态）；不传则仅写后端 */
  onColorLabeled?: (assets: AssetDto[], label: string | null) => void;
  /** 拒绝旗标切换完成（IPC 后同步本地列表态） */
  onRejected?: (assets: AssetDto[], rejected: boolean) => void;
  /** 「移入回收站」请求（确认弹窗由上层挂载）；不传则不显示该按钮 */
  onTrashRequest?: (assets: AssetDto[]) => void;
  /** 全选完成（上层以窗口全量或空集替换选中集）；与 windowIds 同给才显示按钮 */
  onSelectAll?: (ids: number[]) => void;
  /** 反选的数据窗口（当前已加载资产 id 全集）；与 onInvert 同给才显示按钮 */
  windowIds?: number[];
  /** 反选完成（上层以补集替换选中集） */
  onInvert?: (ids: number[]) => void;
}) {
  const { t } = useTranslation();
  const [shareOpen, setShareOpen] = useState(false);
  const [colorOpen, setColorOpen] = useState(false);
  const [subgroupOpen, setSubgroupOpen] = useState(false);
  const [subgroupName, setSubgroupName] = useState("");
  const [toast, setToast] = useState<string | null>(null);
  const [barPos, setBarPos] = useState<{ x: number; y: number } | null>(loadBarPos);
  const shareRef = useRef<HTMLDivElement | null>(null);
  const colorRef = useRef<HTMLDivElement | null>(null);
  const subgroupRef = useRef<HTMLDivElement | null>(null);
  const barRef = useRef<HTMLDivElement | null>(null);

  // 点击浮层菜单外关闭
  useEffect(() => {
    if (!shareOpen && !colorOpen && !subgroupOpen) return;
    const onDown = (e: MouseEvent) => {
      if (e.target instanceof Node) {
        if (shareOpen && shareRef.current && !shareRef.current.contains(e.target)) setShareOpen(false);
        if (colorOpen && colorRef.current && !colorRef.current.contains(e.target)) setColorOpen(false);
        if (subgroupOpen && subgroupRef.current && !subgroupRef.current.contains(e.target)) setSubgroupOpen(false);
      }
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [shareOpen, colorOpen, subgroupOpen]);

  function flash(message: string): void {
    setToast(message);
    window.setTimeout(() => setToast(null), 1500);
  }

  /** 浮条拖动：grip 按下 → pointer 跟随（限视口内）→ 抬起持久化。 */
  function beginBarDrag(e: React.PointerEvent<HTMLButtonElement>): void {
    if (e.button !== 0) return;
    const bar = barRef.current;
    if (bar === null) return;
    setShareOpen(false);
    setColorOpen(false);
    setSubgroupOpen(false);
    const offX = e.clientX - bar.getBoundingClientRect().left;
    const offY = e.clientY - bar.getBoundingClientRect().top;
    let last: { x: number; y: number } | null = null;
    const onMove = (ev: PointerEvent) => {
      const el = barRef.current;
      if (el === null) return;
      const x = Math.min(Math.max(0, ev.clientX - offX), Math.max(0, window.innerWidth - el.offsetWidth));
      const y = Math.min(Math.max(0, ev.clientY - offY), Math.max(0, window.innerHeight - el.offsetHeight));
      last = { x, y };
      setBarPos(last);
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      if (last !== null) {
        try {
          window.localStorage.setItem(BAR_POS_KEY, JSON.stringify(last));
        } catch {
          /* 持久化失败：本次会话内仍有效 */
        }
      }
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  }

  /** 收藏切换：全部已收藏（rating 5）→ 取消；否则批量收藏。 */
  async function favorite(): Promise<void> {
    const target = !allFavorited;
    setFavOverride(target);
    for (const asset of assets) await assetRatingSet(asset.id, target ? 5 : 0);
    onFavoritesChanged?.(assets, target);
    flash(t("selection.done"));
  }

  /** 旗标切换：全部已旗标 → 取消；否则批量旗标。 */
  async function flag(): Promise<void> {
    const target = !allFlagged;
    setFlagOverride(target);
    for (const asset of assets) await assetFlagSet(asset.id, target);
    flash(t("selection.done"));
  }

  /** 颜色标签切换（label=null 清除；全部同色再点 = 清除该色） */
  async function colorLabel(label: string | null): Promise<void> {
    setColorOpen(false);
    const target =
      label !== null && !(assets.length > 0 && assets.every((a) => a.colorLabel === label)) ? label : null;
    const ids = assets.map((a) => a.id);
    await assetLabelSet(ids, target);
    onColorLabeled?.(assets, target);
    flash(target === null ? t("selection.colorCleared") : t("selection.done"));
  }

  // 收藏/旗标切换态的乐观覆盖：点击后立即翻转按钮文案（上层未回写
  // flagged 的页面也能正确显示「取消×」）；选中集变化时重置。
  const idsKey = assets.map((a) => a.id).join(",");
  const [favOverride, setFavOverride] = useState<boolean | null>(null);
  const [flagOverride, setFlagOverride] = useState<boolean | null>(null);
  useEffect(() => {
    setFavOverride(null);
    setFlagOverride(null);
  }, [idsKey]);
  const allFavorited = assets.length > 0 && (favOverride ?? assets.every((a) => a.rating === 5));
  const allFlagged = assets.length > 0 && (flagOverride ?? assets.every((a) => a.flagged));

  // 拒绝旗标智能切换：全部已拒绝 → 取消拒绝；否则批量拒绝
  const allRejected = assets.length > 0 && assets.every((a) => a.rejected === true);

  async function toggleReject(): Promise<void> {
    const next = !allRejected;
    const ids = assets.map((a) => a.id);
    await assetRejectSet(ids, next);
    onRejected?.(assets, next);
    flash(t("selection.done"));
  }

  async function reveal(): Promise<void> {
    setShareOpen(false);
    let ok = 0;
    try {
      // 批量单窗定位（同目录多文件=1 窗多选）；失败回退逐个
      ok = await revealInExplorer(assets.map((a) => a.path));
    } catch {
      for (const asset of assets) {
        try {
          await revealItemInDir(asset.path);
          ok += 1;
        } catch {
          // 非 Tauri 环境静默
        }
      }
    }
    flash(ok > 0 ? t("selection.done") : t("selection.revealUnavailable"));
  }

  async function copyPaths(): Promise<void> {
    setShareOpen(false);
    const text = assets.map((a) => a.path).join("\n");
    try {
      await navigator.clipboard.writeText(text);
      flash(t("selection.copied"));
    } catch {
      flash(t("selection.copyFailed"));
    }
  }

  /** 子分组移组（B4）：target=null 移回相册根；新名输入即建（后端按名幂等） */
  async function moveSubgroup(target: string | null): Promise<void> {
    if (!subgroup) return;
    setSubgroupOpen(false);
    const ok = await subgroup.onMove(target, assets);
    flash(ok ? t("selection.done") : t("albums.subgroupMoveFailed"));
  }


  return (
    <div
      ref={barRef}
      className={`fixed z-30 ${barPos === null ? "bottom-5 left-1/2 w-max max-w-[calc(100vw-2rem)] -translate-x-1/2" : ""}`}
      style={barPos === null ? undefined : { left: barPos.x, top: barPos.y }}
      data-testid="selection-bar"
      data-count={count}
    >
      <div className="flex items-center gap-1.5 rounded-full border border-edge bg-surface px-3 py-1.5 shadow-xl">
        {/* 拖动把手（挪位置；默认底部居中不遮上方 UI） */}
        <button
          type="button"
          onPointerDown={beginBarDrag}
          className="flex h-7 w-6 shrink-0 cursor-grab touch-none items-center justify-center rounded-full text-text-muted/70 transition-colors hover:bg-panel hover:text-text-secondary active:cursor-grabbing"
          title={t("selection.dragHint")}
          aria-label={t("selection.dragHint")}
          data-testid="selection-bar-grip"
        >
          <svg viewBox="0 0 16 16" width="12" height="12" fill="currentColor" aria-hidden="true">
            <circle cx="5.5" cy="3.5" r="1.2" /><circle cx="10.5" cy="3.5" r="1.2" />
            <circle cx="5.5" cy="8" r="1.2" /><circle cx="10.5" cy="8" r="1.2" />
            <circle cx="5.5" cy="12.5" r="1.2" /><circle cx="10.5" cy="12.5" r="1.2" />
          </svg>
        </button>
        <span className="shrink-0 font-mono text-[11px] tabular-nums text-accent" data-testid="selection-count">
          {t("selection.count", { count })}
        </span>
        <span className="h-4 w-px bg-edge" aria-hidden="true" />
        <button
          type="button"
          onClick={() => void favorite()}
          disabled={count === 0}
          className="flex items-center gap-1 rounded-full px-2 py-1 text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent disabled:opacity-40"
          data-testid="selection-favorite"
        >
          <GlyphStar filled={allFavorited} />
          {allFavorited ? t("selection.unfavorite") : t("selection.favorite")}
        </button>
        <button
          type="button"
          onClick={() => void flag()}
          disabled={count === 0}
          className="flex items-center gap-1 rounded-full px-2 py-1 text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent disabled:opacity-40"
          data-testid="selection-flag"
        >
          <GlyphFlag />
          {allFlagged ? t("selection.unflag") : t("selection.flag")}
        </button>
        {/* 颜色标签（LR 五色）：弹出五色点 + 清除行，批量作用于选中集 */}
        <div ref={colorRef} className="relative">
          <button
            type="button"
            onClick={() => setColorOpen((v) => !v)}
            disabled={count === 0}
            aria-expanded={colorOpen}
            className="flex items-center gap-1 rounded-full px-2 py-1 text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent disabled:opacity-40"
            data-testid="selection-color"
          >
            <span className={`h-3 w-3 rounded-full ${COLOR_DOT_RING} bg-panel`} aria-hidden="true" />
            {t("selection.colorLabel")}
          </button>
          {colorOpen && (
            <div
              className="absolute bottom-9 left-0 z-40 flex w-max items-center gap-1.5 rounded-lg border border-edge bg-surface p-2 shadow-xl"
              data-testid="selection-color-menu"
            >
              {COLOR_LABELS.map((label) => (
                <button
                  key={label}
                  type="button"
                  title={t(`gallery.color.${label}`)}
                  aria-label={t(`gallery.color.${label}`)}
                  onClick={() => void colorLabel(label)}
                  className="rounded-full p-1 transition-transform hover:scale-110"
                  data-testid="selection-color-option"
                  data-label={label}
                >
                  <ColorDot label={label} />
                </button>
              ))}
              <span className="h-4 w-px bg-edge" aria-hidden="true" />
              <button
                type="button"
                onClick={() => void colorLabel(null)}
                title={t("selection.colorClear")}
                aria-label={t("selection.colorClear")}
                className="rounded-full p-1 text-text-muted transition-colors hover:bg-panel hover:text-text-primary"
                data-testid="selection-color-clear"
              >
                <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true">
                  <path d="M4 4l8 8M12 4l-8 8" />
                </svg>
              </button>
            </div>
          )}
        </div>
        {/* 拒绝旗标（与星级分层）：全部已拒绝时显示「取消拒绝」 */}
        <button
          type="button"
          onClick={() => void toggleReject()}
          disabled={count === 0}
          aria-pressed={allRejected}
          className={`flex items-center gap-1 rounded-full px-2 py-1 text-[11px] transition-colors disabled:opacity-40 ${
            allRejected
              ? "bg-red-400/15 text-red-400"
              : "text-text-secondary hover:bg-red-400/10 hover:text-red-400"
          }`}
          data-testid="selection-reject"
        >
          <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <circle cx="8" cy="8" r="5.6" />
            <path d="M4.2 11.8l7.6-7.6" />
          </svg>
          {allRejected ? t("selection.unreject") : t("selection.reject")}
        </button>
        <div ref={shareRef} className="relative">
          <button
            type="button"
            onClick={() => setShareOpen((v) => !v)}
            disabled={count === 0}
            aria-expanded={shareOpen}
            className="flex items-center gap-1 rounded-full px-2 py-1 text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent disabled:opacity-40"
            data-testid="selection-share"
          >
            <GlyphShare />
            {t("selection.share")}
          </button>
          {shareOpen && (
            <div
              className="absolute bottom-9 left-0 z-40 w-44 overflow-hidden rounded-lg border border-edge bg-surface p-1 shadow-xl"
              data-testid="selection-share-menu"
            >
              <button
                type="button"
                onClick={() => void reveal()}
                className="block w-full rounded px-2 py-1.5 text-left text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent"
                data-testid="selection-share-reveal"
              >
                {t("selection.reveal")}
              </button>
              <button
                type="button"
                onClick={() => void copyPaths()}
                className="block w-full rounded px-2 py-1.5 text-left text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent"
                data-testid="selection-share-copy"
              >
                {t("selection.copyPath")}
              </button>
            </div>
          )}
        </div>
        <span className="h-4 w-px bg-edge" aria-hidden="true" />
        {onAddToAlbum && (
          <button
            type="button"
            onClick={() => onAddToAlbum(assets)}
            disabled={count === 0}
            className="flex items-center gap-1 rounded-full px-2 py-1 text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent disabled:opacity-40"
            data-testid="selection-add-album"
          >
            <GlyphAlbum />
            {t("albums.addToAlbum")}
          </button>
        )}
        {subgroup && (
          <div ref={subgroupRef} className="relative">
            <button
              type="button"
              onClick={() => {
                setSubgroupOpen((v) => !v);
                setSubgroupName("");
              }}
              disabled={count === 0}
              aria-expanded={subgroupOpen}
              title={subgroup.current === null ? t("albums.moveToSubgroup") : t("albums.moveToSubgroupHint")}
              className="flex items-center gap-1 rounded-full px-2 py-1 text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent disabled:opacity-40"
              data-testid="selection-subgroup-move"
            >
              {t("albums.moveToSubgroup")}
            </button>
            {subgroupOpen && (
              <div
                className="absolute bottom-9 left-0 z-40 w-52 rounded-lg border border-edge bg-surface p-2 shadow-xl"
                data-testid="selection-subgroup-menu"
              >
                {subgroup.current !== null && (
                  <button
                    type="button"
                    onClick={() => void moveSubgroup(null)}
                    className="mb-1.5 block w-full rounded px-2 py-1.5 text-left text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent"
                    data-testid="selection-subgroup-root"
                  >
                    {t("albums.moveToRoot")}
                  </button>
                )}
                <input
                  type="text"
                  value={subgroupName}
                  onChange={(e) => setSubgroupName(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      const name = subgroupName.trim();
                      if (name !== "") void moveSubgroup(name);
                    }
                  }}
                  list="selection-subgroup-datalist"
                  placeholder={t("albums.subgroupInputPlaceholder")}
                  aria-label={t("albums.moveToSubgroup")}
                  className="h-7 w-full rounded-md border border-edge bg-bg px-2 text-[11px] text-text-primary outline-none transition-colors placeholder:text-text-muted/60 focus:border-accent"
                  data-testid="selection-subgroup-name"
                />
                <datalist id="selection-subgroup-datalist">
                  {subgroupSuggestions(subgroup.names)
                    .filter((n) => n !== subgroup.current)
                    .map((n) => (
                      <option key={n} value={n} />
                    ))}
                </datalist>
                <button
                  type="button"
                  onClick={() => {
                    const name = subgroupName.trim();
                    if (name !== "") void moveSubgroup(name);
                  }}
                  disabled={subgroupName.trim() === ""}
                  className="mt-1.5 w-full rounded-md bg-accent px-2 py-1.5 text-[11px] font-medium text-black transition-colors hover:brightness-110 disabled:opacity-40"
                  data-testid="selection-subgroup-confirm"
                >
                  {t("albums.subgroupMoveConfirm", { name: subgroupName.trim() || "…" })}
                </button>
              </div>
            )}
          </div>
        )}
        {/* 全选/取消全选：当前数据窗口全量（再点一次 = 全部取消） */}
        {windowIds !== undefined && onSelectAll && (
          <button
            type="button"
            onClick={() => {
              const allSelected = count > 0 && count === windowIds.length;
              onSelectAll(allSelected ? [] : windowIds);
              flash(t("selection.done"));
            }}
            disabled={count === 0 && windowIds.length === 0}
            className="flex items-center gap-1 rounded-full px-2 py-1 text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent disabled:opacity-40"
            data-testid="selection-all"
          >
            <GlyphInvert />
            {count > 0 && count === windowIds.length ? t("selection.deselectAll") : t("selection.all")}
          </button>
        )}
        {/* 反选：当前数据窗口内取补集（窗口 id 全集由上层传入） */}
        {windowIds !== undefined && onInvert && (
          <button
            type="button"
            onClick={() => {
              const selectedSet = new Set(assets.map((a) => a.id));
              onInvert(windowIds.filter((id) => !selectedSet.has(id)));
              flash(t("selection.done"));
            }}
            disabled={count === 0 && windowIds.length === 0}
            className="flex items-center gap-1 rounded-full px-2 py-1 text-[11px] text-text-secondary transition-colors hover:bg-panel hover:text-accent disabled:opacity-40"
            data-testid="selection-invert"
          >
            <GlyphInvert />
            {t("selection.invert")}
          </button>
        )}
        {onTrashRequest && (
          <button
            type="button"
            onClick={() => onTrashRequest(assets)}
            disabled={count === 0}
            title={t("selection.trashHint")}
            className="flex items-center gap-1 rounded-full px-2 py-1 text-[11px] text-red-400 transition-colors hover:bg-red-400/10 disabled:opacity-40"
            data-testid="selection-trash"
          >
            <GlyphTrash />
            {t("selection.trash")}
          </button>
        )}
        <span className="h-4 w-px bg-edge" aria-hidden="true" />
        <button
          type="button"
          onClick={onDone}
          className="rounded-full px-2 py-1 text-[11px] text-text-muted transition-colors hover:bg-panel hover:text-text-primary"
          data-testid="selection-cancel"
        >
          {t("common.cancel")}
        </button>
      </div>
      {toast !== null && (
        <p
          className="mt-1.5 text-center text-[11px] text-text-muted"
          data-testid="selection-toast"
          role="status"
        >
          {toast}
        </p>
      )}
    </div>
  );
}
