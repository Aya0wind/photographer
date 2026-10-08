import { useId, useLayoutEffect, useRef, type ReactNode } from "react";
import { useTranslation } from "react-i18next";

/** 原生模态抽屉负责焦点限制与恢复，确认前仍可返回修改照片选择。 */
export default function ImportReviewDrawer({ children, footer, busy, onBack }: {
  children: ReactNode;
  footer: ReactNode;
  busy: boolean;
  onBack: () => void;
}) {
  const { t } = useTranslation();
  const titleId = useId();
  const ref = useRef<HTMLDialogElement>(null);
  useLayoutEffect(() => {
    const dialog = ref.current;
    if (!dialog) return;
    dialog.showModal();
    return () => dialog.close();
  }, []);
  return (
    <dialog ref={ref} aria-labelledby={titleId}
      className="ui-glass fixed bottom-3 left-auto right-3 top-3 m-0 h-[calc(100%_-_24px)] max-h-none w-[min(500px,calc(100%_-_24px))] max-w-none overflow-hidden rounded-2xl border border-edge p-0 text-text-primary shadow-2xl backdrop:bg-black/35 backdrop:backdrop-blur-[2px]"
      onCancel={(event) => { event.preventDefault(); if (!busy) onBack(); }}
      onKeyDown={(event) => { if (event.key === "Escape") event.stopPropagation(); }}
      onClick={(event) => {
        if (busy || event.target !== event.currentTarget) return;
        const rect = event.currentTarget.getBoundingClientRect();
        if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) onBack();
      }}
      data-testid="wizard-review-drawer">
      <div className="flex h-full min-h-0 flex-col">
        <header className="flex shrink-0 items-center gap-3 border-b border-edge px-6 py-5">
          <div className="min-w-0 flex-1"><h2 id={titleId} className="text-lg font-semibold">{t("wizard.flow.reviewTitle")}</h2><p className="mt-1 text-xs text-text-muted">{t("wizard.flow.reviewDesc")}</p></div>
          <button type="button" className="ui-icon-button" onClick={onBack} disabled={busy} aria-label={t("ui.close")}>×</button>
        </header>
        <div className="sp-scroll min-h-0 flex-1 overflow-y-auto px-6 py-5">{children}</div>
        <footer className="shrink-0 border-t border-edge px-6 py-5">{footer}</footer>
      </div>
    </dialog>
  );
}
