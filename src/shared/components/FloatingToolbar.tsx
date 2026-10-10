import type { ReactNode } from "react";

/** Browsing actions stay visible; a photo viewer temporarily makes them inactive. */
export default function FloatingToolbar({ children, label, inactive = false, className = "" }: {
  children: ReactNode;
  label: string;
  locked?: boolean;
  inactive?: boolean;
  className?: string;
}) {
  return <div role="toolbar" aria-label={label} inert={inactive} data-visible={!inactive}
    className={`absolute right-3 top-0 z-30 min-h-14 min-w-24 rounded-b-2xl px-2 pt-3 ${className}`}>
    <div className={`ui-floating-gradient pointer-events-none absolute inset-x-0 top-0 h-[67px] rounded-b-2xl transition-opacity duration-200 ${inactive ? "opacity-0" : "opacity-100"}`} aria-hidden="true" />
    <div className={`relative flex items-center gap-2 transition-opacity duration-200 ${inactive ? "pointer-events-none opacity-0" : "opacity-100"}`}>
      {children}
    </div>
  </div>;
}
