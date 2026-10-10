import {useEffect,useRef,useState} from "react";
import type {EditRecipe} from "@/ipc/api";
import {editPreviewMask} from "@/ipc/api";

/** Shape changes request an overlay; color sliders never re-encode it. */
export function useMaskOverlay(sessionId:string|undefined,recipe:EditRecipe,id:string|null,enabled:boolean,onError:(error:unknown)=>void) {
 const [url,setUrl]=useState<string|null>(null);
 const current=useRef<string|null>(null),owned=useRef(new Set<string>()),error=useRef(onError);error.current=onError;
 const mask=recipe.masks?.find(m=>m.id===id);
 const signature=JSON.stringify(mask?{id:mask.id,kind:mask.kind,from:mask.from,to:mask.to,strokes:mask.strokes,inverted:mask.inverted,feather:mask.feather,quarter:recipe.rotateQuarter,geometry:recipe.geometry,crop:recipe.crop}:null);
 const latest=useRef(recipe);latest.current=recipe;
 useEffect(()=>{
  let cancelled=false;
  if(!enabled||!sessionId||!id||signature==="null"){setUrl(null);return;}
  void editPreviewMask(sessionId,latest.current,id).then(async next=>{
   owned.current.add(next);
   try {const image=new Image();image.src=next;if(typeof image.decode==="function")await image.decode();}
   catch(e){owned.current.delete(next);URL.revokeObjectURL(next);throw e;}
   if(cancelled){owned.current.delete(next);URL.revokeObjectURL(next);return;}
   const previous=current.current;current.current=next;setUrl(next);
   if(previous){owned.current.delete(previous);URL.revokeObjectURL(previous);}
  }).catch(e=>{if(!cancelled)error.current(e);});
  return()=>{cancelled=true;};
 },[sessionId,id,enabled,signature]);
 useEffect(()=>()=>{for(const value of owned.current)URL.revokeObjectURL(value);owned.current.clear();current.current=null;},[]);
 return url;
}
