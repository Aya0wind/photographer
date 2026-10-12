import {useState} from "react";
import {useTranslation} from "react-i18next";
import type {AdvancedAdjustments} from "@/ipc/api";
import Slider from "./AdjustmentSlider";
import ColorWheel from "./ColorWheel";
export const DEFAULT_GRADING:NonNullable<AdvancedAdjustments["colorGrading"]>={shadows:{hue:0,sat:0,lum:0},midtones:{hue:0,sat:0,lum:0},highlights:{hue:0,sat:0,lum:0},global:{hue:0,sat:0,lum:0},blending:50,balance:0};
export default function ColorGradingPanel({value,onChange,onReset,begin,end,cancel}:{value:AdvancedAdjustments["colorGrading"];onChange:(value:AdvancedAdjustments["colorGrading"],record?:boolean)=>void;onReset:()=>void;begin:()=>void;end:()=>void;cancel?:()=>void}){
 const {t}=useTranslation();const [tone,setTone]=useState<"shadows"|"midtones"|"highlights"|"global">("midtones");const g=value??DEFAULT_GRADING,w=g[tone];
 return <details role="group" aria-label={t("advancedEditor.grading.title")} className="space-y-3 border-t border-edge pt-3"><summary className="flex cursor-pointer items-center justify-between text-xs font-semibold"><span>{t("advancedEditor.grading.title")}</span><button type="button" disabled={!value} aria-label={t("advancedEditor.resetGroup",{group:t("advancedEditor.grading.title")})} className="rounded px-2 py-1 text-[11px] text-accent disabled:opacity-30" onClick={e=>{e.preventDefault();e.stopPropagation();end();onReset();}}>{t("editor.reset")}</button></summary>
  <select className="w-full rounded-lg border border-edge bg-bg p-2 text-xs" aria-label={t("advancedEditor.grading.range")} value={tone} onChange={e=>{end();setTone(e.target.value as typeof tone);}}>{["shadows","midtones","highlights","global"].map(key=><option key={key} value={key}>{t(key==="global"?"advancedEditor.grading.global":`advancedEditor.extra.${key}`)}</option>)}</select>
  <ColorWheel label={t("advancedEditor.grading.wheel")} value={w} onChange={(wheel,record)=>onChange({...g,[tone]:wheel},record)} begin={begin} end={end} cancel={cancel}/>
  <p className="text-[11px] leading-relaxed text-text-muted">{t("advancedEditor.grading.hint")}</p>
  {(["hue","sat","lum"] as const).map(key=><Slider key={key} label={t(`advancedEditor.grading.${key}`)} value={w[key]} min={key==="lum"?-100:0} max={key==="hue"?360:100} step={key==="hue"?0.1:1} unit={key==="hue"?"°":undefined} onChange={v=>onChange({...g,[tone]:{...w,[key]:v}},false)} onSet={v=>onChange({...g,[tone]:{...w,[key]:v}})} onReset={()=>onChange({...g,[tone]:{...w,[key]:0}})} begin={begin} end={end}/>)}
  <button type="button" disabled={!w.hue&&!w.sat&&!w.lum} className="text-xs text-accent disabled:opacity-30" onClick={()=>{end();onChange({...g,[tone]:{hue:0,sat:0,lum:0}});}}>{t("advancedEditor.extra.resetTone")}</button>
  <Slider label={t("advancedEditor.grading.blending")} value={g.blending} defaultValue={50} min={0} max={100} onChange={blending=>onChange({...g,blending},false)} onSet={blending=>onChange({...g,blending})} onReset={()=>onChange({...g,blending:50})} begin={begin} end={end}/>
  <Slider label={t("advancedEditor.grading.balance")} value={g.balance} onChange={balance=>onChange({...g,balance},false)} onSet={balance=>onChange({...g,balance})} onReset={()=>onChange({...g,balance:0})} begin={begin} end={end}/>
 </details>;
}
