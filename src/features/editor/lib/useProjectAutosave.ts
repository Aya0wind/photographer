import {useCallback,useEffect,useRef,useState} from "react";
import {editProjectSave,type EditRecipe} from "@/ipc/api";

/** All writes, including explicit Save, share one queue to prevent stale overwrites. */
export function useProjectAutosave({assetId,libraryId,recipe,enabled,dirty,onSaved}:{
  assetId:number;libraryId:string;recipe:EditRecipe;enabled:boolean;dirty:boolean;
  onSaved:(snapshot:EditRecipe)=>void;
}) {
  const [status,setStatus]=useState<"idle"|"saving"|"error">("idle");
  const [error,setError]=useState<string|null>(null);
  const identity=`${libraryId}:${assetId}`;
  const latest=useRef({identity,onSaved});latest.current={identity,onSaved};
  const live=useRef(true);
  const tail=useRef<Promise<void>>(Promise.resolve());
  const timer=useRef<ReturnType<typeof setTimeout>|null>(null);
  const signature=JSON.stringify(recipe);
  useEffect(()=>{live.current=true;return()=>{live.current=false;};},[]);
  useEffect(()=>{setStatus("idle");setError(null);},[identity]);
  const cancelPending=useCallback(()=>{if(timer.current!==null){clearTimeout(timer.current);timer.current=null;}},[]);
  const save=useCallback(async(snapshot:EditRecipe):Promise<boolean>=>{
    cancelPending();
    const current=()=>live.current&&latest.current.identity===identity;
    const task=tail.current.catch(()=>{}).then(async()=>{
      if(current()){setStatus("saving");setError(null);}
      await editProjectSave(assetId,libraryId,snapshot);
      if(current()){latest.current.onSaved(snapshot);setStatus("idle");}
    });
    tail.current=task;
    try {await task;return true;}
    catch(reason){if(current()){setError(String(reason));setStatus("error");}return false;}
  },[assetId,libraryId,identity,cancelPending]);
  useEffect(()=>{
    cancelPending();
    if(enabled&&dirty)timer.current=setTimeout(()=>{timer.current=null;void save(recipe);},900);
    return cancelPending;
  // The serialized recipe avoids restarting the debounce for render-only changes.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  },[enabled,dirty,signature,save,cancelPending]);
  return {status,error,save,cancelPending};
}
