import { useEffect, useLayoutEffect, useRef, useState, type PointerEvent } from "react";
import { useTranslation } from "react-i18next";
import CurveCoordinates from "./CurveCoordinates";
import type { AdvancedAdjustments } from "@/ipc/api";
import { addCurvePoint, clampTone, curveSamples, IDENTITY, moveCurvePoint, type CurveChannel, type CurvePicker, type CurvePoint, type HslColorRange,type HslAxis } from "../lib/curves";

export interface CurveSampling {
  sampleSize?:1|3|5|11;
  setSampleSize?:(size:1|3|5|11)=>void;
  setTargetCurveChannel?:(channel:CurveChannel)=>void;
  targetHslAxis?:HslAxis;
  setTargetHslAxis?:(axis:HslAxis)=>void;
  available?:boolean;
  picker: CurvePicker | null;
  busy?:boolean;
  hslRange?:HslColorRange | null;
  setPicker: (picker: CurvePicker | null) => void;
  sample: [number, number, number] | null;
  histogram: number[][];
}
const COLORS = { rgb: "#b7bcc6", red: "#f87171", green: "#4ade80", blue: "#60a5fa", luminance: "#facc15" };
const PRESETS: Record<string, CurvePoint[]> = {
  linear: [], contrast: [[0, 0], [64, 45], [128, 128], [192, 210], [255, 255]],
  faded: [[0, 20], [64, 70], [128, 135], [192, 198], [255, 245]],
};

