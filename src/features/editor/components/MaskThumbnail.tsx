import {useEffect,useState} from "react";
import {editPreviewMask,type EditLocalMask,type EditRecipe} from "@/ipc/api";
export default function MaskThumbnail({sessionId,recipe,mask}:{sessionId?:string;recipe:EditRecipe;mask:EditLocalMask}){
 const [url,setUrl]=useState<string|null>(null);
 const shape=JSON.stringify({...mask,advanced:undefined,adjustments:undefined,name:undefined,overlayColor:undefined});
 useEffect(()=>{
  if(!sessionId)return;let live=true,owned:string|undefined;
  const timer=setTimeout(()=>{void editPreviewMask(sessionId,{...recipe,masks:[mask]},mask.id,true).then(value=>{owned=value;if(live)setUrl(value);else URL.revokeObjectURL(value);}).catch(()=>{if(live)setUrl(null);});},150);
  return()=>{live=false;clearTimeout(timer);if(owned)URL.revokeObjectURL(owned);};
 },[sessionId,shape]);
 return <span aria-hidden="true" className="block h-10 w-10 shrink-0 overflow-hidden rounded bg-black">{url&&<img src={url} alt="" className="h-full w-full object-contain"/>}</span>;
}
