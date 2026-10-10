import {createPortal} from "react-dom";
import Slider from "./AdjustmentSlider";
import {useEffect,useLayoutEffect,useState} from "react";
import {useTranslation} from "react-i18next";
import {open} from "@tauri-apps/plugin-dialog";
import {editLutList,editLutImport,editLutRename,editLutRemove,type EditorLutEntry,type AdvancedAdjustments} from "@/ipc/api";
import ErrorModal from "@/shared/components/ErrorModal";

type Lookup=NonNullable<AdvancedAdjustments["lookup"]>;
const BUTTON="rounded-lg border border-edge px-2 py-1.5 text-xs text-text-secondary hover:border-accent hover:text-accent disabled:opacity-30";
export default function LutPanel({value,onChange,begin,end,onModalChange}:{onModalChange?:(open:boolean)=>void;value:Lookup|undefined;onChange:(lookup:Lookup|undefined,record?:boolean)=>void;begin:()=>void;end:()=>void}) {
  const {t}=useTranslation();
  const [entries,setEntries]=useState<EditorLutEntry[]>([]);
  const [loading,setLoading]=useState(true),[busy,setBusy]=useState(false);
  const [error,setError]=useState<string|null>(null);
  const [editing,setEditing]=useState<EditorLutEntry|null>(null),[name,setName]=useState("");
  const [removing,setRemoving]=useState<EditorLutEntry|null>(null);
  useEffect(()=>{let cancelled=false;void editLutList().then(list=>{if(!cancelled)setEntries(list);})
    .catch(e=>{if(!cancelled)setError(String(e));}).finally(()=>{if(!cancelled)setLoading(false);});return()=>{cancelled=true;};},[]);
  useLayoutEffect(()=>{onModalChange?.(!!editing||!!removing||!!error);return()=>onModalChange?.(false);},[editing,removing,error,onModalChange]);
  useEffect(()=>{
    if(!editing&&!removing&&!error)return;
    const close=(event:KeyboardEvent)=>{if(event.key==="Escape"&&!busy){event.preventDefault();event.stopImmediatePropagation();setEditing(null);setRemoving(null);setError(null);}};
    window.addEventListener("keydown",close,true);return()=>window.removeEventListener("keydown",close,true);
  },[editing,removing,error,busy]);
  const current=entries.find(entry=>entry.id===value?.id);
  const entryName=(entry:EditorLutEntry)=>entry.builtin?t(`advancedEditor.lut.builtin.${entry.id.slice(8)}`,{defaultValue:entry.name}):entry.name;
  async function action(work:()=>Promise<void>) {if(busy)return;setBusy(true);try{await work();}catch(e){setError(e instanceof Error?e.message:String(e));}finally{setBusy(false);}}
  async function importLut() {
    await action(async()=>{
      const path=await open({multiple:false,directory:false,filters:[{name:"LUT",extensions:["cube","3dl","look"]}]});
      if(typeof path!=="string")return;
      const entry=await editLutImport(path);setEntries(await editLutList());
      onChange({id:entry.id,amount:100,enabled:true});
    });
  }
  return <details open className="space-y-3 border-t border-edge pt-3">
    <summary className="flex cursor-pointer items-center justify-between text-xs font-semibold">
      <span>{t("advancedEditor.lut.title")}</span>
      <button type="button" disabled={!value} className="rounded-md px-2 py-1 text-[11px] text-accent disabled:opacity-30"
        aria-label={t("advancedEditor.resetGroup",{group:t("advancedEditor.lut.title")})}
        onClick={event=>{event.preventDefault();event.stopPropagation();onChange(undefined);}}>{t("editor.reset")}</button>
    </summary>
    {loading?<p role="status" className="text-xs text-text-muted">{t("common.loading")}</p>:<>
      <select aria-label={t("advancedEditor.lut.select")} value={value?.id??""} disabled={busy}
        className="w-full rounded-lg border border-edge bg-bg p-2 text-xs"
        onChange={event=>onChange(event.target.value?{id:event.target.value,amount:value?.amount??100,enabled:true}:undefined)}>
        <option value="">{t("advancedEditor.lut.none")}</option>
        {value && !current && <option value={value.id}>{t("advancedEditor.lut.archived")}</option>}
        {entries.map(entry=><option key={entry.id} value={entry.id}>{entryName(entry)}</option>)}
      </select>
      <div className="flex flex-wrap gap-2">
        <button type="button" className={BUTTON} disabled={busy} onClick={()=>void importLut()}>{t("advancedEditor.lut.import")}</button>
        <button type="button" className={BUTTON} disabled={busy||!current||current.builtin} onClick={()=>{if(current){setEditing(current);setName(current.name);}}}>{t("advancedEditor.lut.rename")}</button>
        <button type="button" className={BUTTON} disabled={busy||!current||current.builtin} onClick={()=>current&&setRemoving(current)}>{t("advancedEditor.lut.remove")}</button>
      </div>
      {value && <>
        <label className="flex items-center justify-between text-xs text-text-secondary"><span>{t("advancedEditor.lut.enable")}</span>
          <input type="checkbox" className="accent-accent" checked={value.enabled} onChange={event=>onChange({...value,enabled:event.target.checked})}/></label>
        <fieldset disabled={!value.enabled||busy}>
          <Slider label={t("advancedEditor.lut.strength")} unit="%" value={value.amount} min={0} max={100} step={1} defaultValue={100}
            onChange={amount=>onChange({...value,amount},false)} onSet={amount=>onChange({...value,amount})}
            onReset={()=>onChange({...value,amount:100})} begin={begin} end={end}/>
        </fieldset>
      </>}
    </>}
    {busy && <div role="status" className="flex items-center gap-2 text-xs text-text-muted"><span className="h-3 w-3 animate-spin rounded-full border border-edge border-t-accent"/>{t("common.loading")}</div>}
    {(editing||removing) && createPortal(<div className="fixed inset-0 z-[90] flex items-center justify-center bg-black/65 p-4" role="dialog" aria-modal="true" aria-label={t(editing?"advancedEditor.lut.rename":"advancedEditor.lut.remove")}>
      <div className="ui-glass w-full max-w-sm space-y-4 rounded-2xl border border-edge p-5">
        <h3 className="text-sm font-semibold">{t(editing?"advancedEditor.lut.rename":"advancedEditor.lut.remove")}</h3>
        {editing?<input autoFocus aria-label={t("advancedEditor.lut.name")} className="w-full rounded-lg border border-edge bg-bg p-2 text-sm" maxLength={120} value={name} onChange={event=>setName(event.target.value)}/>:<p className="text-xs leading-relaxed text-text-secondary">{t("advancedEditor.lut.removeHint")}</p>}
        <div className="flex justify-end gap-2"><button type="button" className={BUTTON} disabled={busy} onClick={()=>{setEditing(null);setRemoving(null);}}>{t("common.cancel")}</button>
          <button type="button" className="ui-primary rounded-lg px-3 py-2 text-xs disabled:opacity-30" disabled={busy||(!!editing&&!name.trim())}
            onClick={()=>void action(async()=>{
              if(editing){await editLutRename(editing.id,name.trim());setEditing(null);}else if(removing){await editLutRemove(removing.id);setRemoving(null);}
              setEntries(await editLutList());
            })}>{t("common.confirm")}</button></div>
      </div>
    </div>,document.body)}
    <ErrorModal message={error} onClose={()=>setError(null)}/>
  </details>;
}
