import {useEffect,type ReactNode} from "react";
import {useTranslation} from "react-i18next";
export default function HistogramPanel({bins,onOpen,children}:{bins:number[][];busy?:boolean;onOpen:(open:boolean)=>void;children?:ReactNode}){
 const {t}=useTranslation();useEffect(()=>{onOpen(true);return()=>onOpen(false);},[onOpen]);const peak=Math.max(1,...bins.slice(0,3).flat());
 return <details open className="space-y-2 border-b border-edge pb-3" onToggle={e=>onOpen(e.currentTarget.open)}>
  <summary className="flex cursor-pointer items-center justify-between text-xs font-semibold"><span>{t("advancedEditor.histogram")}</span></summary>
  <svg role="img" aria-label={t("advancedEditor.histogramCurrent")} viewBox="0 0 255 100" preserveAspectRatio="none" className="aspect-[2/1] w-full rounded-lg border border-edge bg-bg">{bins.slice(0,3).map((channel,index)=><path key={index} d={`M0 100 ${channel.map((value,x)=>`L${x} ${100-Math.sqrt(value/peak)*96}`).join(" ")} L255 100Z`} fill={["#f87171","#4ade80","#60a5fa"][index]} fillOpacity={0.2} stroke={["#f87171","#4ade80","#60a5fa"][index]} strokeWidth={0.6}/>)}</svg>
  {children}
 </details>;
}
