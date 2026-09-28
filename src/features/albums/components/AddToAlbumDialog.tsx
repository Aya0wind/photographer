import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  albumAddAssets,
  albumClaimAssets,
  albumCreate,
  albumList,
  type AlbumDto,
  type AssetDto,
} from "@/ipc/api";
import { useSettingsStore } from "@/stores/settingsStore";
import { importRootOf } from "@/features/onboarding/onboardingConfig";

/**
 * 「加入相册」选择弹窗（③ 全局入口共用：多选操作条 / 瓦片右键菜单）：
 * - 已有相册列表（单选，radio 语义）；底部「新建相册」内联输入（重名错误行内提示，
 *   创建成功自动选中并刷新列表）
 * - 归入语义（规格修订，通用「归入=物理挪移改主相册」）：每张照片都有主相册
 *   （「未分组」= 未真正归类），「归入相册」对所有选择可用——后端支持从未分组/
 *   日期根/任意相册挪到目标相册；所选主相册已是目标相册时归入按钮禁用并提示
 *   「已在该相册」（部分在目标时仅归入其余）；「加入（引用）」不变。
 *   识别口径：照片路径位于 `导入收纳区/相册目录/` 之下即认为主相册为该相册。
 * - 结果 toast：「已归入（文件已移动）」/「已加入（引用）」，随后自动关闭
 */

