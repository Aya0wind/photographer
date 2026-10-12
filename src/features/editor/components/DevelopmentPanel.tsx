import {useTranslation} from "react-i18next";
import type {AdvancedAdjustments,DevelopmentAdjustments} from "@/ipc/api";
import {DEFAULT_DEVELOPMENT,DEVELOPMENT_GROUPS,developmentGroupChanged,type DevelopmentGroup} from "../lib/advancedRecipe";
import Slider from "./AdjustmentSlider";
export function DevelopmentControls({adjustments,group,onChange,begin,end}:{adjustments:AdvancedAdjustments;group:DevelopmentGroup;onChange:(patch:Partial<AdvancedAdjustments>,record?:boolean)=>void;begin:()=>void;end:()=>void}) {
 const {t}=useTranslation();const d={...DEFAULT_DEVELOPMENT,...adjustments.development};
 function set(key:keyof DevelopmentAdjustments,value:number|string,record=true){onChange({development:{...d,[key]:value}},record);}
 return <>{DEVELOPMENT_GROUPS[group].map(key=>{
  if(key==="vignetteStyle")return <label key={key} className="block space-y-1 text-xs text-text-secondary"><span>{t("advancedEditor.develop.vignetteStyle")}</span><select className="w-full rounded-lg border border-edge bg-bg p-2" value={d.vignetteStyle} onChange={e=>set(key,e.target.value)}>{["highlightPriority","colorPriority","paintOverlay"].map(style=><option key={style} value={style}>{t(`advancedEditor.develop.${style}`)}</option>)}</select></label>;
  const min=["highlights","shadows","whites","blacks","texture","clarity","dehaze","vignetteAmount","vignetteRoundness"].includes(key)?-100:key==="sharpenRadius"?0.5:0;
  const max=key==="sharpenRadius"?3:key==="sharpenAmount"?150:100;
  return <Slider key={key} label={t(`advancedEditor.develop.${key}`)} unit={key==="sharpenRadius"?"px":undefined} min={min} max={max} step={key==="sharpenRadius"?0.1:1} value={d[key]} defaultValue={DEFAULT_DEVELOPMENT[key]} onChange={v=>set(key,v,false)} onSet={v=>set(key,v)} onReset={()=>set(key,DEFAULT_DEVELOPMENT[key])} begin={begin} end={end}/>;
 })}</>;
}
export default function DevelopmentPanel({adjustments,onChange,onReset,begin,end}:{
 adjustments:AdvancedAdjustments;onChange:(patch:Partial<AdvancedAdjustments>,record?:boolean)=>void;onReset:(group:DevelopmentGroup)=>void;begin:()=>void;end:()=>void;
}) {
 const {t}=useTranslation();
 return <div className="space-y-3">{(Object.keys(DEVELOPMENT_GROUPS) as DevelopmentGroup[]).map(group=><details key={group} role="group" aria-label={t(`advancedEditor.develop.${group}`)} className="space-y-2.5 border-t border-edge pt-3">
  <summary className="flex cursor-pointer items-center justify-between text-xs font-semibold"><span>{t(`advancedEditor.develop.${group}`)}</span><button type="button" className="rounded px-2 py-1 text-[11px] text-accent disabled:opacity-30" disabled={!developmentGroupChanged(adjustments,group)} aria-label={t("advancedEditor.resetGroup",{group:t(`advancedEditor.develop.${group}`)})} onClick={e=>{e.preventDefault();e.stopPropagation();end();onReset(group);}}>{t("editor.reset")}</button></summary>
  <DevelopmentControls adjustments={adjustments} group={group} onChange={onChange} begin={begin} end={end}/>
 </details>)}</div>;
}
