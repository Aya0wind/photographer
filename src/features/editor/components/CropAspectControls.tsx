import {useState} from "react";
import {useTranslation} from "react-i18next";
import {fitCropRect,type Size} from "../lib/coords";
import type {EditRecipe} from "@/ipc/api";

export default function CropAspectControls({ratio,onRatio,size,original,onCrop}:{
  ratio:number|null;onRatio:(value:number|null)=>void;size:Size;original:number;
  onCrop:(value:NonNullable<EditRecipe["crop"]>)=>void;
}) {
  const {t}=useTranslation();
  const [width,setWidth]=useState("3"),[height,setHeight]=useState("2");
  function choose(value:number|null){
    onRatio(value);
    if(value!==null)onCrop(fitCropRect(value,size.width/size.height));
  }
  const valid=Number(width)>0&&Number(height)>0&&Number.isFinite(Number(width)/Number(height));
  return <section className="space-y-2">
    <div className="flex flex-wrap gap-2">{[null,original,1,4/3,3/2,16/9].map((value,index)=><button key={index} type="button" aria-pressed={ratio===value}
      className="rounded-lg border border-edge px-2 py-1.5 text-xs aria-pressed:border-accent aria-pressed:text-accent" onClick={()=>choose(value)}>
      {[t("editor.crop.free"),t("advancedEditor.crop.original"),"1:1","4:3","3:2","16:9"][index]}</button>)}</div>
    <div className="flex items-center justify-between gap-2 text-xs">
      <label className="flex items-center gap-2"><input type="checkbox" checked={ratio!==null} onChange={event=>choose(event.target.checked?original:null)}/>{t("advancedEditor.crop.lock")}</label>
      <button type="button" disabled={ratio===null} className="rounded-lg border border-edge px-2 py-1.5 disabled:opacity-30" onClick={()=>{if(ratio!==null)choose(1/ratio);}}>{t("advancedEditor.crop.swap")}</button>
    </div>
    <form className="flex items-center gap-2" onSubmit={event=>{event.preventDefault();if(valid)choose(Number(width)/Number(height));}}>
      <input type="number" min={0.01} step="any" value={width} onChange={event=>setWidth(event.target.value)} aria-label={t("advancedEditor.crop.ratioWidth")} className="min-w-0 flex-1 rounded-lg border border-edge bg-bg p-2 text-xs"/>
      <span>:</span><input type="number" min={0.01} step="any" value={height} onChange={event=>setHeight(event.target.value)} aria-label={t("advancedEditor.crop.ratioHeight")} className="min-w-0 flex-1 rounded-lg border border-edge bg-bg p-2 text-xs"/>
      <button type="submit" disabled={!valid} className="rounded-lg border border-edge px-2 py-2 text-xs disabled:opacity-30">{t("advancedEditor.crop.custom")}</button>
    </form>
  </section>;
}