/** 路径归一（小写 + 统一 "\"），供目录前缀比对 */
function normalizePath(p: string): string {
  return p.replace(/\//g, "\\").toLowerCase();
}

/** 资产是否位于相册主目录 `photoRoot/{相册创建YYYY}/{MM}/{相册目录}/` 之下
 * （即主相册 = 该相册；固定布局公式，与后端 album_home_rel 同口径） */
function isUnderAlbumDir(path: string, importRoot: string, albumHome: string): boolean {
  if (importRoot === "" || albumHome === "") return false;
  const p = normalizePath(path);
  const root = normalizePath(importRoot);
  if (!p.startsWith(`${root}\\`)) return false;
  return p.slice(root.length + 1).startsWith(`${normalizePath(albumHome)}\\`);
}

/** 操作结果（toast 文案区分归入/引用；显示后自动关闭弹窗） */
interface AddResult {
  mode: "claim" | "add";
  count: number;
}

export default function AddToAlbumDialog({
  assets,
  onClose,
}: {
  assets: AssetDto[];
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [albums, setAlbums] = useState<AlbumDto[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [newName, setNewName] = useState("");
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [addError, setAddError] = useState<string | null>(null);
  const [claiming, setClaiming] = useState(false);
  const [claimError, setClaimError] = useState<string | null>(null);
  const [result, setResult] = useState<AddResult | null>(null);

  // 活动库导入收纳区（claim 启发式基准：日期根 = importRoot/日期模板）
  const importRoot = useSettingsStore((s) => {
    const lib = s.settings.libraries.find((l) => l.id === s.settings.activeLibraryId);
    return lib ? importRootOf(lib.photoRoot) : "";
  });

  // 挂载拉相册清单（后端不可用 → 空列表 + 只能新建）
  useEffect(() => {
    let cancelled = false;
    void albumList().then((list) => {
      if (!cancelled) setAlbums(list);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  // 归入语义（规格修订）：以选中目标相册为基准——
  // 「已在该相册」= 路径位于 `photoRoot/{创建YYYY}/{创建MM}/{相册目录}/` 之下
  // （固定布局公式；dir_name 缺省回退显示名）。
  const targetAlbum = albums.find((a) => a.id === selectedId) ?? null;
  const targetHome =
    targetAlbum !== null
      ? `${targetAlbum.createdAt.slice(0, 4)}\\${targetAlbum.createdAt.slice(5, 7)}\\${(targetAlbum.dirName ?? targetAlbum.name).trim()}`
      : "";
  const inTargetIds = useMemo(() => {
    if (targetHome === "") return [];
    return assets
      .filter((a) => isUnderAlbumDir(a.path, importRoot, targetHome))
      .map((a) => a.id);
  }, [assets, importRoot, targetHome]);
  const inTargetCount = inTargetIds.length;
  const claimableIds = useMemo(
    () => assets.map((a) => a.id).filter((id) => !inTargetIds.includes(id)),
    [assets, inTargetIds],
  );
  /** 全部已在目标相册 → 归入禁用；部分在 → 只归入其余；全不在 → 归入全部 */
  const allInTarget = assets.length > 0 && inTargetCount === assets.length;

  // 结果 toast：显示 1.6s 后自动关闭
  useEffect(() => {
    if (result === null) return;
    const timer = window.setTimeout(onClose, 1600);
    return () => window.clearTimeout(timer);
  }, [result, onClose]);

  // 成功 toast 期间组件保持挂载（仅切换为 toast 分支）；父层再次打开（换一组资产）
  // 时重置为表单态——同时经由 result 置空取消上一次的自动关闭定时器。
  const assetsKey = assets.map((a) => a.id).join(",");
  useEffect(() => {
    setResult(null);
    setAddError(null);
    setClaimError(null);
  }, [assetsKey]);

  // Esc 关闭（无输入焦点语义冲突时）
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  async function createAndSelect(): Promise<void> {
    const name = newName.trim();
    if (name === "") return;
    setCreating(true);
    setCreateError(null);
    const created = await albumCreate(name);
    setCreating(false);
    if (!created.ok) {
      // 重名等后端业务错误原文透传；invoke 不可用给通用文案
      setCreateError(created.error ?? t("albums.createFailed"));
      return;
    }
    setAlbums((prev) => [created.album, ...prev]);
    setSelectedId(created.album.id);
    setNewName("");
  }

  /** 归入相册：可归入子集物理挪移并改主相册（通用来源；已在目标的保持不动） */
  async function claim(): Promise<void> {
    if (selectedId === null || claiming || claimableIds.length === 0) return;
    setClaiming(true);
    setClaimError(null);
    try {
      const moved = await albumClaimAssets(selectedId, claimableIds);
      setClaiming(false);
      setResult({ mode: "claim", count: moved ?? claimableIds.length });
    } catch (err) {
      setClaiming(false);
      const message = err instanceof Error ? err.message : String(err);
      setClaimError(`${message}；${t("albums.claimErrorHint")}`);
    }
  }

  async function add(): Promise<void> {
    if (selectedId === null || adding) return;
    setAdding(true);
    setAddError(null);
    const added = await albumAddAssets(
      selectedId,
      assets.map((a) => a.id),
    );
    setAdding(false);
    if (added === null) {
      setAddError(t("albums.addFailed"));
      return;
    }
    setResult({ mode: "add", count: added });
  }

  // 结果 toast：替代弹窗面板短暂展示（随后 onClose 由定时器收尾）
  if (result !== null) {
    return (
      <div className="fixed bottom-6 left-1/2 z-[80] -translate-x-1/2" data-testid="add-to-album-toast" role="status">
        <p className="rounded-full border border-edge bg-surface px-4 py-2 text-xs text-text-secondary shadow-xl">
          {result.mode === "claim"
            ? t("albums.claimedToast", { count: result.count })
            : t("albums.addedRefToast", { count: result.count })}
        </p>
      </div>
    );
  }

  return (
    <div
      className="fixed inset-0 z-[70] flex items-center justify-center bg-black/45"
      onClick={onClose}
      data-testid="add-to-album-overlay"
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={t("albums.addToAlbum")}
        className="flex max-h-[70vh] w-[380px] flex-col overflow-hidden rounded-xl border border-edge bg-surface shadow-2xl"
        onClick={(e) => e.stopPropagation()}
        data-testid="add-to-album-dialog"
      >
        <div className="flex shrink-0 items-center justify-between border-b border-edge px-4 py-3">
          <h2 className="text-sm font-semibold text-text-primary">
            {t("albums.addToAlbum")}
            <span className="ml-2 font-mono text-[11px] font-normal text-text-muted">
              {t("albums.addCount", { count: assets.length })}
            </span>
          </h2>
          <button
            type="button"
            onClick={onClose}
            aria-label={t("common.close")}
            className="rounded px-1.5 text-lg leading-none text-text-muted transition-colors hover:text-text-primary"
            data-testid="add-to-album-close"
          >
            ×
          </button>
        </div>

        {/* 已有相册单选列表 */}
        <div className="sp-scroll min-h-0 flex-1 overflow-y-auto p-2" data-testid="add-to-album-list">
          {albums.length === 0 ? (
            <p className="px-2 py-3 text-xs text-text-muted" data-testid="add-to-album-empty">
              {t("albums.listEmpty")}
            </p>
          ) : (
            albums.map((album) => (
              <label
                key={album.id}
                className="flex cursor-pointer items-center gap-2.5 rounded-md px-2.5 py-2 transition-colors hover:bg-panel/40"
                data-testid="add-to-album-option"
                data-album-id={album.id}
                data-selected={selectedId === album.id}
              >
                <input
                  type="radio"
                  name="add-to-album"
                  checked={selectedId === album.id}
                  onChange={() => setSelectedId(album.id)}
                  className="h-3.5 w-3.5 shrink-0 accent-[#F0A83C]"
                />
                <span className="min-w-0 flex-1 truncate text-xs text-text-secondary" title={album.name}>
                  {album.name}
                </span>
                <span className="shrink-0 font-mono text-[10px] tabular-nums text-text-muted">
                  {t("albums.itemCountBadge", { count: album.itemCount })}
                </span>
              </label>
            ))
          )}
        </div>

        {/* 新建相册内联输入 */}
        <div className="shrink-0 border-t border-edge px-4 py-3">
          <div className="flex items-center gap-2">
            <input
              type="text"
              value={newName}
              onChange={(e) => {
                setNewName(e.target.value);
                setCreateError(null);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  void createAndSelect();
                }
              }}
              placeholder={t("albums.newNamePlaceholder")}
              aria-label={t("albums.createAlbum")}
              className="min-w-0 flex-1 rounded-md border border-edge bg-bg px-2.5 py-1.5 text-xs text-text-primary outline-none transition-colors placeholder:text-text-muted/60 focus:border-accent"
              data-testid="add-to-album-new-name"
            />
            <button
              type="button"
              onClick={() => void createAndSelect()}
              disabled={creating || newName.trim() === ""}
              className="shrink-0 rounded-md border border-edge px-2.5 py-1.5 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent disabled:opacity-40"
              data-testid="add-to-album-new-create"
            >
              {t("albums.createAlbum")}
            </button>
          </div>
          {createError && (
            <p className="mt-1.5 text-[11px] text-red-400" role="alert" data-testid="add-to-album-new-error">
              {createError}
            </p>
          )}
        </div>

        {/* 归入 vs 引用语义说明（选中目标相册后显示） */}
        {targetAlbum !== null && (
          <p
            className="shrink-0 border-t border-edge/60 bg-panel/30 px-4 py-2 text-[11px] leading-relaxed text-text-muted"
            data-testid="add-to-album-mode-hint"
          >
            {allInTarget
              ? t("albums.claimHintInTarget", { name: targetAlbum.name })
              : inTargetCount > 0
                ? t("albums.claimHintMixed", { count: inTargetCount })
                : t("albums.claimHintAll", { count: assets.length })}
          </p>
        )}

        {/* 底部操作条：取消 /（归入相册：已在目标册时禁用）/ 加入（引用） */}
        <div className="flex shrink-0 items-center justify-end gap-2 border-t border-edge px-4 py-3">
          {claimError !== null ? (
            <p className="mr-auto min-w-0 flex-1 truncate text-[11px] text-red-400" role="alert" title={claimError} data-testid="add-to-album-claim-error">
              {claimError}
            </p>
          ) : addError !== null ? (
            <p className="mr-auto text-[11px] text-red-400" role="alert" data-testid="add-to-album-error">
              {addError}
            </p>
          ) : (
            <span className="mr-auto" aria-hidden="true" />
          )}
          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
            data-testid="add-to-album-cancel"
          >
            {t("common.cancel")}
          </button>
          {targetAlbum !== null && (
            <button
              type="button"
              onClick={() => void claim()}
              disabled={allInTarget || claiming}
              title={allInTarget ? t("albums.claimInTarget") : t("albums.claimHint")}
              className={`rounded-md px-4 py-1.5 text-xs font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-40 ${
                inTargetCount > 0 || allInTarget
                  ? "border border-accent/60 bg-accent/10 text-accent hover:bg-accent/20"
                  : "bg-accent text-black hover:brightness-110"
              }`}
              data-testid="add-to-album-claim"
            >
              {allInTarget
                ? t("albums.claimInTarget")
                : inTargetCount > 0
                  ? t("albums.claimManyAction", { count: claimableIds.length })
                  : t("albums.claimAction")}
            </button>
          )}
          <button
            type="button"
            onClick={() => void add()}
            disabled={selectedId === null || adding}
            className="rounded-md border border-edge px-4 py-1.5 text-xs font-medium text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="add-to-album-confirm"
          >
            {t("albums.addRefAction")}
          </button>
        </div>
      </div>
    </div>
  );
}
