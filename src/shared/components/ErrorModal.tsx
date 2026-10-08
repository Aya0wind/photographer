import { useEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";

/** An operation failure needs an explicit acknowledgement before the user retries. */
export default function ErrorModal({
  message,
  onClose,
}: {
  message: string | null;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const button = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!message) return;
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    button.current?.focus();
    const keydown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
      if (event.key === "Tab") {
        event.preventDefault();
        button.current?.focus();
      }
    };
    window.addEventListener("keydown", keydown);
    return () => {
      window.removeEventListener("keydown", keydown);
      previousFocus?.focus();
    };
  }, [message, onClose]);
  if (!message) return null;
  return createPortal(
    <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/65 px-4" role="alertdialog" aria-modal="true" aria-labelledby="operation-error-title" aria-describedby="operation-error-message" data-testid="operation-error-dialog" onClick={(event) => event.stopPropagation()}>
      <div className="ui-glass w-full max-w-md rounded-2xl border border-edge p-5 shadow-2xl">
        <h2 id="operation-error-title" className="text-base font-semibold text-text-primary">{t("common.operationFailed")}</h2>
        <p id="operation-error-message" className="mt-3 whitespace-pre-wrap break-words text-sm leading-relaxed text-text-secondary">{message}</p>
        <div className="mt-5 flex justify-end">
          <button ref={button} type="button" onClick={onClose} className="rounded-md bg-accent px-5 py-2 text-sm font-medium text-black hover:brightness-110">{t("common.confirm")}</button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
