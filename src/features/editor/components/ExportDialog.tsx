import { useMemo } from "react";
import { useTranslation } from "react-i18next";

import type { AlbumDto, AssetDto, EditRecipe, ExportOptions } from "@/ipc/api";
import { parseLongEdge, validateExportDraft, type ExportOptionsDraft } from "../lib/exportOptions";

/**
 * 导出确认弹窗：选项摘要 + 校验错误拦截（校验失败时确认禁用）。
 * 摘要含元数据写入目标说明——只写导出的新 JPEG，不改原片/原片 XMP（§8 要求
 * 让用户知道哪些字段影响原件）。
 */

interface ExportDialogProps {
  asset: AssetDto;
  recipe: EditRecipe;
  draft: ExportOptionsDraft;
  albums: AlbumDto[];
  /** 已有导出任务在跑（防重复启动） */
  running: boolean;
  onCancel: () => void;
  onConfirm: (options: ExportOptions) => void;
}

export default function ExportDialog({
  asset,
  recipe,
  draft,
  albums,
  running,
  onCancel,
  onConfirm,
}: ExportDialogProps) {
  const { t } = useTranslation();
  const validation = useMemo(
    () => validateExportDraft(draft, recipe.output.quality),
    [draft, recipe.output.quality],
  );

  const album = draft.mode === "album" ? albums.find((a) => String(a.id) === draft.albumId) : undefined;
  const longEdge = parseLongEdge(draft.longEdge);

  const rows: { label: string; value: string }[] = [
    {
      label: t("editor.output.mode"),
      value:
        draft.mode === "folder"
          ? t("editor.summary.folder", { dir: draft.outputDir || "—", name: draft.fileName || "—" })
          : album !== undefined
            ? draft.subgroup.trim() !== ""
              ? t("editor.summary.albumSub", { album: album.name, subgroup: draft.subgroup.trim() })
              : t("editor.summary.album", { album: album.name })
            : "—",
    },
    {
      label: t("editor.output.longEdge"),
      value: Number.isNaN(longEdge) ? draft.longEdge : longEdge === null ? t("editor.summary.originalSize") : `${longEdge} px`,
    },
    { label: t("editor.output.quality"), value: `${recipe.output.quality}` },
    { label: "GPS", value: draft.removeGps ? t("editor.summary.gpsRemoved") : t("editor.summary.gpsKept") },
  ];
  if (draft.copyright.trim() !== "") rows.push({ label: t("editor.output.copyright"), value: draft.copyright.trim() });
  if (draft.author.trim() !== "") rows.push({ label: t("editor.output.author"), value: draft.author.trim() });
  if (draft.keywords.trim() !== "") rows.push({ label: t("editor.output.keywords"), value: draft.keywords.trim() });

  return (
    <div
      className="fixed inset-0 z-[90] flex items-center justify-center bg-black/50"
      onClick={onCancel}
      data-testid="export-dialog-overlay"
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={t("editor.exportTitle")}
        className="w-[420px] overflow-hidden rounded-xl border border-edge bg-surface shadow-2xl"
        onClick={(e) => e.stopPropagation()}
        data-testid="export-dialog"
      >
        <div className="flex items-center justify-between border-b border-edge px-4 py-3">
          <h2 className="text-sm font-semibold text-text-primary">
            {t("editor.exportTitle")}
            <span className="ml-2 font-mono text-[11px] font-normal text-text-muted" title={asset.name}>
              {asset.name}
            </span>
          </h2>
          <button
            type="button"
            onClick={onCancel}
            aria-label={t("common.close")}
            className="rounded px-1.5 text-lg leading-none text-text-muted transition-colors hover:text-text-primary"
            data-testid="export-dialog-close"
          >
            ×
          </button>
        </div>

        <div className="px-4 py-3" data-testid="export-dialog-summary">
          <dl className="space-y-1.5">
            {rows.map((row) => (
              <div key={row.label} className="flex items-baseline justify-between gap-3 text-xs">
                <dt className="shrink-0 text-text-muted">{row.label}</dt>
                <dd className="min-w-0 truncate text-right text-text-secondary" title={row.value}>
                  {row.value}
                </dd>
              </div>
            ))}
          </dl>
          <p className="mt-3 rounded-md bg-panel/60 px-2.5 py-2 text-[11px] leading-relaxed text-text-muted">
            {t("editor.exportNote")}
          </p>
          {validation.ok === false && (
            <ul className="mt-2 space-y-1" role="alert" data-testid="export-dialog-errors">
              {validation.errors.map((code) => (
                <li key={code} className="text-[11px] text-red-400">
                  {t(`editor.error.${code}`)}
                </li>
              ))}
            </ul>
          )}
        </div>

        <div className="flex items-center justify-end gap-2 border-t border-edge px-4 py-3">
          <button
            type="button"
            onClick={onCancel}
            className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
            data-testid="export-dialog-cancel"
          >
            {t("common.cancel")}
          </button>
          <button
            type="button"
            disabled={!validation.ok || running}
            onClick={() => {
              if (validation.ok) onConfirm(validation.options);
            }}
            className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="export-dialog-confirm"
          >
            {t("editor.exportConfirm")}
          </button>
        </div>
      </div>
    </div>
  );
}
