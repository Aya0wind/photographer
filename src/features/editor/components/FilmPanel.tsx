import {useEffect,useState} from "react";
import {useTranslation} from "react-i18next";
import {open} from "@tauri-apps/plugin-dialog";
import {editFilmList,editFilmImport,editRawlabAvailable,type EditorFilmEntry,type EditRecipe} from "@/ipc/api";
import Slider from "./AdjustmentSlider";
type Pipeline=EditRecipe["colorPipeline"];
function FilmOptions({entries}:{entries:EditorFilmEntry[]}){
 const {t}=useTranslation();
 const groups=[{label:t("advancedEditor.film.builtin"),items:entries.filter(entry=>entry.builtin)},
  {label:t("advancedEditor.film.imported"),items:entries.filter(entry=>!entry.builtin)}];
 return <>{groups.filter(group=>group.items.length>0).map(group=><optgroup key={group.label} label={group.label}>
  {group.items.map(entry=><option key={entry.id} value={entry.id}>{[entry.name,entry.subtitle].filter(Boolean).join(" · ")}</option>)}
 </optgroup>)}</>;
}
export default function FilmPanel({value,onChange,begin,end}:{value:Pipeline;onChange:(value:Pipeline,record?:boolean)=>void;begin:()=>void;end:()=>void}){
 const {t}=useTranslation();const [available,setAvailable]=useState(false),[coreRevision,setCoreRevision]=useState(""),[entries,setEntries]=useState<EditorFilmEntry[]>([]),[busy,setBusy]=useState(false),[error,setError]=useState<string|null>(null);
 useEffect(()=>{let closed=false;void Promise.all([editRawlabAvailable(),editFilmList()]).then(([ok,list])=>{if(!closed){setAvailable(ok.available);setCoreRevision(ok.coreRevision);setEntries(list);}}).catch(e=>{if(!closed)setError(String(e));});return()=>{closed=true;};},[]);
 const film=value?.film;
 const selected=entries.find(entry=>entry.id===film?.id);
 async function importFilm(){setBusy(true);setError(null);try{
  const path=await open({multiple:false,directory:false,filters:[{name:"Film CUBE",extensions:["cube"]}]});if(typeof path!=="string")return;
  const entry=await editFilmImport(path);setEntries(await editFilmList());onChange({version:1,backend:"rawlab",coreRevision,film:{id:entry.id,amount:100,enabled:true}});
 }catch(e){setError(String(e));}finally{setBusy(false);}}
 return <details open className="space-y-3 border-b border-edge pb-3"><summary className="text-xs font-semibold">{t("advancedEditor.film.title")}</summary>
  <label className="flex items-center justify-between text-xs"><span>{t("advancedEditor.film.enable")}</span><input type="checkbox" className="accent-accent" checked={!!value} disabled={(!available&&!value)||busy} onChange={e=>onChange(e.target.checked?{version:1,backend:"rawlab",coreRevision}:undefined)}/></label>
  {!available&&<p className="text-xs text-text-muted">{t("advancedEditor.film.unavailable")}</p>}
  {(available||value)&&<><select aria-label={t("advancedEditor.film.select")} value={film?.id??""} disabled={!available||busy} className="w-full rounded-lg border border-edge bg-bg p-2 text-xs" onChange={e=>onChange({...value,version:1,backend:"rawlab",coreRevision:value?.coreRevision??coreRevision,film:e.target.value?{id:e.target.value,amount:film?.amount??100,enabled:true}:undefined})}>
   <option value="">{t("advancedEditor.film.neutral")}</option>{film&&!entries.some(e=>e.id===film.id)&&<option value={film.id}>{t("advancedEditor.lut.archived")}</option>}
   <FilmOptions entries={entries}/></select>
   {selected?.summary&&<p className="text-xs text-text-muted">{selected.summary}</p>}
   {film&&<><label className="flex items-center justify-between text-xs"><span>{t("advancedEditor.lut.enable")}</span><input type="checkbox" checked={film.enabled} onChange={e=>onChange({...value,film:{...film,enabled:e.target.checked}})}/></label><fieldset disabled={!film.enabled||busy}><Slider label={t("advancedEditor.lut.strength")} unit="%" value={film.amount} min={0} max={100} defaultValue={100} onChange={amount=>onChange({...value,film:{...film,amount}},false)} onSet={amount=>onChange({...value,film:{...film,amount}})} onReset={()=>onChange({...value,film:{...film,amount:100}})} begin={begin} end={end}/></fieldset></>}
  </>}
  <p className="text-xs text-text-muted">{t("advancedEditor.film.hint")}</p>
  <button type="button" disabled={!available||busy} className="rounded-lg border border-edge px-2 py-1.5 text-xs disabled:opacity-30" onClick={()=>void importFilm()}>{t("advancedEditor.film.import")}</button>
  {error&&<p role="alert" className="text-xs text-red-400">{error}</p>}
 </details>;
}
