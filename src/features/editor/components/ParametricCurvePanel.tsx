import {useTranslation} from "react-i18next";
import {useEffect,useLayoutEffect,useRef,useState} from "react";
import {editParametricSamples} from "@/ipc/api";
import type {AdvancedAdjustments} from "@/ipc/api";
import Slider from "./AdjustmentSlider";
import ErrorModal from "@/shared/components/ErrorModal";
export const DEFAULT_PARAMETRIC:NonNullable<AdvancedAdjustments["parametricCurve"]>={highlights:0,lights:0,darks:0,shadows:0,splits:[25,50,75]};
export default function ParametricCurvePanel({value,onChange,onReset,begin,end,onModalChange}:{onModalChange?:(open:boolean)=>void;value:AdvancedAdjustments["parametricCurve"];onChange:(value:AdvancedAdjustments["parametricCurve"],record?:boolean)=>void;onReset:()=>void;begin:()=>void;end:()=>void}){
 const {t}=useTranslation();const c=value??DEFAULT_PARAMETRIC;
 const [open,setOpen]=useState(false),[samples,setSamples]=useState<number[]>(Array.from({length:256},(_,i)=>i)),[error,setError]=useState<string|null>(null);
 const latest=useRef(c);latest.current=c;const signature=JSON.stringify(c);
 useLayoutEffect(()=>{onModalChange?.(!!error);return()=>onModalChange?.(false);},[error,onModalChange]);
 useEffect(()=>{
  if(!error)return;
  const close=(event:KeyboardEvent)=>{if(event.key==="Escape"){event.preventDefault();event.stopImmediatePropagation();setError(null);}};
  window.addEventListener("keydown",close,true);return()=>window.removeEventListener("keydown",close,true);
 },[error]);
 useEffect(()=>{if(!open)return;if(![c.highlights,c.lights,c.darks,c.shadows].some(v=>v!==0)){setSamples(Array.from({length:256},(_,i)=>i));setError(null);return;}
  let cancelled=false;const timer=setTimeout(()=>{void editParametricSamples(latest.current).then(samples=>{if(!cancelled){setSamples(samples);setError(null);}}).catch(e=>{if(!cancelled)setError(String(e));});},32);
  return()=>{cancelled=true;clearTimeout(timer);};
 },[open,signature]);
 function split(index:number,v:number,record=true){const splits:[number,number,number]=[...c.splits];splits[index]=v;onChange({...c,splits},record);}
 return <details role="group" onToggle={e=>setOpen(e.currentTarget.open)} aria-label={t("advancedEditor.parametric.title")} className="space-y-3 border-t border-edge pt-3"><summary className="flex cursor-pointer items-center justify-between text-xs font-semibold"><span>{t("advancedEditor.parametric.title")}</span><button type="button" disabled={!value} className="rounded px-2 py-1 text-[11px] text-accent disabled:opacity-30" aria-label={t("advancedEditor.resetGroup",{group:t("advancedEditor.parametric.title")})} onClick={e=>{e.preventDefault();e.stopPropagation();end();onReset();}}>{t("editor.reset")}</button></summary>
 <svg role="img" aria-label={t("advancedEditor.parametric.title")} viewBox="0 0 100 100" preserveAspectRatio="none" className="aspect-[2/1] w-full rounded-lg border border-edge bg-bg"><path d="M0 100 100 0" stroke="currentColor" className="text-text-muted/30" fill="none"/>{c.splits.map((x,i)=><line key={i} x1={x} y1={0} x2={x} y2={100} stroke="currentColor" className="text-edge"/>)}<polyline points={samples.map((y,x)=>`${x/(samples.length-1)*100},${100-y/255*100}`).join(" ")} fill="none" stroke="currentColor" className="text-accent" strokeWidth={1} vectorEffect="non-scaling-stroke"/></svg>
 <ErrorModal message={error} onClose={()=>setError(null)}/>
 {(["highlights","lights","darks","shadows"] as const).map(key=><Slider key={key} label={t(`advancedEditor.parametric.${key}`)} value={c[key]} onChange={v=>onChange({...c,[key]:v},false)} onSet={v=>onChange({...c,[key]:v})} onReset={()=>onChange({...c,[key]:0})} begin={begin} end={end}/>)}
 <div className="border-t border-edge pt-2"><h4 className="mb-2 text-xs text-text-muted">{t("advancedEditor.parametric.splits")}</h4>{c.splits.map((value,index)=><Slider key={index} label={t(`advancedEditor.parametric.split${index}`)} unit="%" value={value} defaultValue={DEFAULT_PARAMETRIC.splits[index]} min={index===0?5:c.splits[index-1]+1} max={index===2?95:c.splits[index+1]-1} onChange={v=>split(index,v,false)} onSet={v=>split(index,v)} onReset={()=>split(index,Math.max(index===0?5:c.splits[index-1]+1,Math.min(index===2?95:c.splits[index+1]-1,DEFAULT_PARAMETRIC.splits[index])))} begin={begin} end={end}/>)}</div>
 </details>;
}
