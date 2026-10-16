import {useEffect,useMemo,useRef,useState} from "react";
import {useTranslation} from "react-i18next";
import {assetsByIds,editRawlabAvailable,editProjectOpen,editProjectSave,type AssetDto,type EditRecipe} from "@/ipc/api";
import {ipc} from "@/ipc";
import AssetThumb from "@/features/gallery/components/AssetThumb";
import {useAdvancedEditorStore} from "../lib/advancedEditorStore";
import {useEditorPreferences} from "../lib/editorPreferences";
import {advancedRecipe} from "../lib/advancedRecipe";
import {TRANSFER_GROUPS,transferRecipe,type TransferGroup} from "../lib/recipeTransfer";

const BUTTON="rounded border border-edge px-2 py-1 text-xs disabled:opacity-30";
export default function EditorFilmstrip({asset,recipe,disabled,save,onApply,savedRevision}:{asset:AssetDto;recipe:EditRecipe;savedRevision:EditRecipe;disabled:boolean;save:()=>Promise<boolean>;onApply:(value:EditRecipe)=>void}) {
 const {t}=useTranslation();const {preferences}=useEditorPreferences();const current=useAdvancedEditorStore(s=>s.current);
 const ids=useMemo(()=>Array.from(new Set(current?.assetIds?.length?current.assetIds:[asset.id])),[current?.assetIds,asset.id]);
 const position=ids.indexOf(asset.id);const [rows,setRows]=useState<AssetDto[]>([]),[selected,setSelected]=useState<number[]>([]),[expanded,setExpanded]=useState(true),[sync,setSync]=useState(false),[busy,setBusy]=useState(false),[message,setMessage]=useState<string|null>(null);
 const [automatic,setAutomatic]=useState(false);const automaticRevision=useRef(JSON.stringify(savedRevision));
 const [groups,setGroups]=useState<TransferGroup[]>(["light","color","curves","hsl","grading","detail","effects"]);
 const visible=ids.slice(Math.max(0,position-15),Math.max(0,position-15)+31);const signature=visible.join(",");
 useEffect(()=>{let live=true;void assetsByIds(visible,true).then(result=>{if(live){const map=new Map(result.map(row=>[row.id,row]));setRows(visible.flatMap(id=>{const row=map.get(id);return row?[row]:[];}));}}).catch(e=>{if(live)setMessage(String(e));});return()=>{live=false;};},[signature]);
 async function navigate(id:number){
  if(disabled||busy||!current||id===asset.id)return;setBusy(true);setMessage(null);
  try{if(!await save())return;const [next]=await assetsByIds([id],true);if(next?.libraryId&&(next.kind==="photo"||next.kind==="raw"))useAdvancedEditorStore.getState().request({...current,asset:next,libraryId:next.libraryId,assetIds:ids});}
  catch(e){setMessage(String(e));}finally{setBusy(false);}
 }
 async function mark(patch:Partial<AssetDto>){
  if(disabled||busy)return;setBusy(true);setMessage(null);
  try{if(patch.rating!==undefined)await ipc("asset_rating_set",{assetId:asset.id,rating:patch.rating});
   if(patch.flagged!==undefined)await ipc("asset_flag_set",{assetId:asset.id,flagged:patch.flagged});
   if(patch.rejected!==undefined)await ipc("asset_reject_set",{assetIds:[asset.id],rejected:patch.rejected});
   setRows(value=>value.map(row=>row.id===asset.id?{...row,...patch}:row));
  }catch(e){setMessage(String(e));}finally{setBusy(false);}
 }
 async function synchronize(persist=true){
  if(!selected.length||!groups.length||busy)return;setBusy(true);setMessage(null);
  let done=0;const failed:number[]=[];
  try{if(persist&&!await save())return;const targets=await assetsByIds(selected,true);
   for(const id of selected){const target=targets.find(row=>row.id===id);if(!target?.libraryId){failed.push(id);continue;}
    try{const saved=await editProjectOpen(id,target.libraryId);let base=saved??advancedRecipe();
     if(!saved&&target.kind==="raw"){const backend=await editRawlabAvailable();if(!backend.available)throw new Error(t("advancedEditor.baseColor.unavailable"));base={...base,colorPipeline:{version:2,backend:"rawlab",coreRevision:backend.coreRevision,baseProfile:preferences.defaultProfile}};}
     await editProjectSave(id,target.libraryId,transferRecipe(recipe,base,groups));done++;}catch{failed.push(id);}
   }
   automaticRevision.current=JSON.stringify(recipe);setMessage(t("advancedEditor.strip.syncResult",{done,failed:failed.length}));if(failed.length||!automatic)setSelected(failed);if(!failed.length&&!automatic)setSync(false);
  }catch(e){setMessage(String(e));}finally{setBusy(false);}
 }
 async function previousSettings(){
  const previous=ids[position-1];if(!previous||busy)return;setBusy(true);
  try{const [source]=await assetsByIds([previous],true);if(!source?.libraryId)return;const saved=await editProjectOpen(previous,source.libraryId);if(!saved)throw new Error(t("advancedEditor.strip.noPrevious"));onApply(transferRecipe(saved,recipe,groups));}catch(e){setMessage(String(e));}finally{setBusy(false);}
 }
 useEffect(()=>{
  const signature=JSON.stringify(savedRevision);
  if(!automatic){automaticRevision.current=signature;return;}
  if(disabled||busy||automaticRevision.current===signature)return;automaticRevision.current=signature;
  if(selected.length)void synchronize(false);
 },[savedRevision,automatic,disabled,busy]);
 function copySettings(){try{localStorage.setItem("photographer.editor.clipboard.v1",JSON.stringify({version:1,recipe,groups}));setMessage(t("advancedEditor.strip.copied"));}catch(e){setMessage(String(e));}}
 function pasteSettings(){try{const stored=localStorage.getItem("photographer.editor.clipboard.v1");if(!stored||stored.length>4*1024*1024)throw new Error(t("advancedEditor.strip.noClipboard"));const value=JSON.parse(stored);if(value.version!==1||value.recipe?.version!==1||value.recipe?.renderer!=="photocraft")throw new Error(t("advancedEditor.strip.noClipboard"));onApply(transferRecipe(value.recipe,recipe,groups));}catch(e){setMessage(String(e));}}
 const active=rows.find(row=>row.id===asset.id)??asset;
 return <section className="shrink-0 border-t border-edge bg-panel px-3 py-2" aria-label={t("advancedEditor.strip.title")}>
  <fieldset disabled={disabled||busy} className="flex flex-wrap items-center gap-2">
   <button type="button" className={BUTTON} aria-expanded={expanded} onClick={()=>setExpanded(value=>!value)}>{t("advancedEditor.strip.title")} {position+1}/{ids.length}</button>
   <button type="button" className={BUTTON} disabled={position<=0} onClick={()=>void navigate(ids[position-1])}>←</button><button type="button" className={BUTTON} disabled={position<0||position>=ids.length-1} onClick={()=>void navigate(ids[position+1])}>→</button>
   <select aria-label={t("advancedEditor.strip.rating")} className={`${BUTTON} bg-bg`} value={active.rating??0} onChange={e=>void mark({rating:Number(e.target.value)})}>{[0,1,2,3,4,5].map(value=><option key={value} value={value}>{value?"★".repeat(value):t("advancedEditor.strip.unrated")}</option>)}</select>
   <button type="button" className={BUTTON} aria-pressed={!!active.flagged} onClick={()=>void mark({flagged:!active.flagged})}>{t("advancedEditor.strip.flag")}</button>
   <button type="button" className={BUTTON} aria-pressed={!!active.rejected} onClick={()=>void mark({rejected:!active.rejected})}>{t("advancedEditor.strip.reject")}</button>
   <button type="button" className={BUTTON} onClick={()=>setSync(value=>!value)}>{t("advancedEditor.strip.sync")} ({selected.length})</button>
  </fieldset>
  {sync&&<fieldset disabled={disabled||busy} className="mt-2 flex flex-wrap gap-3 rounded border border-edge p-2 text-[11px]">
   {TRANSFER_GROUPS.map(group=><label key={group} className="flex items-center gap-1"><input type="checkbox" checked={groups.includes(group)} onChange={e=>setGroups(value=>e.target.checked?[...value,group]:value.filter(item=>item!==group))}/>{t(`advancedEditor.transfer.${group}`)}</label>)}
   <button type="button" className={BUTTON} disabled={!selected.length||!groups.length} onClick={()=>void synchronize()}>{t("advancedEditor.strip.applySelected")}</button>
   <label className="flex items-center gap-1"><input type="checkbox" checked={automatic} disabled={!selected.length} onChange={e=>setAutomatic(e.target.checked)}/>{t("advancedEditor.strip.autoSync")}</label>
   <button type="button" className={BUTTON} onClick={copySettings}>{t("advancedEditor.strip.copy")}</button><button type="button" className={BUTTON} onClick={pasteSettings}>{t("advancedEditor.strip.paste")}</button>
   <button type="button" className={BUTTON} disabled={position<=0||!groups.length} onClick={()=>void previousSettings()}>{t("advancedEditor.strip.previousSettings")}</button>
  </fieldset>}
  {expanded&&<div className="sp-scroll mt-2 flex gap-2 overflow-x-auto">{rows.map(row=><div key={row.id} className={`relative w-20 shrink-0 rounded border p-1 ${row.id===asset.id?"border-accent":"border-edge"}`}>
   <button type="button" disabled={disabled||busy} aria-label={row.name} aria-current={row.id===asset.id?"true":undefined} className="block h-14 w-full overflow-hidden rounded" onClick={()=>void navigate(row.id)}><AssetThumb asset={row} size={128} className="h-full w-full"/></button>
   <span className="block truncate text-[10px]" title={row.name}>{row.name}</span>
   {row.id!==asset.id&&<input type="checkbox" disabled={disabled||busy} aria-label={t("advancedEditor.strip.select",{name:row.name})} className="absolute right-1 top-1" checked={selected.includes(row.id)} onChange={e=>setSelected(value=>e.target.checked?[...value,row.id]:value.filter(id=>id!==row.id))}/>}
  </div>)}</div>}
  {message&&<p role="status" className="mt-1 text-xs text-amber-500">{message}</p>}
 </section>;
}
