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
 * - 语义分层（B1 追加包，相册物理目录化）：
 *   · 所选照片全部在日期根未归册 → 默认动作突出「归入相册」（album_claim_assets，
 *     物理移动文件到相册目录）；旁边保留「加入（引用）」
 *   · 混合选择（部分已归册）：双档按钮——归入只作用于未归册子集，加入引用作用于全部
 *   · 全部已归册 → 维持「加入（引用）」（album_add_assets）
 *   归入失败（如已在别册主目录）透传后端 Err 原文 + 改用引用的提示。
 * - 结果 toast：「已归入（文件已移动）」/「已加入（引用）」，随后自动关闭
 */

/** 路径归一（小写 + 统一 "\"），供目录前缀比对 */
function normalizePath(p: string): string {
  return p.replace(/\//g, "\\").toLowerCase();
}

/** 相册主目录名清单（dir_name 缺省回退显示名） */
function albumDirsOf(albums: AlbumDto[]): string[] {
  return albums
    .map((a) => (a.dirName ?? a.name).trim())
    .filter((s) => s !== "");
}

/** 资产是否「日期根未归册」：位于导入收纳区下且不在任何相册主目录内 */
function isClaimable(path: string, importRoot: string, albumDirs: string[]): boolean {
  if (importRoot === "") return false;
  const p = normalizePath(path);
  const root = normalizePath(importRoot);
  if (!p.startsWith(`${root}\\`)) return false;
  const rel = p.slice(root.length + 1);
  return !albumDirs.some((dir) => rel.startsWith(`${normalizePath(dir)}\\`));
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
    return lib ? importRootOf(lib.photoRoot, lib.importSubdir) : "";
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

  // 归入候选：未归册子集（挂载相册清单到位后重算）
  const albumDirs = useMemo(() => albumDirsOf(albums), [albums]);
  const claimableIds = useMemo(
    () => assets.filter((a) => isClaimable(a.path, importRoot, albumDirs)).map((a) => a.id),
    [assets, importRoot, albumDirs],
  );
  const claimCount = claimableIds.length;
  const allClaimable = assets.length > 0 && claimCount === assets.length;

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

  /** 归入相册：未归册子集物理移动文件到相册目录（已在别册的整批报错透传） */
  async function claim(): Promise<void> {
    if (selectedId === null || claiming || claimCount === 0) return;
    setClaiming(true);
    setClaimError(null);
    try {
      const moved = await albumClaimAssets(selectedId, claimableIds);
      setClaiming(false);
      setResult({ mode: "claim", count: moved ?? claimCount });
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

        {/* 归入 vs 引用语义说明（存在可归入子集时显示） */}
        {claimCount > 0 && (
          <p
            className="shrink-0 border-t border-edge/60 bg-panel/30 px-4 py-2 text-[11px] leading-relaxed text-text-muted"
            data-testid="add-to-album-mode-hint"
          >
            {allClaimable ? t("albums.claimHintAll", { count: claimCount }) : t("albums.claimHintMixed", { count: claimCount })}
          </p>
        )}

        {/* 底部操作条：取消 /（归入相册）/ 加入（引用） */}
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
          {claimCount > 0 && (
            <button
              type="button"
              onClick={() => void claim()}
              disabled={selectedId === null || claiming}
              title={t("albums.claimHint")}
              className={`rounded-md px-4 py-1.5 text-xs font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-40 ${
                allClaimable
                  ? "bg-accent text-black hover:brightness-110"
                  : "border border-accent/60 bg-accent/10 text-accent hover:bg-accent/20"
              }`}
              data-testid="add-to-album-claim"
            >
              {allClaimable
                ? t("albums.claimAction")
                : t("albums.claimManyAction", { count: claimCount })}
            </button>
          )}
          <button
            type="button"
            onClick={() => void add()}
            disabled={selectedId === null || adding}
            className={`rounded-md px-4 py-1.5 text-xs font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-40 ${
              claimCount > 0
                ? "border border-edge text-text-secondary hover:border-text-muted hover:text-text-primary"
                : "bg-accent text-black hover:brightness-110"
            }`}
            data-testid="add-to-album-confirm"
          >
            {claimCount > 0 ? t("albums.addRefAction") : t("albums.addConfirm")}
          </button>
        </div>
      </div>
    </div>
  );
}
