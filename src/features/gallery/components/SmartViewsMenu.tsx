import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import type { SmartViewDto } from "@/ipc/api";

/**
 * 已存视图下拉（B1，图库工具条）：点击视图=应用（上层反解 filtersJson 回面板
 * 输入）；每项 × 删除需行内确认（确认/取消两键）；挂载清单由上层拉取传入。
 */

export default function SmartViewsMenu({
  views,
  onApply,
  onDelete,
}: {
  views: SmartViewDto[];
  /** 应用视图（上层反解 filtersJson → 面板输入并套用） */
  onApply: (view: SmartViewDto) => void;
  /** 删除视图（上层调 smart_view_delete 并刷新清单；仅确认后回调） */
  onDelete: (view: SmartViewDto) => void;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  /** 行内删除确认中的视图 id（一次只确认一项） */
  const [confirmId, setConfirmId] = useState<number | null>(null);
  const rootRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) return;
    const closeOutside = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    window.addEventListener("pointerdown", closeOutside);
    window.addEventListener("keydown", closeOnEscape);
    return () => {
      window.removeEventListener("pointerdown", closeOutside);
      window.removeEventListener("keydown", closeOnEscape);
    };
  }, [open]);

  return (
    <div ref={rootRef} className="relative shrink-0">
      <button
        type="button"
        onClick={() => {
          setOpen((v) => !v);
          setConfirmId(null);
        }}
        aria-expanded={open}
        className={`flex items-center gap-1.5 rounded-md border px-2.5 py-1 text-[11px] transition-colors ${
          open
            ? "border-accent text-accent"
            : "border-edge text-text-secondary hover:border-text-muted hover:text-text-primary"
        }`}
        data-testid="smart-views-toggle"
      >
        <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="M2 4h12M4 8h8M6 12h4" />
        </svg>
        {t("smartview.title")}
        {views.length > 0 && (
          <span className="rounded-full bg-accent/15 px-1.5 font-mono text-[10px] leading-4 tabular-nums text-accent">
            {views.length}
          </span>
        )}
        <svg viewBox="0 0 16 16" width="9" height="9" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" className={`transition-transform ${open ? "rotate-180" : ""}`} aria-hidden="true">
          <path d="M3.5 6l4.5 4.5L12.5 6" />
        </svg>
      </button>
      {open && (
        <div
          className="sp-scroll absolute right-0 top-8 z-30 max-h-72 w-56 overflow-y-auto rounded-xl border border-edge bg-surface p-1.5 shadow-2xl shadow-black/40"
          data-testid="smart-views-menu"
        >
          {views.length === 0 ? (
            <p className="px-2 py-2 text-[11px] leading-relaxed text-text-muted">{t("smartview.empty")}</p>
          ) : (
            views.map((view) =>
              confirmId === view.id ? (
                <div
                  key={view.id}
                  className="flex items-center gap-1.5 rounded px-2 py-1.5"
                  data-testid="smart-view-delete-row"
                >
                  <span className="min-w-0 flex-1 truncate text-[11px] text-red-400">
                    {t("smartview.deleteConfirmTitle")}
                  </span>
                  <button
                    type="button"
                    onClick={() => {
                      setConfirmId(null);
                      onDelete(view);
                    }}
                    className="shrink-0 rounded border border-red-400/60 px-1.5 py-0.5 text-[10px] text-red-400 transition-colors hover:bg-red-400/10"
                    data-testid="smart-view-delete-confirm"
                  >
                    {t("smartview.deleteConfirmYes")}
                  </button>
                  <button
                    type="button"
                    onClick={() => setConfirmId(null)}
                    className="shrink-0 rounded border border-edge px-1.5 py-0.5 text-[10px] text-text-muted transition-colors hover:bg-panel hover:text-text-primary"
                    data-testid="smart-view-delete-cancel"
                  >
                    {t("common.cancel")}
                  </button>
                </div>
              ) : (
                <div
                  key={view.id}
                  className="flex items-center gap-1 rounded px-2 py-1.5 transition-colors hover:bg-panel/40"
                  data-testid="smart-view-option"
                  data-view-id={view.id}
                >
                  <button
                    type="button"
                    onClick={() => {
                      setOpen(false);
                      onApply(view);
                    }}
                    className="min-w-0 flex-1 truncate text-left text-[11px] text-text-secondary hover:text-accent"
                    title={view.name}
                    data-testid="smart-view-apply"
                  >
                    {view.name}
                  </button>
                  <button
                    type="button"
                    onClick={() => setConfirmId(view.id)}
                    aria-label={`${t("smartview.delete")} ${view.name}`}
                    title={t("smartview.delete")}
                    className="flex h-4 w-4 shrink-0 items-center justify-center rounded-full text-text-muted transition-colors hover:bg-red-400/10 hover:text-red-400"
                    data-testid="smart-view-delete"
                  >
                    ×
                  </button>
                </div>
              ),
            )
          )}
        </div>
      )}
    </div>
  );
}
