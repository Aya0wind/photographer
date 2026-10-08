import { useEffect, useRef, useState, type ReactNode } from "react";

/** 次要操作统一支持点击外部关闭、Esc 关闭并把键盘焦点还给入口。 */
export default function ActionPopover({ label, children, onOpenChange, closeOnAction = false, panelClassName = "" }: {
  label: string;
  children: ReactNode;
  onOpenChange?: (open: boolean) => void;
  closeOnAction?: boolean;
  panelClassName?: string;
}) {
  const ref = useRef<HTMLDetailsElement>(null);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    if (!open) return;
    const close = () => { if (ref.current) ref.current.open = false; };
    const outside = (event: PointerEvent) => { if (!ref.current?.contains(event.target as Node)) close(); };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      close();
      ref.current?.querySelector("summary")?.focus();
    };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", escape);
    return () => { document.removeEventListener("pointerdown", outside); document.removeEventListener("keydown", escape); };
  }, [open]);
  return (
    <details ref={ref} className="relative" onToggle={(event) => { setOpen(event.currentTarget.open); onOpenChange?.(event.currentTarget.open); }}>
      <summary className="ui-icon-button list-none [&::-webkit-details-marker]:hidden" aria-label={label} title={label}>···</summary>
      <div className={`ui-glass ui-popover absolute right-0 top-12 z-40 flex w-64 flex-col gap-3 rounded-2xl border p-4 ${panelClassName}`}
        onClick={(event) => { if (closeOnAction && (event.target as Element).closest("button") && ref.current) ref.current.open = false; }}>
        {children}
      </div>
    </details>
  );
}
