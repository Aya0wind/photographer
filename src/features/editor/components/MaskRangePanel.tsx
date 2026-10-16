import {useTranslation} from "react-i18next";
import type {EditLocalMask} from "@/ipc/api";
import Slider from "./AdjustmentSlider";
export const DEFAULT_MASK_RANGE={kind:"color" as const,color:"#808080",tolerance:50,low:64,high:192,smoothness:32};
export default function MaskRangePanel({value,onChange,begin,end}:{value:EditLocalMask["range"];onChange:(value:NonNullable<EditLocalMask["range"]>,record?:boolean)=>void;begin:()=>void;end:()=>void}){
 const {t}=useTranslation();const range=value??DEFAULT_MASK_RANGE;
 return <section className="space-y-2 rounded border border-edge p-2"><select className="w-full rounded border border-edge bg-bg p-2 text-xs" aria-label={t("advancedEditor.maskRange.mode")} value={range.kind} onChange={e=>onChange({...range,kind:e.target.value as typeof range.kind})}>{["color","luminance"].map(kind=><option key={kind} value={kind}>{t(`advancedEditor.maskRange.${kind}`)}</option>)}</select>
 {range.kind==="color"&&<label className="flex justify-between text-xs">{t("advancedEditor.maskRange.color")}<input type="color" value={range.color} onChange={e=>onChange({...range,color:e.target.value})}/></label>}
 {(range.kind==="color"?["tolerance"] as const:["low","high","smoothness"] as const).map(key=><Slider key={key} label={t(`advancedEditor.maskRange.${key}`)} value={range[key]} min={key==="high"?range.low:0} max={key==="tolerance"?200:key==="low"?range.high:255} onChange={value=>onChange({...range,[key]:value},false)} onSet={value=>onChange({...range,[key]:value})} onReset={()=>onChange({...range,[key]:key==="low"?0:key==="high"?255:DEFAULT_MASK_RANGE[key]})} begin={begin} end={end}/>)}
 </section>;
}
