import LensProfilePanel from "./LensProfilePanel";
import {useState} from "react";
import {useTranslation} from "react-i18next";
import {editLensAutoscale,type EditorLensSettings,type EditRecipe} from "@/ipc/api";
import {DEFAULT_LENS,fitLensGuides,type LensGuide} from "../lib/lensGuides";
import Slider from "./AdjustmentSlider";
export default function LensPanel({assetId,libraryId,isRaw,onProfile,preparing,value,onChange,recipe,size,axis,onAxis,lines,onLines,grid,onGrid,begin,end}:{assetId:number;libraryId:string;isRaw:boolean;onProfile:(value:NonNullable<EditRecipe["colorPipeline"]>["lensCorrection"])=>void;preparing:boolean;recipe:EditRecipe;value:EditorLensSettings|undefined;onChange:(value:EditorLensSettings|undefined,record?:boolean)=>void;size:{width:number;height:number};axis:LensGuide["axis"]|null;onAxis:(value:LensGuide["axis"]|null)=>void;lines:LensGuide[];onLines:(lines:LensGuide[])=>void;grid:boolean;onGrid:(value:boolean)=>void;begin:()=>void;end:()=>void}){
  const {t}=useTranslation();const settings=value??{...DEFAULT_LENS,enabled:false};const [busy,setBusy]=useState(false),[error,setError]=useState<string|null>(null);
  function set(key:keyof EditorLensSettings,value:number|boolean,record=true){onChange({...settings,[key]:value},record);}
  async function autoScale(){setBusy(true);setError(null);try{const scale=await editLensAutoscale(settings,size.width,size.height);onChange({...settings,scale});}catch(e){setError(String(e));}finally{setBusy(false);}}
  return <section className="editor-lens-panel">
    <LensProfilePanel assetId={assetId} libraryId={libraryId} isRaw={isRaw} value={recipe.colorPipeline?.lensCorrection} onChange={onProfile} preparing={preparing}/>
    <label className="flex items-center gap-2 text-xs"><input type="checkbox" checked={settings.enabled} disabled={busy} onChange={e=>set("enabled",e.target.checked)}/>{t("advancedEditor.lens.enable")}</label>
    {settings.enabled&&<fieldset disabled={busy} className="space-y-2 disabled:opacity-40">
      {(["distortion","redCyan","blueYellow","purple","green","vignette","midpoint","horizontal","vertical","scale"] as const).map(key=><Slider key={key} label={t(`advancedEditor.lens.${key}`)} value={settings[key]} defaultValue={DEFAULT_LENS[key]} unit={key==="scale"?"%":undefined} min={key==="scale"?50:["purple","green","midpoint"].includes(key)?0:-100} max={key==="scale"?400:100} step={0.1} onChange={value=>set(key,value,false)} onSet={value=>set(key,value)} onReset={()=>set(key,DEFAULT_LENS[key])} begin={begin} end={end}/>)}
      <label className="flex items-center gap-2 text-xs"><input type="checkbox" checked={settings.extendEdges} onChange={e=>set("extendEdges",e.target.checked)}/>{t("advancedEditor.lens.extendEdges")}</label>
      <button type="button" className="rounded border border-edge p-2 text-xs" onClick={()=>void autoScale()}>{t("advancedEditor.lens.autoScale")}</button>
      <details className="editor-section space-y-2"><summary className="text-xs">{t("advancedEditor.lens.guides")}</summary>
        <div className="flex gap-2">{(["vertical","horizontal"] as const).map(key=><button key={key} type="button" className="flex-1 rounded border border-edge p-2 text-xs aria-pressed:text-accent" aria-pressed={axis===key} onClick={()=>onAxis(axis===key?null:key)}>{t(`advancedEditor.lens.${key}Guides`)}</button>)}</div>
        <p className="text-[11px] text-text-muted">{t("advancedEditor.lens.guideCount",{vertical:lines.filter(g=>g.axis==="vertical").length,horizontal:lines.filter(g=>g.axis==="horizontal").length})}</p>
        <button type="button" className="rounded border border-edge p-2 text-xs" disabled={lines.length<2} onClick={()=>{try{end();onChange(fitLensGuides(lines,settings,size,recipe));onLines([]);onAxis(null);setError(null);}catch(e){setError(String(e));}}}>{t("advancedEditor.lens.applyGuides")}</button>
        {settings.guide&&<button type="button" className="text-xs text-accent" onClick={()=>onChange({...settings,guide:undefined})}>{t("advancedEditor.lens.removeAppliedGuides")}</button>}
        <button type="button" className="ml-2 text-xs text-accent" onClick={()=>{onLines([]);onAxis(null);}}>{t("advancedEditor.lens.clearGuides")}</button>
      </details>
    </fieldset>}
    <label className="flex items-center gap-2 text-xs"><input type="checkbox" checked={grid} onChange={e=>onGrid(e.target.checked)}/>{t("advancedEditor.lens.grid")}</label>
    <button type="button" className="text-xs text-accent" disabled={!value||busy} onClick={()=>{end();onChange(undefined);onLines([]);onAxis(null);}}>{t("editor.reset")}</button>
    {error&&<p role="alert" className="text-xs text-red-400">{error}</p>}
  </section>;
}
