import {useTranslation} from "react-i18next";
import type {EditRecipe} from "@/ipc/api";
import Slider from "./AdjustmentSlider";
type Balance=NonNullable<EditRecipe["colorPipeline"]>["rawWhiteBalance"];
export default function RawWhiteBalancePanel({value,onChange,onPicker,ready,disabled,begin,end}:{value:Balance;onChange:(value:Balance,record?:boolean)=>void;begin:()=>void;end:()=>void;onPicker:()=>void;ready:boolean;disabled:boolean}){
 const {t}=useTranslation();const temperature=value?.mode==="temperature"?value:{mode:"temperature" as const,kelvin:6500,tint:0};
 const mode=value?.mode??"asShot";
 return <fieldset disabled={disabled} className="space-y-3 border-t border-edge pt-3">
  <legend className="text-xs font-semibold">{t("advancedEditor.rawWb.title")}</legend>
  <select aria-label={t("advancedEditor.rawWb.mode")} value={mode} onChange={e=>onChange(e.target.value==="temperature"?temperature:{mode:e.target.value as "asShot"|"auto"})} className="w-full rounded border border-edge bg-bg p-2 text-xs">{["asShot","auto","temperature"].map(mode=><option key={mode} value={mode}>{t(`advancedEditor.rawWb.${mode}`)}</option>)}</select>
  <div className="flex flex-wrap gap-1">{[[2856,"tungsten"],[4000,"fluorescent"],[5500,"daylight"],[6500,"cloudy"],[7500,"shade"]].map(([kelvin,name])=><button type="button" key={name} className="rounded border border-edge p-1 text-[11px]" onClick={()=>onChange({mode:"temperature",kelvin:Number(kelvin),tint:0})}>{t(`advancedEditor.rawWb.${name}`)}</button>)}</div>
  {mode==="temperature"&&<><Slider label={t("advancedEditor.rawWb.kelvin")} unit="K" min={2000} max={50000} step={50} value={temperature.kelvin} defaultValue={6500} onChange={kelvin=>onChange({...temperature,kelvin},false)} onSet={kelvin=>onChange({...temperature,kelvin})} onReset={()=>onChange({...temperature,kelvin:6500})} begin={begin} end={end}/><Slider label={t("advancedEditor.rawWb.tint")} min={-150} max={150} value={temperature.tint} onChange={tint=>onChange({...temperature,tint},false)} onSet={tint=>onChange({...temperature,tint})} onReset={()=>onChange({...temperature,tint:0})} begin={begin} end={end}/></>}
  <button type="button" disabled={!ready} onClick={onPicker} className="w-full rounded border border-edge p-2 text-xs disabled:opacity-30">{t("advancedEditor.rawWb.picker")}</button>

 </fieldset>;
}