export default function CurveEditor({ adjustments, onChange, begin, end, sampling, onReset, onModalChange }: {
  adjustments: AdvancedAdjustments; onChange: (patch: Partial<AdvancedAdjustments>, record?: boolean) => void;
  onReset:()=>void;
  onModalChange?:(value:boolean)=>void;
  begin: () => void; end: () => void; sampling: CurveSampling;
}) {
  const [expanded,setExpanded]=useState(false);
  useEffect(()=>{onModalChange?.(expanded);return()=>onModalChange?.(false);},[expanded,onModalChange]);
  const { t } = useTranslation();
  const [channel, setChannel] = useState<CurveChannel>("rgb");
  const [selected, setSelected] = useState<number | null>(null);
  const histogram = sampling.histogram;
  const drag = useRef<{ id: number; index: number; points: CurvePoint[]; x: number; y: number } | null>(null);
  const raw = channel === "rgb" ? adjustments.curves : adjustments.channelCurves?.[channel];
  // Reconcile before the next pointer event can advance the gesture ref.
  // Parents may copy equal coordinates without cancelling the current drag.
  useLayoutEffect(()=>{
    const current=drag.current;
    if(current&&!(raw?.length===current.points.length&&raw.every(([x,y],i)=>x===current.points[i][0]&&y===current.points[i][1])))drag.current=null;
  },[raw]);
  const points = raw?.length ? raw : IDENTITY;
  const samples = curveSamples(points);
  const apply = (curve: CurvePoint[], record = true) => onChange(channel === "rgb" ? { curves: curve }
    : { channelCurves: { ...adjustments.channelCurves, [channel]: curve } }, record);
  const sampleRef = useRef(sampling.sample);
  useEffect(()=>{if(sampling.picker==="curveTarget"&&sampling.sample){const rgb=sampling.sample;const x=channel==="red"?rgb[0]:channel==="green"?rgb[1]:channel==="blue"?rgb[2]:rgb[0]*.2126+rgb[1]*.7152+rgb[2]*.0722;const index=points.findIndex(([p])=>Math.abs(p-x)<1);setSelected(index<0?null:index);}},[sampling.picker,sampling.sample,points,channel]);
  useEffect(() => {
    if (!sampling.sample || sampleRef.current === sampling.sample || !sampling.picker) return;
    sampleRef.current = sampling.sample;
    const rgb = sampling.sample;
    const luma = rgb[0] * .2126 + rgb[1] * .7152 + rgb[2] * .0722;
    if (sampling.picker === "point") {
      const x = clampTone(channel === "red" ? rgb[0] : channel === "green" ? rgb[1] : channel === "blue" ? rgb[2] : luma);
      const added = addCurvePoint(points, x, samples[Math.round(x)]);
      apply(added.points); setSelected(added.index >= 0 ? added.index : null);
    }
  }, [sampling, channel, adjustments, points, samples]);
  function position(event: PointerEvent<SVGSVGElement>): [number, number] {
    const rect = event.currentTarget.getBoundingClientRect();
    return [clampTone((event.clientX - rect.left) / rect.width * 255), clampTone(255 - (event.clientY - rect.top) / rect.height * 255)];
  }
  function finish() { if (drag.current) { drag.current = null; end(); } }
  function removePoint() {
    if (selected === null || selected >= points.length || selected === 0 || selected === points.length - 1) return;
    apply(points.filter((_, i) => i !== selected)); setSelected(null);
  }
  const bins = channel === "rgb" && histogram.length ? histogram[0].map((v, i) => v + histogram[1][i] + histogram[2][i]) : histogram[channel === "red" ? 0 : channel === "green" ? 1 : channel === "blue" ? 2 : 3] ?? [];
  const peak = Math.max(1, ...bins);
  return <details open className={expanded?"fixed inset-y-12 left-1/2 z-[75] w-[min(90vw,40rem)] -translate-x-1/2 space-y-3 overflow-y-auto rounded-2xl border border-edge bg-panel p-5 shadow-2xl":"space-y-3 border-t border-edge pt-4"}
    onKeyDownCapture={event=>{if(expanded&&event.key==="Escape"){event.preventDefault();event.stopPropagation();finish();setExpanded(false);}}}>
    <summary className="flex cursor-pointer items-center justify-between"><h3 className="text-xs font-semibold">{t("advancedEditor.curves")}</h3>
      <button type="button" aria-label={t("advancedEditor.resetGroup",{group:t("advancedEditor.curves")})}
        disabled={!adjustments.curves.length && !Object.values(adjustments.channelCurves??{}).some(points=>!!points?.length)}
        className="rounded-md px-2 py-1 text-[11px] text-accent disabled:opacity-30" onClick={event=>{event.preventDefault();event.stopPropagation();finish();setSelected(null);onReset();}}>{t("editor.reset")}</button>
      <button type="button" className="rounded border border-edge px-2 py-1 text-[11px]" aria-pressed={expanded} onClick={event=>{event.preventDefault();event.stopPropagation();finish();setExpanded(value=>!value);}}>{t(expanded?"advancedEditor.curve.compact":"advancedEditor.curve.expand")}</button>
      <select value={channel} aria-label={t("advancedEditor.curveChannel")} className="rounded-lg border border-edge bg-bg px-2 py-1 text-xs" onChange={(e) => { finish(); setChannel(e.target.value as CurveChannel);sampling.setTargetCurveChannel?.(e.target.value as CurveChannel); setSelected(null); }}>
        {Object.keys(COLORS).map((key) => <option key={key} value={key}>{t(`advancedEditor.channels.${key}`)}</option>)}
      </select></summary>
    <svg viewBox="0 0 255 255" preserveAspectRatio="none" tabIndex={0} role="application" aria-label={t("advancedEditor.curves")}
      className="aspect-square w-full touch-none rounded-xl border border-edge bg-bg outline-none focus:border-accent"
      onPointerDown={(event) => {
        if (event.button !== 0) return;
        event.preventDefault(); event.currentTarget.focus();
        const [x, y] = position(event);
        const nearest = points.findIndex(([px, py]) => Math.hypot(px - x, py - y) < 12);
        begin();
        const added = nearest >= 0 ? { points, index: nearest } : addCurvePoint(points, x, y);
        if (added.index < 0) { end(); return; }
        setSelected(added.index); apply(added.points, false);
        drag.current = { id: event.pointerId, index: added.index, points: added.points, x: event.clientX, y: event.clientY };
        event.currentTarget.setPointerCapture(event.pointerId);
      }}
      onPointerMove={(event) => {
        const gesture = drag.current;
        if (!gesture || gesture.id !== event.pointerId) return;
        const rect = event.currentTarget.getBoundingClientRect();
        const factor = event.altKey ? .1 : 1;
        const [x, y] = gesture.points[gesture.index];
        gesture.points = moveCurvePoint(gesture.points, gesture.index, x + (event.clientX - gesture.x) / rect.width * 255 * factor,
          y - (event.clientY - gesture.y) / rect.height * 255 * factor);
        gesture.x = event.clientX; gesture.y = event.clientY;
        apply(gesture.points, false);
      }} onPointerUp={finish} onPointerCancel={finish} onLostPointerCapture={finish} onBlur={finish}
      onDoubleClick={(event) => { event.preventDefault(); removePoint(); }}
      onKeyDown={(event) => {
        if (event.key === "Delete" || event.key === "Backspace") { event.preventDefault(); event.stopPropagation(); removePoint(); }
        if (selected !== null && selected < points.length && ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(event.key)) {
          event.preventDefault(); event.stopPropagation(); const step = event.altKey ? .1 : event.shiftKey ? 10 : 1;
          const [x, y] = points[selected]; apply(moveCurvePoint(points, selected, x + (event.key === "ArrowRight" ? step : event.key === "ArrowLeft" ? -step : 0), y + (event.key === "ArrowUp" ? step : event.key === "ArrowDown" ? -step : 0)));
        }
      }}>
      <path d={`M0 255 ${bins.map((count, x) => `L${x} ${255 - Math.sqrt(count / peak) * 230}`).join(" ")} L255 255Z`} fill={COLORS[channel]} opacity=".13" />
      {[64, 128, 192].map((v) => <path key={v} d={`M${v} 0V255M0 ${v}H255`} stroke="currentColor" className="text-edge" />)}
      <path d="M0 255 255 0" stroke="currentColor" className="text-text-muted/30" />
      <polyline points={samples.map((y, x) => `${x},${255 - y}`).join(" ")} fill="none" stroke={COLORS[channel]} strokeWidth="2" />
      {points.map(([x, y], i) => <circle key={i} cx={x} cy={255 - y} r={selected === i ? 5 : 3.5} fill={selected === i ? "white" : COLORS[channel]} stroke={COLORS[channel]} />)}
    </svg>
    <CurveCoordinates key={channel} points={points} selected={selected} onChange={curve=>{finish();apply(curve);}}/>
    {histogram.length>0&&<p className="text-[11px] text-text-muted">{t("advancedEditor.histogramOriginal")}</p>}
    {sampling.available!==false&&<div className="flex gap-1">{(["point", "black", "gray", "white"] as const).map((picker) => <button key={picker} type="button" disabled={sampling.busy} aria-pressed={sampling.picker === picker} className={`flex-1 rounded-lg border px-1 py-1.5 text-[11px] ${sampling.picker === picker ? "border-accent text-accent" : "border-edge"}`} onClick={() => sampling.setPicker(sampling.picker === picker ? null : picker)}>{t(`advancedEditor.pickers.${picker}`)}</button>)}</div>}
    {sampling.available!==false&&sampling.setTargetCurveChannel&&<button type="button" className="w-full rounded-lg border border-edge px-2 py-2 text-xs aria-pressed:border-accent aria-pressed:text-accent" disabled={sampling.busy} aria-pressed={sampling.picker==="curveTarget"} onClick={()=>{sampling.setTargetCurveChannel!(channel);sampling.setPicker(sampling.picker==="curveTarget"?null:"curveTarget");}}>{t("advancedEditor.targetCurve")}</button>}
    {sampling.picker==="curveTarget"&&<p className="text-[11px] text-text-muted">{t("advancedEditor.targetHint")}</p>}
    <button type="button" className="text-[11px] text-accent" disabled={!raw?.length} onClick={()=>{finish();apply([]);setSelected(null);}}>{t("advancedEditor.resetCurveChannel")}</button>
    <div className="flex gap-1">{Object.entries(PRESETS).map(([name, curve]) => <button key={name} className="flex-1 rounded-lg border border-edge py-1.5 text-[11px]" onClick={() => { apply(curve); setSelected(null); }}>{t(`advancedEditor.curve.${name}`)}</button>)}</div>
    {sampling.busy&&<p role="status" className="text-xs text-text-muted">{t("advancedEditor.sampling")}</p>}
    <p className="text-[11px] leading-relaxed text-text-muted">{t("advancedEditor.curveInteractionHint")}</p>
  </details>;
}
