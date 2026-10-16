import {useEffect,useRef,type ReactNode} from "react";
import {useTranslation} from "react-i18next";
export type HistogramZone="blacks"|"shadows"|"exposure"|"highlights"|"whites";
export interface HistogramGesture {phase:"start"|"move"|"end"|"cancel";zone:HistogramZone;delta:number}
const ZONES:HistogramZone[]=["blacks","shadows","exposure","highlights","whites"];
export default function HistogramPanel({bins,onOpen,children,onToneGesture}:{bins:number[][];busy?:boolean;onOpen:(open:boolean)=>void;children?:ReactNode;onToneGesture?:(event:HistogramGesture)=>void}){
 const drag=useRef<{id:number;x:number;width:number;zone:HistogramZone}|null>(null);
 function finish(cancel=false){const value=drag.current;if(!value)return;drag.current=null;onToneGesture?.({phase:cancel?"cancel":"end",zone:value.zone,delta:0});}
 const {t}=useTranslation();useEffect(()=>{onOpen(true);return()=>onOpen(false);},[onOpen]);const peak=Math.max(1,...bins.slice(0,3).flat());
 return <details open className="space-y-2 border-b border-edge pb-3" onToggle={e=>onOpen(e.currentTarget.open)}>
  <summary className="flex cursor-pointer items-center justify-between text-xs font-semibold"><span>{t("advancedEditor.histogram")}</span></summary>
  <svg role="img" aria-label={t("advancedEditor.histogramCurrent")} viewBox="0 0 255 100" preserveAspectRatio="none" className="aspect-[2/1] w-full touch-none rounded-lg border border-edge bg-bg"
    onPointerDown={event=>{if(!onToneGesture||event.button!==0)return;event.preventDefault();const r=event.currentTarget.getBoundingClientRect();const zone=ZONES[Math.max(0,Math.min(4,Math.floor((event.clientX-r.left)/r.width*5)))];drag.current={id:event.pointerId,x:event.clientX,width:r.width,zone};event.currentTarget.setPointerCapture(event.pointerId);onToneGesture({phase:"start",zone,delta:0});}}
    onPointerMove={event=>{const g=drag.current;if(g?.id===event.pointerId)onToneGesture?.({phase:"move",zone:g.zone,delta:(event.clientX-g.x)/g.width});}}
    onPointerUp={()=>finish()} onPointerCancel={()=>finish(true)} onLostPointerCapture={()=>finish()}>{bins.slice(0,3).map((channel,index)=><path key={index} d={`M0 100 ${channel.map((value,x)=>`L${x} ${100-Math.sqrt(value/peak)*96}`).join(" ")} L255 100Z`} fill={["#f87171","#4ade80","#60a5fa"][index]} fillOpacity={0.2} stroke={["#f87171","#4ade80","#60a5fa"][index]} strokeWidth={0.6}/>)}</svg>
  {onToneGesture&&<div className="grid grid-cols-5 gap-1 text-center text-[9px] text-text-muted">{ZONES.map(zone=><span key={zone}>{t(zone==="exposure"?"advancedEditor.exposure":`advancedEditor.develop.${zone}`)}</span>)}</div>}
  {onToneGesture&&<p className="text-[10px] text-text-muted">{t("advancedEditor.histogramDrag")}</p>}
  {children}
 </details>;
}
