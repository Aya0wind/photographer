import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { albumCreate, albumSubgroups, type AlbumDto } from "@/ipc/api";

/**
 * 编辑器「加入相册」导出目标选择（程序内对话框，2026-09-29 导出重做）：
 * 单选已有相册 + 可选子组（按所选相册已有子组提示）+ 内联新建（成功直接
 * 以新相册确认导出）。确认后由上层以 album 模式导出（派生成片入册）。
 */

interface ExportAlbumPickerProps {
  albums: AlbumDto[];
  onCancel: () => void;
  onConfirm: (albumId: number, subgroup: string | null) => void;
}

export default function ExportAlbumPicker({ albums, onCancel, onConfirm }: ExportAlbumPickerProps) {
  const { t } = useTranslation();
  const [selectedId, setSelectedId] = useState<number | null>(albums[0]?.id ?? null);
  const [subgroup, setSubgroup] = useState("");
  const [subgroups, setSubgroups] = useState<string[]>([]);
  const [newName, setNewName] = useState("");
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  // 所选相册的已有子组（datalist 提示；切相册时清子组输入）
  useEffect(() => {
    setSubgroup("");
    if (selectedId === null) {
      setSubgroups([]);
      return;
    }
    let cancelled = false;
    void albumSubgroups(selectedId).then((list) => {
      if (!cancelled) setSubgroups(list.map((g) => g.name));
    }).catch(() => {
      if (!cancelled) setSubgroups([]);
    });
    return () => {
      cancelled = true;
    };
  }, [selectedId]);

  async function createAndConfirm(): Promise<void> {
    const name = newName.trim();
    if (name === "" || creating) return;
    setCreating(true);
    setCreateError(null);
    const created = await albumCreate(name);
    setCreating(false);
    if (!created.ok) {
      // 重名等后端业务错误原文透传
      setCreateError(created.error ?? t("albums.createFailed"));
      return;
    }
    onConfirm(created.album.id, null);
  }

  return (
    <div
      className="fixed inset-0 z-[90] flex items-center justify-center bg-black/50"
      onClick={onCancel}
      data-testid="export-album-overlay"
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={t("editor.albumPicker.title")}
        className="flex max-h-[70vh] w-[380px] flex-col overflow-hidden rounded-xl border border-edge bg-surface shadow-2xl"
        onClick={(e) => e.stopPropagation()}
        data-testid="export-album-picker"
      >
        <div className="flex shrink-0 items-center justify-between border-b border-edge px-4 py-3">
          <h2 className="text-sm font-semibold text-text-primary">
            {t("editor.albumPicker.title")}
          </h2>
          <button
            type="button"
            onClick={onCancel}
            aria-label={t("common.close")}
            className="rounded px-1.5 text-lg leading-none text-text-muted transition-colors hover:text-text-primary"
            data-testid="export-album-close"
          >
            ×
          </button>
        </div>

        {/* 已有相册单选列表 */}
        <div className="sp-scroll min-h-0 flex-1 overflow-y-auto p-2" data-testid="export-album-list">
          {albums.length === 0 ? (
            <p className="px-2 py-3 text-xs text-text-muted" data-testid="export-album-empty">
              {t("editor.albumPicker.empty")}
            </p>
          ) : (
            albums.map((album) => (
              <label
                key={album.id}
                className="flex cursor-pointer items-center gap-2.5 rounded-md px-2.5 py-2 transition-colors hover:bg-panel/40"
                data-testid="export-album-option"
                data-album-id={album.id}
                data-selected={selectedId === album.id}
              >
                <input
                  type="radio"
                  name="export-album"
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

        {/* 子组（可选）+ 新建相册内联输入 */}
        <div className="shrink-0 space-y-2 border-t border-edge px-4 py-3">
          <label className="block" data-testid="export-album-subgroup-row">
            <span className="mb-1 block text-[11px] text-text-muted">{t("editor.output.subgroup")}</span>
            <input
              type="text"
              list="export-album-subgroup-options"
              value={subgroup}
              onChange={(e) => setSubgroup(e.target.value)}
              placeholder={t("editor.output.subgroupPlaceholder")}
              className="w-full rounded-md border border-edge bg-bg px-2 py-1.5 text-xs text-text-primary outline-none placeholder:text-text-muted/60 focus:border-accent"
              data-testid="export-album-subgroup"
            />
            <datalist id="export-album-subgroup-options">
              {subgroups.map((name) => (
                <option key={name} value={name} />
              ))}
            </datalist>
          </label>
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
                  void createAndConfirm();
                }
              }}
              placeholder={t("albums.newNamePlaceholder")}
              aria-label={t("albums.createAlbum")}
              className="min-w-0 flex-1 rounded-md border border-edge bg-bg px-2.5 py-1.5 text-xs text-text-primary outline-none transition-colors placeholder:text-text-muted/60 focus:border-accent"
              data-testid="export-album-new-name"
            />
            <button
              type="button"
              onClick={() => void createAndConfirm()}
              disabled={creating || newName.trim() === ""}
              className="shrink-0 rounded-md border border-edge px-2.5 py-1.5 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent disabled:opacity-40"
              data-testid="export-album-new-create"
            >
              {t("albums.createAlbum")}
            </button>
          </div>
          {createError && (
            <p className="text-[11px] text-red-400" role="alert" data-testid="export-album-new-error">
              {createError}
            </p>
          )}
        </div>

        <div className="flex shrink-0 items-center justify-end gap-2 border-t border-edge px-4 py-3">
          <button
            type="button"
            onClick={onCancel}
            className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
            data-testid="export-album-cancel"
          >
            {t("common.cancel")}
          </button>
          <button
            type="button"
            onClick={() => onConfirm(selectedId as number, subgroup.trim() === "" ? null : subgroup.trim())}
            disabled={selectedId === null}
            className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="export-album-confirm"
          >
            {t("editor.albumPicker.confirm")}
          </button>
        </div>
      </div>
    </div>
  );
}
