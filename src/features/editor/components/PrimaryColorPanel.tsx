import {useState} from "react";
import {useTranslation} from "react-i18next";
import type {AdvancedAdjustments} from "@/ipc/api";
import Slider from "./AdjustmentSlider";
export default function PrimaryColorPanel({value,onChange,begin,end}:{value:AdvancedAdjustments["primaries"];onChange:(value:AdvancedAdjustments["primaries"],record?:boolean)=>void;begin:()=>void;end:()=>void}){
  const {t}=useTranslation();const [primary,setPrimary]=useState<"red"|"green"|"blue">("red");
  const current=value?.[primary]??{hue:0,saturation:0};
  function update(key:"hue"|"saturation",amount:number,record=true){onChange({...value,[primary]:{...current,[key]:amount}},record);}
  return <details className="space-y-2 rounded border border-edge p-2"><summary className="text-xs font-semibold">{t("advancedEditor.primaries.title")}</summary>
    <select aria-label={t("advancedEditor.primaries.title")} className="w-full rounded border border-edge bg-bg p-2 text-xs" value={primary} onChange={e=>{end();setPrimary(e.target.value as typeof primary);}}>{["red","green","blue"].map(key=><option key={key} value={key}>{t(`advancedEditor.channels.${key}`)}</option>)}</select>
    {(["hue","saturation"] as const).map(key=><Slider key={key} label={t(`advancedEditor.hslLabels.${key}`)} value={current[key]} min={key==="hue"?-30:-100} max={key==="hue"?30:100} unit={key==="hue"?"°":"%"} onChange={v=>update(key,v,false)} onSet={v=>update(key,v)} onReset={()=>update(key,0)} begin={begin} end={end}/>)}
    <button type="button" disabled={!value} className="text-xs text-accent disabled:opacity-30" onClick={()=>{end();onChange(undefined);}}>{t("editor.reset")}</button>

  </details>;
}
