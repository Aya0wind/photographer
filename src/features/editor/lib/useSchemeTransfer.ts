import {useEffect,useRef,useState} from "react";
import {open,save} from "@tauri-apps/plugin-dialog";
import {editSchemeOpen,editSchemeSave,type EditRecipe} from "@/ipc/api";
interface Props {assetId:number;libraryId:string;name:string;recipe:EditRecipe;label:string;disabled:boolean;onApply:(recipe:EditRecipe)=>void;onSaved?:()=>void;onBusy:(busy:boolean)=>void}
export function useSchemeTransfer(props:Props){
 const latest=useRef(props);latest.current=props;
 const [busy,setBusy]=useState(false),[error,setError]=useState<string|null>(null);
 const sequence=useRef(0),locked=useRef(false);
 useEffect(()=>{sequence.current++;locked.current=false;setBusy(false);setError(null);return()=>{sequence.current++;if(locked.current){locked.current=false;latest.current.onBusy(false);}};},[props.assetId,props.libraryId]);
 async function transfer(importing:boolean){
  const p=latest.current;if(locked.current||p.disabled)return;
  const token=++sequence.current;locked.current=true;p.onBusy(true);setBusy(true);setError(null);
  const current=()=>token===sequence.current;
  try{
   const filters=[{name:p.label,extensions:["photoedit"]}];
   const path=importing?await open({multiple:false,directory:false,filters}):await save({defaultPath:`${p.name.replace(/\.[^.]+$/,"").replace(/[<>:"/\\|?*]/g,"_")}.photoedit`,filters});
   if(!current()||typeof path!=="string")return;
   if(importing){const recipe=await editSchemeOpen(p.assetId,p.libraryId,path);if(current()){window.dispatchEvent(new Event("editor-luts-changed"));latest.current.onApply(recipe);}}
   else {await editSchemeSave(p.assetId,p.libraryId,p.recipe,path.toLowerCase().endsWith(".photoedit")?path:`${path}.photoedit`);if(current())latest.current.onSaved?.();}
  }catch(e){if(current())setError(e instanceof Error?e.message:String(e));}
  finally{if(current()){locked.current=false;latest.current.onBusy(false);setBusy(false);}}
 }
 return {busy,error,closeError:()=>setError(null),save:()=>transfer(false),open:()=>transfer(true)};
}
