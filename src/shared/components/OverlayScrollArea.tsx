import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";

/** Thin overlay scrollbar; pointer capture and window listeners keep dragging outside the rail. */
export default function OverlayScrollArea({ children, className = "", contentClassName = "", testId }: {
  children: ReactNode;
  className?: string;
  contentClassName?: string;
  testId?: string;
}) {
  const { t } = useTranslation();
  const id = useId();
  const scroll = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLDivElement>(null);
  const track = useRef<HTMLDivElement>(null);
  const [metrics, setMetrics] = useState({ height: 0, thumb: 24, progress: 0, maximum: 0 });
  const [dragging, setDragging] = useState(false);
  const drag = useRef<{ offset: number } | null>(null);
  const handlers = useRef<{ move: (y: number) => void; end: () => void } | null>(null);
  const update = useCallback(() => {
    const element = scroll.current;
    if (!element) return;
    const height = track.current?.clientHeight || element.clientHeight;
    const maximum = Math.max(0, element.scrollHeight - element.clientHeight);
    const thumb = Math.min(height, Math.max(24, height * element.clientHeight / Math.max(1, element.scrollHeight)));
    const progress = maximum ? element.scrollTop / maximum : 0;
    setMetrics((previous) => previous.height === height && previous.thumb === thumb && previous.progress === progress && previous.maximum === maximum
      ? previous : { height, thumb, progress, maximum });
  }, []);
  useLayoutEffect(update, [children, update]);
  useEffect(() => {
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(update);
    if (scroll.current) observer.observe(scroll.current);
    if (content.current) observer.observe(content.current);
    if (track.current) observer.observe(track.current);
    return () => observer.disconnect();
  }, [update]);
  useEffect(() => {
    const move = (event: MouseEvent) => { if (drag.current) handlers.current?.move(event.clientY); };
    const end = () => handlers.current?.end();
    window.addEventListener("pointermove", move);
    window.addEventListener("mousemove", move);
    window.addEventListener("pointerup", end);
    window.addEventListener("mouseup", end);
    window.addEventListener("pointercancel", end);
    window.addEventListener("blur", end);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("mousemove", move);
      window.removeEventListener("pointerup", end);
      window.removeEventListener("mouseup", end);
      window.removeEventListener("pointercancel", end);
      window.removeEventListener("blur", end);
    };
  }, []);
  const setScroll = (value: number) => {
    if (!scroll.current) return;
    scroll.current.scrollTop = Math.max(0, Math.min(metrics.maximum, value));
    update();
  };
  handlers.current = {
    move: (y) => {
      const rect = track.current?.getBoundingClientRect();
      if (!rect || !drag.current) return;
      const ratio = (y - rect.top - drag.current.offset) / Math.max(1, metrics.height - metrics.thumb);
      setScroll(ratio * metrics.maximum);
    },
    end: () => { if (drag.current) { drag.current = null; setDragging(false); } },
  };
  const start = (y: number, onThumb: boolean) => {
    if (drag.current || metrics.maximum <= 0) return false;
    const rect = track.current?.getBoundingClientRect();
    if (!rect) return false;
    drag.current = { offset: onThumb ? y - rect.top - metrics.progress * (metrics.height - metrics.thumb) : metrics.thumb / 2 };
    setDragging(true);
    handlers.current?.move(y);
    return true;
  };
  return <div className={`relative h-full min-h-0 ${className}`}>
    <div id={id} ref={scroll} onScroll={update} className="h-full overflow-y-auto [scrollbar-gutter:auto] [scrollbar-width:none] [&::-webkit-scrollbar]:hidden" data-testid={testId}>
      <div ref={content} className={contentClassName}>{children}</div>
    </div>
    <div ref={track} className={`group absolute bottom-4 right-0 top-4 w-5 touch-none select-none ${dragging ? "cursor-grabbing" : "cursor-grab"} ${metrics.maximum ? "" : "pointer-events-none opacity-0"}`}
      role="scrollbar" tabIndex={metrics.maximum ? 0 : -1} aria-label={t("common.scrollbar")} aria-controls={id} aria-orientation="vertical" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(metrics.progress * 100)}
      onPointerDown={(event) => {
        if (event.button !== 0 || !start(event.clientY, Boolean((event.target as HTMLElement).closest("[data-scroll-thumb]")))) return;
        event.preventDefault();
        try { event.currentTarget.setPointerCapture?.(event.pointerId); } catch { /* Window listeners continue the drag. */ }
      }}
      onMouseDown={(event) => { if (event.button === 0 && start(event.clientY, Boolean((event.target as HTMLElement).closest("[data-scroll-thumb]")))) event.preventDefault(); }}
      onWheel={(event) => { event.preventDefault(); setScroll((scroll.current?.scrollTop ?? 0) + event.deltaY); }}
      onKeyDown={(event) => {
        const top = scroll.current?.scrollTop ?? 0;
        const page = scroll.current?.clientHeight ?? 200;
        const value = { ArrowUp: top - 80, ArrowDown: top + 80, PageUp: top - page, PageDown: top + page, Home: 0, End: metrics.maximum }[event.key];
        if (value === undefined) return;
        event.preventDefault();
        setScroll(value);
      }}>
      <span className="pointer-events-none absolute bottom-0 right-[7px] top-0 w-px rounded-full bg-text-muted/15" />
      <span data-scroll-thumb className={`absolute right-[5px] rounded-full bg-accent/80 shadow-sm transition-[width,opacity] duration-150 ${dragging ? "w-[5px] opacity-100" : "w-[3px] opacity-80 group-hover:w-[5px] group-hover:opacity-100"}`}
        style={{ height: metrics.thumb, top: metrics.progress * (metrics.height - metrics.thumb) }} />
    </div>
  </div>;
}
