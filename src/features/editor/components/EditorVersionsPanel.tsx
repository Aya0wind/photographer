import {useEffect,useState} from "react";
import {useTranslation} from "react-i18next";
import {editVersionsList,editVersionCreate,editVersionApply,editVersionDelete,type EditorEdition,type EditRecipe} from "@/ipc/api";

export default function EditorVersionsPanel({assetId,libraryId,recipe,disabled,revision,save,onApply,onBusy}:{
 assetId:number;libraryId:string;recipe:EditRecipe;disabled:boolean;revision:EditRecipe;
 onBusy:(value:boolean)=>void;save:()=>Promise<boolean>;onApply:(recipe:EditRecipe,branch:boolean)=>void;
}) {
 const {t}=useTranslation();const [items,setItems]=useState<EditorEdition[]>([]),[name,setName]=useState(""),[busy,setBusy]=useState(false),[error,setError]=useState<string|null>(null);
 async function refresh(){setItems(await editVersionsList(assetId,libraryId));}
 useEffect(()=>{let live=true;void editVersionsList(assetId,libraryId).then(rows=>{if(live)setItems(rows);}).catch(e=>{if(live)setError(String(e));});return()=>{live=false;};},[assetId,libraryId,revision]);
 async function run(action:()=>Promise<void>,persist=true){if(busy)return;setBusy(true);setError(null);try{if(persist&&!await save())return;onBusy(true);await action();await refresh();}catch(e){setError(String(e));}finally{onBusy(false);setBusy(false);}}
 async function create(kind:"snapshot"|"copy"){
  if(!name.trim())return;
  await run(async()=>{await editVersionCreate(assetId,libraryId,kind,name.trim(),recipe);setName("");});
 }
 return <section className="space-y-3">

  <fieldset disabled={disabled||busy} className="space-y-2">
   <input aria-label={t("advancedEditor.versions.name")} value={name} maxLength={120} onChange={e=>setName(e.target.value)} className="w-full rounded border border-edge bg-bg p-2 text-xs" placeholder={t("advancedEditor.versions.name")}/>
   <div className="flex gap-1">{(["snapshot","copy"] as const).map(kind=><button key={kind} type="button" disabled={!name.trim()} onClick={()=>void create(kind)} className="flex-1 rounded border border-edge p-2 text-xs disabled:opacity-40">＋ {t(`advancedEditor.versions.${kind}`)}</button>)}</div>
   {(["original","copy","snapshot","history"] as const).map(kind=><details key={kind} open={kind!=="history"} className="space-y-2 border-t border-edge pt-2"><summary className="text-xs">{t(`advancedEditor.versions.${kind}`)} ({items.filter(item=>item.kind===kind).length})</summary>
    {items.filter(item=>item.kind===kind).map(item=><div key={item.id} className="flex items-center gap-1">
     <button type="button" aria-pressed={item.active} disabled={item.active} title={new Date(item.updatedAt).toLocaleString()} className="min-w-0 flex-1 truncate rounded border border-edge p-2 text-left text-[11px] aria-pressed:border-accent" onClick={()=>void run(async()=>{const value=await editVersionApply(assetId,libraryId,item.id);onApply(value,item.kind==="copy"||item.kind==="original");})}>{item.kind==="original"?t("advancedEditor.versions.original"):item.name||new Date(item.updatedAt).toLocaleString()}</button>
     {!item.active&&item.kind!=="original"&&<button type="button" aria-label={t("advancedEditor.versions.delete",{name:item.name})} className="rounded p-1 text-text-muted hover:text-red-400" onClick={()=>void run(()=>editVersionDelete(assetId,libraryId,item.id),false)}>×</button>}
    </div>)}
   </details>)}
  </fieldset>
  {error&&<p role="alert" className="text-xs text-red-400">{error}</p>}
 </section>;
}
