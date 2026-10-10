import {useState} from "react";
import {useTranslation} from "react-i18next";
import type {AdvancedAdjustments} from "@/ipc/api";
import type {AdjustmentGroup} from "../lib/advancedRecipe";
import Slider from "./AdjustmentSlider";
export const BW_DEFAULTS:[number,number,number,number,number,number]=[40,60,40,60,20,80];
const COLORS=["reds","yellows","greens","cyans","blues","magentas"] as const;
const ALL_COLORS=[...COLORS,"whites","neutrals","blacks"] as const;
const SELECT="w-full rounded-lg border border-edge bg-bg px-2 py-1.5 text-xs";
export default function ExtendedColorPanel({adjustments:a,onChange,onReset,begin,end}:{
 adjustments:AdvancedAdjustments;onChange:(patch:Partial<AdvancedAdjustments>,record?:boolean)=>void;onReset:(group:AdjustmentGroup)=>void;begin:()=>void;end:()=>void;
}) {
 const {t}=useTranslation();
 const [tone,setTone]=useState<"shadows"|"midtones"|"highlights">("midtones");
 const [range,setRange]=useState<typeof ALL_COLORS[number]>("reds");
 const balance=a.colorBalance??{shadows:[0,0,0],midtones:[0,0,0],highlights:[0,0,0],preserveLuminosity:true};
 const bw=a.blackWhite??{enabled:false,weights:BW_DEFAULTS,tint:null};
 const selective=a.selectiveColor??{relative:true,ranges:{}};
 const values=selective.ranges[range]??[0,0,0,0];
 function reset(group:AdjustmentGroup,changed:boolean){return <button type="button" disabled={!changed} aria-label={t("advancedEditor.resetGroup",{group:t(`advancedEditor.extra.${group}`)})} className="rounded px-2 py-1 text-[11px] text-accent disabled:opacity-30" onClick={e=>{e.preventDefault();e.stopPropagation();end();onReset(group);}}>{t("editor.reset")}</button>;}
 function group(group:AdjustmentGroup,changed:boolean,children:React.ReactNode){return <details role="group" aria-label={t(`advancedEditor.extra.${group}`)} className="space-y-3 border-t border-edge pt-3"><summary className="flex cursor-pointer items-center justify-between text-xs font-semibold"><span>{t(`advancedEditor.extra.${group}`)}</span>{reset(group,changed)}</summary>{children}</details>;}
 function balanceChannel(index:number,value:number,record=true){
  const row:[number,number,number]=[...balance[tone]];row[index]=value;onChange({colorBalance:{...balance,[tone]:row}},record);
 }
 function bwChannel(index:number,value:number,record=true){const weights:[number,number,number,number,number,number]=[...bw.weights];weights[index]=value;onChange({blackWhite:{...bw,weights}},record);}
 function selectiveChannel(index:number,value:number,record=true){const row:[number,number,number,number]=[...values];row[index]=value;onChange({selectiveColor:{...selective,ranges:{...selective.ranges,[range]:row}}},record);}
 return <>
 {group("colorBalance",!!a.colorBalance,<>
  <select className={SELECT} aria-label={t("advancedEditor.extra.tone")} value={tone} onChange={e=>{end();setTone(e.target.value as typeof tone);}}>{(["shadows","midtones","highlights"] as const).map(key=><option key={key} value={key}>{t(`advancedEditor.extra.${key}`)}</option>)}</select>
  {(["cyanRed","magentaGreen","yellowBlue"] as const).map((key,index)=><Slider key={key} label={t(`advancedEditor.extra.${key}`)} value={balance[tone][index]} onChange={v=>balanceChannel(index,v,false)} onSet={v=>balanceChannel(index,v)} onReset={()=>balanceChannel(index,0)} begin={begin} end={end}/>)}
  <label className="flex items-center gap-2 text-xs"><input type="checkbox" checked={balance.preserveLuminosity} onChange={e=>onChange({colorBalance:{...balance,preserveLuminosity:e.target.checked}})}/>{t("advancedEditor.extra.preserveLuminosity")}</label>
  <button className="text-xs text-accent disabled:opacity-30" disabled={balance[tone].every(v=>v===0)} onClick={()=>{end();onChange({colorBalance:{...balance,[tone]:[0,0,0]}});}}>{t("advancedEditor.extra.resetTone")}</button>
 </>)}
 {group("blackWhite",!!a.blackWhite,<>
  <label className="flex items-center gap-2 text-xs"><input type="checkbox" checked={bw.enabled} onChange={e=>onChange({blackWhite:{...bw,enabled:e.target.checked}})}/>{t("advancedEditor.extra.enableBlackWhite")}</label>
  <fieldset disabled={!bw.enabled} className="space-y-2 disabled:opacity-40">
   {COLORS.map((key,index)=><Slider key={key} label={t(`advancedEditor.ranges.${key}`)} min={-200} max={300} value={bw.weights[index]} defaultValue={BW_DEFAULTS[index]} onChange={v=>bwChannel(index,v,false)} onSet={v=>bwChannel(index,v)} onReset={()=>bwChannel(index,BW_DEFAULTS[index])} begin={begin} end={end}/>)}
   <label className="flex items-center gap-2 text-xs"><input type="checkbox" checked={bw.tint!==null} onChange={e=>onChange({blackWhite:{...bw,tint:e.target.checked?"#E1D3B3":null}})}/>{t("advancedEditor.extra.tint")}</label>
   {bw.tint&&<label className="flex items-center justify-between text-xs">{t("advancedEditor.extra.tintColor")}<input type="color" value={bw.tint} onChange={e=>onChange({blackWhite:{...bw,tint:e.target.value}})}/></label>}
  </fieldset>
 </>)}
 {group("selectiveColor",!!a.selectiveColor,<>
  <select className={SELECT} aria-label={`${t("advancedEditor.extra.selectiveColor")} · ${t("advancedEditor.colorRange")}`} value={range} onChange={e=>{end();setRange(e.target.value as typeof range);}}>{ALL_COLORS.map(key=><option key={key} value={key}>{t(COLORS.includes(key as typeof COLORS[number])?`advancedEditor.ranges.${key}`:`advancedEditor.extra.${key}`)}</option>)}</select>
  <select className={SELECT} aria-label={t("advancedEditor.extra.method")} value={selective.relative?"relative":"absolute"} onChange={e=>onChange({selectiveColor:{...selective,relative:e.target.value==="relative"}})}>{["relative","absolute"].map(key=><option key={key} value={key}>{t(`advancedEditor.extra.${key}`)}</option>)}</select>
  {(["cyan","magenta","yellow","black"] as const).map((key,index)=><Slider key={key} label={t(`advancedEditor.extra.${key}`)} unit="%" value={values[index]} onChange={v=>selectiveChannel(index,v,false)} onSet={v=>selectiveChannel(index,v)} onReset={()=>selectiveChannel(index,0)} begin={begin} end={end}/>)}
  <button className="text-xs text-accent disabled:opacity-30" disabled={values.every(v=>v===0)} onClick={()=>{end();const ranges={...selective.ranges};delete ranges[range];onChange({selectiveColor:{...selective,ranges}});}}>{t("advancedEditor.resetColorRange")}</button>
 </>)}
 </>;
}
