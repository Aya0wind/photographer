import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  albumAddAssets,
  albumCreate,
  albumList,
  type AlbumDto,
  type AssetDto,
} from "@/ipc/api";

/**
 * 「加入相册」选择弹窗（③ 全局入口共用：多选操作条 / 瓦片右键菜单）：
 * - 已有相册列表（单选，radio 语义）；底部「新建相册」内联输入（重名错误行内提示，
 *   创建成功自动选中并刷新列表）
 * - 确定 → album_add_assets（已引用幂等跳过）；按返回的实际新增数 toast：
 *   「已加入 N 张（M 张已在相册）」，1.4s 后自动关闭
 * - 后端不可用/命令失败 → 行内失败文案（不静默吞掉）
 */

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
  const [toast, setToast] = useState<string | null>(null);
  const closeTimerRef = useRef<number | null>(null);

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

  // toast 出现后 1.4s 自动关弹窗
  useEffect(() => {
    if (toast === null) return;
    closeTimerRef.current = window.setTimeout(onClose, 1400);
    return () => {
      if (closeTimerRef.current !== null) window.clearTimeout(closeTimerRef.current);
    };
  }, [toast, onClose]);

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
    const result = await albumCreate(name);
    setCreating(false);
    if (!result.ok) {
      // 重名等后端业务错误原文透传；invoke 不可用给通用文案
      setCreateError(result.error ?? t("albums.createFailed"));
      return;
    }
    setAlbums((prev) => [result.album, ...prev]);
    setSelectedId(result.album.id);
    setNewName("");
  }

  async function add(): Promise<void> {
    if (selectedId === null || adding) return;
    setAdding(true);
    const added = await albumAddAssets(
      selectedId,
      assets.map((a) => a.id),
    );
    setAdding(false);
    if (added === null) {
      setToast(t("albums.addFailed"));
      return;
    }
    const already = Math.max(0, assets.length - added);
    setToast(already > 0 ? t("albums.addedToast", { added, already }) : t("albums.addedToastAll", { added }));
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

        {/* 底部操作条：取消 / 加入 */}
        <div className="flex shrink-0 items-center justify-end gap-2 border-t border-edge px-4 py-3">
          {toast !== null ? (
            <p className="mr-auto text-[11px] text-accent" role="status" data-testid="add-to-album-toast">
              {toast}
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
          <button
            type="button"
            onClick={() => void add()}
            disabled={selectedId === null || adding || toast !== null}
            className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="add-to-album-confirm"
          >
            {t("albums.addConfirm")}
          </button>
        </div>
      </div>
    </div>
  );
}
