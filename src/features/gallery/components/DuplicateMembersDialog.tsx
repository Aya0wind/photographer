import { useTranslation } from "react-i18next";

import type { AssetDto, PhotoLibrary } from "@/ipc/api";
import AssetThumb from "./AssetThumb";

/**
 * 跨库重复项列表（M5 §四，「重复项」列表可看全部）：画廊「隐藏跨库重复」
 * 折叠后，点瓦片 ×N 徽标打开——列出该同哈希组的全部副本（不折叠、权威来自
 * duplicates_list，不受画廊分页影响）。每行：缩略图 + 照片库名 + 在线/缺失
 * 标记 + 评分；可见代表行带 accent 描边。纯查看（折叠 = 纯显示过滤，隐藏副本
 * 不受画廊操作作用；要操作某副本可到其所属照片库过滤视图）。
 */

/** 组内缩略图名义边长（后端 snap 256 档，与网格/相似页同缓存） */
const MEMBER_THUMB_PX = 240;

export default function DuplicateMembersDialog({
  members,
  visibleId,
  libraries,
  onClose,
}: {
  /** 组内全部副本（duplicates_list 原序：导入早在前） */
  members: AssetDto[];
  /** 当前画廊可见代表资产 id（描边标识；折叠关闭时打开则不标） */
  visibleId: number | null;
  /** 照片库登记表（名称/在线状态展示；null=未知按在线） */
  libraries: PhotoLibrary[] | null;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const libraryById = new Map((libraries ?? []).map((l) => [l.id, l]));

  return (
    <div
      className="fixed inset-0 z-[70] flex items-center justify-center bg-black/60 p-6"
      role="dialog"
      aria-modal="true"
      aria-label={t("gallery.duplicates.title")}
      data-testid="duplicates-dialog"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="flex max-h-[80vh] w-full max-w-lg flex-col rounded-xl border border-edge bg-surface p-4 shadow-2xl">
        <div className="flex items-center gap-2">
          <h2 className="text-sm font-semibold text-text-primary" data-testid="duplicates-dialog-title">
            {t("gallery.duplicates.title", { count: members.length })}
          </h2>
          <span className="text-[11px] text-text-muted">{t("gallery.duplicates.hint")}</span>
          <button
            type="button"
            onClick={onClose}
            aria-label={t("ui.close")}
            className="ml-auto flex h-6 w-6 items-center justify-center rounded-md text-text-muted transition-colors hover:bg-panel hover:text-text-primary"
            data-testid="duplicates-dialog-close"
          >
            ×
          </button>
        </div>
        <p className="mt-1 text-[11px] leading-relaxed text-text-muted">
          {t("gallery.duplicates.note")}
        </p>
        <div className="sp-scroll mt-3 flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto pr-1">
          {members.map((asset) => {
            const library = asset.libraryId != null ? libraryById.get(asset.libraryId) : undefined;
            const missing = asset.missing === true || library?.status === "offline";
            const visible = asset.id === visibleId;
            return (
              <div
                key={asset.id}
                className={`flex items-center gap-3 rounded-lg border p-2 ${
                  visible ? "border-accent/70 bg-accent/5" : "border-edge/60"
                }`}
                data-testid="duplicates-dialog-member"
                data-asset-id={asset.id}
                data-visible={visible || undefined}
              >
                <div className="h-12 w-16 shrink-0 overflow-hidden rounded bg-panel/60">
                  <AssetThumb asset={asset} size={MEMBER_THUMB_PX} className="h-full w-full" />
                </div>
                <div className="min-w-0 flex-1">
                  <p className="truncate text-xs text-text-primary" title={asset.path}>
                    {asset.name}
                  </p>
                  <p className="mt-0.5 truncate font-mono text-[10px] text-text-muted" title={asset.path}>
                    {asset.path}
                  </p>
                  <div className="mt-1 flex flex-wrap items-center gap-1.5">
                    <span
                      className="rounded bg-panel px-1.5 py-0.5 text-[10px] leading-none text-text-secondary"
                      data-testid="duplicates-member-library"
                    >
                      {library !== undefined
                        ? library.name
                        : t("gallery.duplicates.unknownLibrary")}
                    </span>
                    {missing && (
                      <span
                        className="rounded bg-amber-300/10 px-1.5 py-0.5 text-[10px] leading-none text-amber-300"
                        data-testid="duplicates-member-missing"
                      >
                        {t("gallery.duplicates.missing")}
                      </span>
                    )}
                    {(asset.rating ?? 0) > 0 && (
                      <span className="rounded bg-panel px-1.5 py-0.5 font-mono text-[10px] leading-none text-amber-400">
                        {t("gallery.duplicates.rating", { count: asset.rating ?? 0 })}
                      </span>
                    )}
                  </div>
                </div>
                {visible && (
                  <span
                    className="shrink-0 rounded-full bg-accent/15 px-2 py-0.5 text-[10px] leading-none text-accent"
                    data-testid="duplicates-member-visible"
                  >
                    {t("gallery.duplicates.visibleTag")}
                  </span>
                )}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
