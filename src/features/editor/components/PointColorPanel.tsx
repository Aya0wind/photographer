import {useEffect,useRef,useState} from "react";
import {useTranslation} from "react-i18next";
import type {AdvancedAdjustments} from "@/ipc/api";
import type {CurveSampling} from "./CurveEditor";
import Slider from "./AdjustmentSlider";
import {newLayerId} from "../lib/recipe";
type Point=NonNullable<AdvancedAdjustments["pointColors"]>[number];
function create(color:string):Point{return {id:newLayerId(),enabled:true,color,hueRange:30,saturationRange:40,luminanceRange:40,hue:0,saturation:0,luminance:0};}
export default function PointColorPanel({value,onChange,sampling,begin,end}:{value:AdvancedAdjustments["pointColors"];onChange:(value:Point[],record?:boolean)=>void;sampling:CurveSampling;begin:()=>void;end:()=>void}){
  const {t}=useTranslation();const points=value??[];
  const [selected,setSelected]=useState<string|null>(null);
  const point=points.find(p=>p.id===selected)??points[0];
  const seen=useRef(sampling.sample);
  function update(patch:Partial<Point>,record=true){if(point)onChange(points.map(p=>p.id===point.id?{...p,...patch}:p),record);}
  useEffect(()=>{
    if(seen.current===sampling.sample)return;seen.current=sampling.sample;
    if(sampling.picker!=="pointColor"||!sampling.sample)return;
    const color="#"+sampling.sample.map(v=>Math.round(Math.max(0,Math.min(255,v))).toString(16).padStart(2,"0")).join("");
    if(point)update({color});else if(points.length<8){const next=create(color);onChange([...points,next]);setSelected(next.id);}
    sampling.setPicker(null);
  },[sampling.sample,sampling.picker,point,points,onChange]);
  return <details className="space-y-2 rounded border border-edge p-2"><summary className="text-xs font-semibold">{t("advancedEditor.pointColor.title")}</summary>
    <div className="flex flex-wrap gap-2">{points.map((p,index)=><button key={p.id} type="button" className="h-8 w-8 rounded border-2 border-edge aria-pressed:border-white" style={{backgroundColor:p.color}} aria-label={t("advancedEditor.pointColor.select",{index:index+1})} aria-pressed={p.id===point?.id} onClick={()=>{end();setSelected(p.id);}}/>)}<button type="button" disabled={points.length>=8} className="rounded border border-edge px-2 text-xs disabled:opacity-30" onClick={()=>{end();const next=create("#808080");onChange([...points,next]);setSelected(next.id);}}>＋</button></div>
    {sampling.available!==false&&<button type="button" disabled={sampling.busy} aria-pressed={sampling.picker==="pointColor"} className="w-full rounded border border-edge p-2 text-xs aria-pressed:text-accent" onClick={()=>sampling.setPicker(sampling.picker==="pointColor"?null:"pointColor")}>{t("advancedEditor.pointColor.sample")}</button>}
    {point&&<>
      <label className="flex items-center justify-between text-xs">{t("advancedEditor.maskRange.color")}<input type="color" value={point.color} onChange={e=>update({color:e.target.value})}/></label>
      <label className="flex items-center gap-2 text-xs"><input type="checkbox" checked={point.enabled} onChange={e=>update({enabled:e.target.checked})}/>{t("advancedEditor.pointColor.enable")}</label>
      {(["hueRange","saturationRange","luminanceRange","hue","saturation","luminance"] as const).map(key=><Slider key={key} label={t(`advancedEditor.pointColor.${key}`)} value={point[key]} min={key.endsWith("Range")?1:key==="hue"?-180:-100} max={key==="hueRange"||key==="hue"?180:100} unit={key==="hueRange"||key==="hue"?"°":"%"} onChange={value=>update({[key]:value},false)} onSet={value=>update({[key]:value})} onReset={()=>update({[key]:key==="hueRange"?30:key.endsWith("Range")?40:0})} begin={begin} end={end}/>)}
      <button type="button" className="text-xs text-accent" onClick={()=>{end();onChange(points.filter(p=>p.id!==point.id));setSelected(null);}}>{t("advancedEditor.pointColor.remove")}</button>
    </>}
    <p className="text-[11px] text-text-muted">{t("advancedEditor.pointColor.hint")}</p>
  </details>;
}
