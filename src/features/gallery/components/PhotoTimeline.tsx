import { useEffect, useMemo, useRef, useState } from "react";
import { motion } from "motion/react";
import { useTranslation } from "react-i18next";
import { getIntlLocale } from "@/i18n";
import { useMotionOn } from "@/lib/motion";
import type { AssetGroupDate } from "@/ipc/api";

export interface TimelineMonth extends AssetGroupDate { month: string }

export function timelineMonths(dates: AssetGroupDate[]): TimelineMonth[] {
  const months = new Map<string, TimelineMonth>();
  for (const entry of [...dates].sort((a, b) => (b.date ?? "").localeCompare(a.date ?? ""))) {
    if (!entry.date || !/^\d{4}-\d{2}-\d{2}$/.test(entry.date)) continue;
    const month = entry.date.slice(0, 7);
    const existing = months.get(month);
    if (existing) existing.count += entry.count;
    else months.set(month, { ...entry, month });
  }
  return [...months.values()];
}

export default function PhotoTimeline({ dates, currentDate, dateProgress = 0, busy, onJump, onDrag, onWheelScroll }: {
  dates: AssetGroupDate[];
  currentDate: string | null;
  dateProgress?: number;
  busy: boolean;
  onJump: (entry: AssetGroupDate, fraction?: number) => void;
  onDrag?: (entry: AssetGroupDate, fraction: number) => void;
  onWheelScroll?: (delta: number) => void;
}) {
  const { t } = useTranslation();
  const motionOn = useMotionOn();
  const months = useMemo(() => {
    const grouped = timelineMonths(dates);
    const known = grouped.length >= 2 ? grouped : dates.filter((entry) => entry.date && /^\d{4}-\d{2}-\d{2}$/.test(entry.date))
      .sort((a, b) => b.date!.localeCompare(a.date!))
      .map((entry) => ({ ...entry, month: entry.date! }));
    const unknown = dates.filter((entry) => !entry.date || entry.date === "unknown");
    if (unknown.length) known.push({ date: null, month: "unknown", count: unknown.reduce((sum, entry) => sum + entry.count, 0), coverAssetId: unknown[0].coverAssetId });
    return known;
  }, [dates]);
  const basePositions = useMemo(() => {
    let sum = 0;
    const positions = months.map((entry) => {
      const value = sum;
      sum += 2 + Math.log2(entry.count + 1);
      return value;
    });
    return positions.map((value) => sum ? value / sum : 0);
  }, [months]);
  const track = useRef<HTMLDivElement>(null);
  const [focus, setFocus] = useState<number | null>(null);
  const [hovered, setHovered] = useState<number | null>(null);
  const [height, setHeight] = useState(600);
  const [dragRatio, setDragRatio] = useState<number | null>(null);
  const dragging = useRef(false);
  const dragStart = useRef(0);
  const moved = useRef(false);
  const suppressClick = useRef(false);
  const selectedFraction = useRef(0);
  const selectedEntry = useRef<AssetGroupDate | null>(null);
  const pressedButton = useRef<number | null>(null);
  const dragHandlers = useRef<{ move: (y: number) => void; end: () => void } | null>(null);
  useEffect(() => {
    const move = (event: MouseEvent) => { if (dragging.current) dragHandlers.current?.move(event.clientY); };
    const end = () => { if (dragging.current) dragHandlers.current?.end(); };
    const cancel = () => { dragging.current = false; setDragRatio(null); setFocus(null); setHovered(null); };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", end);
    window.addEventListener("pointercancel", cancel);
    // Mouse fallback also covers embedded webviews without reliable pointer capture.
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", end);
    window.addEventListener("blur", cancel);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", end);
      window.removeEventListener("pointercancel", cancel);
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", end);
      window.removeEventListener("blur", cancel);
    };
  }, []);
  useEffect(() => {
    const element = track.current;
    if (!element) return;
    const update = () => setHeight(element.clientHeight || 600);
    update();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(update);
    observer.observe(element);
    return () => observer.disconnect();
  }, [months.length]);

  const active = months.findIndex((entry) => entry.month === "unknown" ? currentDate === null : entry.month === currentDate?.slice(0, entry.month.length));
  const highlighted = hovered ?? active;
  const preview = months[highlighted];
  // Hover changes mark width and color only; dates never move under the pointer.
  const positions = basePositions;
  const spacing = Math.max(1, Math.ceil(months.length * 7 / height));
  const yearLabels = new Set<number>();
  let lastYearY = -Infinity;
  months.forEach((entry, index) => {
    const y = basePositions[index] * height;
    if ((index === 0 || months[index - 1].month.slice(0, 4) !== entry.month.slice(0, 4)) && y - lastYearY >= 16) {
      yearLabels.add(index);
      lastYearY = y;
    }
  });
  const yearY = new Map<number, number>();
  let previousYearY = -14;
  for (const index of yearLabels) {
    const y = Math.max(positions[index] * height, previousYearY + 14);
    yearY.set(index, y);
    previousYearY = y;
  }
  let nextYearY = height + 14;
  for (const index of [...yearLabels].reverse()) {
    const y = Math.min(yearY.get(index)!, nextYearY - 14);
    yearY.set(index, y);
    nextYearY = y;
  }
  const label = (entry: TimelineMonth) => entry.month === "unknown" ? t("gallery.unknownDate") : new Intl.DateTimeFormat(getIntlLocale(), {
    year: "numeric", month: "long", ...(entry.month.length === 10 ? { day: "numeric" as const } : {}),
  }).format(new Date(Number(entry.month.slice(0, 4)), Number(entry.month.slice(5, 7)) - 1, Number(entry.month.slice(8, 10)) || 1));
  const transition = motionOn ? { type: "spring" as const, stiffness: 450, damping: 38, mass: 0.55 } : { duration: 0 };
  const ratioAt = (y: number) => {
    const rect = track.current?.getBoundingClientRect();
    return rect && rect.height > 0 ? Math.max(0, Math.min(1, (y - rect.top) / rect.height)) : null;
  };
  const nearest = (ratio: number, points: number[]) => points.reduce((best, value, index) =>
    Math.abs(value - ratio) < Math.abs(points[best] - ratio) ? index : best, 0);
  const pickPosition = (y: number) => {
    if (!Number.isFinite(y) || !months.length) return;
    const ratio = ratioAt(y);
    if (ratio === null) return;
    const base = ratio;
    let index = basePositions.length - 1;
    for (let i = 0; i < basePositions.length - 1; i++) {
      if (base < basePositions[i + 1]) { index = i; break; }
    }
    selectedFraction.current = Math.max(0, Math.min(1, (base - basePositions[index]) / Math.max(0.001, (basePositions[index + 1] ?? 1) - basePositions[index])));
    selectedEntry.current = months[index];
    if (months[index].month !== "unknown" && months[index].month.length === 7) {
      const days = dates.filter((entry) => entry.date?.startsWith(months[index].month)).sort((a, b) => b.date!.localeCompare(a.date!));
      let remaining = selectedFraction.current * months[index].count;
      for (const day of days) {
        if (remaining <= day.count || day === days[days.length - 1]) {
          selectedEntry.current = day;
          selectedFraction.current = Math.min(1, remaining / Math.max(1, day.count));
          break;
        }
        remaining -= day.count;
      }
    }
    setHovered(index);
    setDragRatio(ratio);
  };
  const startDrag = (y: number, target: HTMLElement) => {
    if (dragging.current || busy || target.closest("[data-timeline-label]")) return false;
    dragging.current = true;
    moved.current = false;
    suppressClick.current = false;
    const buttonIndex = target.closest<HTMLElement>("[data-timeline-index]")?.dataset.timelineIndex;
    pressedButton.current = buttonIndex === undefined ? null : Number(buttonIndex);
    dragStart.current = y;
    pickPosition(y);
    return true;
  };
  dragHandlers.current = {
    move: (y) => {
      if (Math.abs(y - dragStart.current) > 3) moved.current = true;
      pickPosition(y);
      if (moved.current && selectedEntry.current) onDrag?.(selectedEntry.current, selectedFraction.current);
    },
    end: () => {
      dragging.current = false;
      suppressClick.current = true;
      if (!moved.current && pressedButton.current !== null) onJump(months[pressedButton.current]);
      else if (selectedEntry.current) onJump(selectedEntry.current, selectedFraction.current);
      setDragRatio(null);
      setTimeout(() => { suppressClick.current = false; }, 0);
    },
  };
  let withinMonth = dateProgress;
  if (active >= 0 && months[active].month !== "unknown" && months[active].month.length === 7) {
    const inMonth = dates.filter((entry) => entry.date?.startsWith(months[active].month)).sort((a, b) => b.date!.localeCompare(a.date!));
    let before = 0;
    for (const entry of inMonth) {
      if (entry.date === currentDate) { withinMonth = (before + entry.count * dateProgress) / Math.max(1, months[active].count); break; }
      before += entry.count;
    }
  }
  const currentPosition = active < 0 ? (currentDate === null ? 1 : 0) : basePositions[active] + withinMonth * ((basePositions[active + 1] ?? 1) - basePositions[active]);
  const thumbPosition = dragRatio ?? currentPosition;

  if (months.length === 0) return null;

  return (
    <nav aria-label={t("timeline.label")} className="absolute inset-y-0 right-0 z-20 flex w-5 flex-col py-4" data-testid="photo-timeline"
      onWheel={(event) => { if (onWheelScroll) { event.preventDefault(); onWheelScroll(event.deltaY); } }}>
      <div ref={track} className="relative min-h-0 flex-1 cursor-grab touch-none select-none active:cursor-grabbing"
        onPointerDown={(event) => {
          if (event.button !== 0 || !startDrag(event.clientY, event.target as HTMLElement)) return;
          event.preventDefault();
          try { event.currentTarget.setPointerCapture?.(event.pointerId); } catch { /* Global listeners continue the drag. */ }
        }}
        onMouseDown={(event) => { if (event.button === 0 && startDrag(event.clientY, event.target as HTMLElement)) event.preventDefault(); }}
        onClickCapture={(event) => { if (suppressClick.current) { event.preventDefault(); event.stopPropagation(); suppressClick.current = false; } }}
        onPointerMove={(event) => {
          if (!dragging.current) {
            const ratio = ratioAt(event.clientY);
            if (ratio !== null) { setFocus(ratio); setHovered(nearest(ratio, basePositions)); }
          }
        }}
        onPointerUp={(event) => {
          if (!dragging.current) return;
          try { event.currentTarget.releasePointerCapture?.(event.pointerId); } catch { /* Capture may have been released by the webview. */ }
          dragHandlers.current?.end();
        }}
        onPointerLeave={() => { if (!dragging.current) { setFocus(null); setHovered(null); } }}
        onPointerCancel={() => { dragging.current = false; setDragRatio(null); setFocus(null); setHovered(null); }}>
        <motion.span className="pointer-events-none absolute bottom-0 right-[7px] top-0 w-px rounded-full bg-text-muted/25" animate={{ opacity: focus === null ? 0.45 : 1 }} />
        <motion.span aria-hidden="true" className="absolute right-[5px] h-6 w-[5px] cursor-grab rounded-full bg-accent/80 shadow-sm active:cursor-grabbing"
          animate={{ top: `${thumbPosition * Math.max(0, height - 24)}px`, width: focus === null ? 3 : 5, opacity: busy ? 0.4 : 0.9 }} transition={dragRatio === null ? transition : { duration: 0 }} data-testid="timeline-scroll-thumb" />
        {months.map((entry, index) => {
          const firstOfYear = yearLabels.has(index);
          const proximity = focus === null ? 0 : Math.max(0, 1 - Math.abs(basePositions[index] - focus) / 0.12);
          if (!firstOfYear && index % spacing !== 0 && proximity < 0.2 && index !== months.length - 1 && index !== highlighted) return null;
          return <motion.button key={entry.month} type="button" disabled={busy} data-timeline-index={index}
            aria-label={t("timeline.jumpTo", { date: label(entry) })} aria-current={index === active ? "date" : undefined}
            title={`${label(entry)} · ${t("gallery.groupCount", { count: entry.count })}`}
            onClick={() => { if (suppressClick.current) { suppressClick.current = false; return; } onJump(entry); }}
            onMouseEnter={() => { if (!dragging.current) setHovered(index); }}
            onFocus={() => { if (!dragging.current) { setHovered(index); setFocus(basePositions[index]); } }}
            onBlur={() => { setHovered(null); setFocus(null); }}
            className={`absolute right-0 flex ${firstOfYear ? "h-4" : "h-2"} w-full -translate-y-1/2 items-center justify-end rounded text-[10px] tabular-nums focus-visible:outline focus-visible:outline-accent disabled:cursor-wait ${index === highlighted ? "text-accent" : "text-text-muted"}`}
            initial={false} animate={{ top: `${positions[index] * 100}%` }} transition={transition}>
            {firstOfYear && <motion.span data-timeline-label className="absolute right-[15px] whitespace-nowrap rounded bg-bg/75 px-1 py-0.5 font-medium backdrop-blur-sm" animate={{ y: yearY.get(index)! - positions[index] * height }} transition={transition}>{entry.month === "unknown" ? t("gallery.unknownDate") : entry.month.slice(0, 4)}</motion.span>}
            <motion.span className={`mr-[6px] shrink-0 rounded-full ${index === highlighted ? "bg-accent" : "bg-text-muted/55"}`}
              animate={{ width: 3 + proximity * 9, height: 3, opacity: focus === null ? 0.6 : 0.35 + proximity * 0.65 }} transition={transition} />
          </motion.button>;
        })}
      </div>
      {((hovered !== null && preview) || busy) && <div className="pointer-events-none absolute right-12 z-20 whitespace-nowrap rounded-lg border border-edge bg-surface/95 px-3 py-2 text-xs text-text-primary shadow-xl backdrop-blur-sm" style={{ top: `${12 + (focus ?? thumbPosition) * Math.max(0, height - 44)}px` }} role="status">
        {busy ? t("timeline.loading") : preview && <><span className="font-semibold">{label(preview)}</span><span className="ml-2 text-text-muted">{t("gallery.groupCount", { count: preview.count })}</span></>}
      </div>}
    </nav>
  );
}
