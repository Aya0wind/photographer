import {rawSourceKey} from "./rawSourceKey";
import {useEffect,useRef,useState,type MutableRefObject} from "react";
import {editPreviewOpen,editPreviewClose,type EditRecipe,type EditorPreviewSession} from "@/ipc/api";
import {prepareFrame,type PreparedFrame} from "./useSmartPreview";
import {releasePreviewImage} from "./previewImages";

/** Prepare a replacement session offscreen; never clear the currently displayed frame. */
export function useRawSourcePreview({assetId,libraryId,session,sourceKey,latest,onPrepared}:{
 assetId:number;libraryId:string;session:EditorPreviewSession|null;sourceKey:string|null;latest:MutableRefObject<EditRecipe>;
 onPrepared:(value:PreparedFrame)=>void;
}){
 const desired=rawSourceKey(latest.current);
 const [busy,setBusy]=useState(false),[error,setError]=useState<string|null>(null);
 const callback=useRef(onPrepared);callback.current=onPrepared;
 useEffect(()=>{
  setError(null);
  if(!session?.sensorRaw||sourceKey===null||sourceKey===desired){setBusy(false);return;}
  let cancelled=false,adopted=false,id:string|null=null;setBusy(true);
  // Native decoding starts only after a pause; rapid source setting changes supersede it.
  const timer=setTimeout(()=>{void(async()=>{
   const candidate=await editPreviewOpen(assetId,libraryId,false);id=candidate.sessionId;
   const current=()=>!cancelled&&rawSourceKey(latest.current)===desired;
   if(!current())return;
   const prepared=await prepareFrame(id,latest,current,current,()=>{});
   if(!prepared)return;
   if(!current()){releasePreviewImage(prepared.frame);return;}
   adopted=true;callback.current(prepared);
  })().catch(e=>{if(!cancelled)setError(String(e));}).finally(()=>{if(!adopted&&id)void editPreviewClose(id).catch(()=>{});if(!cancelled)setBusy(false);});},450);
  return()=>{cancelled=true;clearTimeout(timer);if(!adopted&&id)void editPreviewClose(id).catch(()=>{});};
 },[assetId,libraryId,session?.sessionId,sourceKey,desired,latest]);
 return {busy,error};
}
