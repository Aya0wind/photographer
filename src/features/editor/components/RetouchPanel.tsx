import {useTranslation} from "react-i18next";
import type {EditRecipe,EditRetouchStroke} from "@/ipc/api";
import Slider from "./AdjustmentSlider";
export interface RetouchBrush {kind:EditRetouchStroke["kind"];source:{x:number;y:number}|null;widthRel:number;hardness:number;opacity:number}
export default function RetouchPanel({recipe,brush,onBrush,onChange}:{recipe:EditRecipe;brush:RetouchBrush;onBrush:(value:RetouchBrush)=>void;onChange:(value:EditRetouchStroke[])=>void}){
 const {t}=useTranslation();const strokes=recipe.retouch??[];
 return <section className="space-y-3">
  <select aria-label={t("advancedEditor.retouch.mode")} value={brush.kind} onChange={e=>onBrush({...brush,kind:e.target.value as RetouchBrush["kind"]})} className="w-full rounded border border-edge bg-bg p-2 text-xs">{["clone","heal","redEye"].map(kind=><option key={kind} value={kind}>{t(`advancedEditor.retouch.${kind}`)}</option>)}</select>
  <p className="text-[11px] text-text-muted">{t(brush.kind==="redEye"?"advancedEditor.retouch.redEyeHint":"advancedEditor.retouch.hint")}</p>
  {brush.kind!=="redEye"&&<p className="text-xs">{brush.source?`${t("advancedEditor.retouch.source")}: ${(brush.source.x*100).toFixed(1)}%, ${(brush.source.y*100).toFixed(1)}%`:t("advancedEditor.retouch.chooseSource")}</p>}
  {(["widthRel","hardness","opacity"] as const).map(key=><Slider key={key} label={t(`advancedEditor.retouch.${key}`)} min={key==="widthRel"?0.1:0} max={key==="widthRel"?20:100} step={0.1} unit="%" value={brush[key]*100} onChange={value=>onBrush({...brush,[key]:value/100})} onSet={value=>onBrush({...brush,[key]:value/100})} onReset={()=>onBrush({...brush,[key]:key==="widthRel"?0.02:key==="hardness"?0.5:1})} begin={()=>{}} end={()=>{}}/>)}
  <p className="text-[11px] text-text-muted">{t("advancedEditor.retouch.count",{count:strokes.length})}</p>
  {[...strokes].reverse().map((stroke,index)=><div key={stroke.id} className="flex items-center gap-2 rounded border border-edge p-2 text-xs"><span className="flex-1">{strokes.length-index}. {t(`advancedEditor.retouch.${stroke.kind}`)}</span><button type="button" aria-label={t("advancedEditor.retouch.delete",{index:strokes.length-index})} onClick={()=>onChange(strokes.filter(item=>item.id!==stroke.id))}>×</button></div>)}
 </section>;
}
