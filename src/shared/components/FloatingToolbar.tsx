import { useEffect, useState, type ReactNode } from "react";

/** 固定边缘热区：隐藏仅影响控件，不改变照片区域的尺寸。 */
export default function FloatingToolbar({ children, label, locked = false, className = "" }: {
  children: ReactNode;
  label: string;
  locked?: boolean;
  className?: string;
}) {
  const [visible, setVisible] = useState(true);
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const held = locked || hovered || focused;
  useEffect(() => {
    if (held) { setVisible(true); return; }
    if (!visible) return;
    const timer = window.setTimeout(() => setVisible(false), 3000);
    return () => window.clearTimeout(timer);
  }, [held, visible]);
  const shown = held || visible;
  return (
    <div
      role="toolbar"
      aria-label={label}
      tabIndex={0}
      className={`absolute right-3 top-0 z-30 min-h-14 min-w-24 rounded-b-2xl px-2 pt-3 ${className}`}
      onPointerEnter={() => { setHovered(true); setVisible(true); }}
      onPointerLeave={() => setHovered(false)}
      onFocusCapture={() => { setFocused(true); setVisible(true); }}
      onBlurCapture={(event) => {
        if (!(event.relatedTarget instanceof Node) || !event.currentTarget.contains(event.relatedTarget)) setFocused(false);
      }}
      onPointerDown={() => setVisible(true)}
      data-visible={shown}
    >
      <div className={`ui-floating-gradient pointer-events-none absolute inset-x-0 top-0 h-24 rounded-b-2xl transition-opacity duration-200 ${shown ? "opacity-100" : "opacity-0"}`} aria-hidden="true" />
      <div className={`relative flex items-center gap-2 transition-opacity duration-200 ${shown ? "opacity-100" : "pointer-events-none opacity-0"}`} inert={!shown}>
        {children}
      </div>
    </div>
  );
}
