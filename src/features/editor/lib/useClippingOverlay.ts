import {useEffect,useRef,useState} from "react";
import {editPreviewClipping,type EditRecipe} from "@/ipc/api";
/** The native canvas needs no PNG; this is only for ordinary preview displays. */
export function useClippingOverlay(sessionId:string|undefined,recipe:EditRecipe,original:boolean,warnings:number,enabled:boolean,onError:(error:unknown)=>void){
 const [url,setUrl]=useState<string|null>(null),current=useRef<string|null>(null),owned=useRef(new Set<string>());
 const latest=useRef({recipe,onError});latest.current={recipe,onError};
 const signature=JSON.stringify(original?{rotateQuarter:recipe.rotateQuarter,geometry:recipe.geometry,crop:recipe.crop}:recipe);
 useEffect(()=>{
  let cancelled=false;setUrl(null);
  if(!enabled||!sessionId||!warnings)return;
  const timer=setTimeout(()=>{void editPreviewClipping(sessionId,latest.current.recipe,original,warnings).then(async next=>{
   owned.current.add(next);try{const image=new Image();image.src=next;if(typeof image.decode==="function")await image.decode();}catch(e){owned.current.delete(next);URL.revokeObjectURL(next);throw e;}
   if(cancelled){owned.current.delete(next);URL.revokeObjectURL(next);return;}
   const old=current.current;current.current=next;setUrl(next);if(old){owned.current.delete(old);URL.revokeObjectURL(old);}
  }).catch(error=>{if(!cancelled)latest.current.onError(error);});},48);
  return()=>{cancelled=true;clearTimeout(timer);};
 },[sessionId,signature,original,warnings,enabled]);
 useEffect(()=>()=>{for(const value of owned.current)URL.revokeObjectURL(value);owned.current.clear();current.current=null;},[]);
 return url;
}
