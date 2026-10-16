import {useEffect,useMemo,useRef,useState} from "react";
import {useTranslation} from "react-i18next";
import {editVersionsList,editVersionRead,type EditorEdition,type EditRecipe,type AssetDto} from "@/ipc/api";
import {advancedRecipe} from "../lib/advancedRecipe";
import {useEditorPreview} from "../lib/useEditorPreview";
export default function EditorReferenceDialog({asset,libraryId,recipe,current,quick,onClose}:{asset:AssetDto;libraryId:string;recipe:EditRecipe;current:string|null;quick:string|null;onClose:()=>void}){
 const {t}=useTranslation();const [selected,setSelected]=useState("unedited"),[saved,setSaved]=useState<EditRecipe|null>(null),[versions,setVersions]=useState<EditorEdition[]>([]),[error,setError]=useState<string|null>(null),[reading,setReading]=useState(false),[zoom,setZoom]=useState(1);
 const left=useRef<HTMLDivElement>(null),right=useRef<HTMLDivElement>(null);
 useEffect(()=>{let live=true;void editVersionsList(asset.id,libraryId).then(rows=>{if(live)setVersions(rows);}).catch(e=>{if(live)setError(String(e));});return()=>{live=false;};},[asset.id,libraryId]);
 useEffect(()=>{
  let live=true;setError(null);setSaved(null);if(selected==="unedited"||selected==="quick")return;
  setReading(true);void editVersionRead(asset.id,libraryId,selected).then(value=>{if(live)setSaved(value);}).catch(e=>{if(live)setError(String(e));}).finally(()=>{if(live)setReading(false);});return()=>{live=false;};
 },[selected,asset.id,libraryId]);
 useEffect(()=>{const close=(event:KeyboardEvent)=>{if(event.key==="Escape"){event.preventDefault();event.stopImmediatePropagation();onClose();}};window.addEventListener("keydown",close,true);return()=>window.removeEventListener("keydown",close,true);},[onClose]);
 const reference=useMemo<EditRecipe>(()=>{
  const value=saved??{...advancedRecipe(),colorPipeline:asset.kind==="raw"&&recipe.colorPipeline?{...recipe.colorPipeline,film:undefined,rawWhiteBalance:undefined,relativeWhiteBalance:undefined}:undefined};
  return {...value,renderer:selected==="quick"?undefined:"photocraft",rotateQuarter:recipe.rotateQuarter,geometry:recipe.geometry,crop:recipe.crop,textLayers:[],brushStrokes:[]};
 },[saved,selected,recipe.rotateQuarter,recipe.geometry,recipe.crop,recipe.colorPipeline,asset.kind]);
 const preview=useEditorPreview(asset.id,reference,libraryId);
 function scroll(from:HTMLDivElement,to:HTMLDivElement|null){if(!to)return;to.scrollLeft=from.scrollLeft;to.scrollTop=from.scrollTop;}
 return <div className="fixed inset-0 z-[85] flex flex-col bg-bg p-4" role="dialog" aria-modal="true" aria-label={t("advancedEditor.reference.title")}>
  <header className="flex flex-wrap items-center gap-3"><h2 className="mr-auto text-sm font-semibold">{t("advancedEditor.reference.title")}</h2><select aria-label={t("advancedEditor.reference.source")} className="max-w-96 rounded border border-edge bg-panel p-2 text-xs" value={selected} onChange={e=>setSelected(e.target.value)}><option value="unedited">{t("advancedEditor.reference.unedited")}</option>{quick&&<option value="quick">{t("advancedEditor.reference.quick")}</option>}{versions.map(item=><option key={item.id} value={item.id}>{item.name||new Date(item.updatedAt).toLocaleString()}</option>)}</select>
   <select aria-label={t("editor.zoomIn")} value={zoom} onChange={e=>setZoom(Number(e.target.value))} className="rounded border border-edge bg-panel p-2 text-xs">{[1,2,4].map(value=><option key={value} value={value}>{value===1?t("editor.zoomFit"):`${value}×`}</option>)}</select><button type="button" className="rounded border border-edge p-2 text-xs" onClick={onClose}>{t("common.close")}</button>
  </header><p className="my-3 text-[11px] text-text-muted">{t(selected==="quick"?"advancedEditor.reference.quickHint":"advancedEditor.reference.hint")}</p>
  <div className="grid min-h-0 flex-1 grid-cols-2 gap-3">{[[selected==="quick"?quick:preview.url,"left"],[current,"right"]].map(([src,side])=><figure key={side} className="flex min-h-0 flex-col rounded bg-[#202020]"><figcaption className="p-2 text-center text-[11px] text-white/70">{t(`advancedEditor.reference.${side}`)}</figcaption><div ref={side==="left"?left:right} className="sp-scroll min-h-0 flex-1 overflow-auto" onScroll={e=>scroll(e.currentTarget,side==="left"?right.current:left.current)}>{src&&<div style={{width:`${zoom*100}%`,height:`${zoom*100}%`}}><img src={src} alt={t(`advancedEditor.reference.${side}`)} className="h-full w-full object-contain"/></div>}</div></figure>)}</div>
  {(reading||preview.loading||preview.pending||preview.preparingWhiteBalance)&&<p role="status" className="pt-2 text-xs">{t("advancedEditor.previewUpdating")}</p>}{(error||preview.error||preview.warning)&&<p role="alert" className="pt-2 text-xs text-red-400">{error||preview.error||preview.warning}</p>}
 </div>;
}
