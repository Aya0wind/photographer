import {useEffect,useRef,useState} from "react";
import {editPreviewHistogram,type EditRecipe} from "@/ipc/api";
interface Input {recipe:EditRecipe;original:boolean}
export function startHistogramScheduler(sessionId:string,getInput:()=>Input,output:{result:(bins:number[][])=>void;busy:(busy:boolean)=>void;error:(error:string)=>void}){
 let closed=false,running=false,dirty=false,revision=0,changedAt=0;let timer:ReturnType<typeof setTimeout>|undefined;
 function schedule(){clearTimeout(timer);timer=setTimeout(()=>void pump(),Math.max(0,200-(Date.now()-changedAt)));}
 async function pump(){timer=undefined;if(closed||running||!dirty)return;dirty=false;running=true;const version=revision;const input=getInput();
  try{const bins=await editPreviewHistogram(sessionId,input.recipe,input.original);if(!closed&&version===revision)output.result(bins);}
  catch(error){if(!closed&&version===revision)output.error(error instanceof Error?error.message:String(error));}
  finally{running=false;if(!closed){if(dirty)schedule();else output.busy(false);}}
 }
 return {wake(){if(closed)return;dirty=true;revision++;changedAt=Date.now();output.busy(true);schedule();},cancel(){revision++;dirty=false;clearTimeout(timer);output.busy(false);},close(){closed=true;clearTimeout(timer);}};
}
export function useEditorHistogram(sessionId:string|undefined,recipe:EditRecipe,original:boolean,enabled:boolean){
 const [bins,setBins]=useState<number[][]>([]),[busy,setBusy]=useState(false),[error,setError]=useState<string|null>(null);
 const latest=useRef({recipe,original});latest.current={recipe,original};const scheduler=useRef<ReturnType<typeof startHistogramScheduler>|null>(null);
 const signature=JSON.stringify({recipe:{...recipe,output:undefined,textLayers:[],brushStrokes:[]},original});
 useEffect(()=>{setBins([]);setError(null);if(!sessionId){setBusy(false);return;}
  const worker=startHistogramScheduler(sessionId,()=>latest.current,{result:value=>{setBins(value);setError(null);},busy:setBusy,error:setError});scheduler.current=worker;
  return()=>{worker.close();scheduler.current=null;};
 },[sessionId]);
 useEffect(()=>{if(enabled)scheduler.current?.wake();else{scheduler.current?.cancel();setBins(current=>current.length?[]:current);setError(null);}},[signature,sessionId,enabled]);
 return {bins,busy,error,closeError:()=>setError(null)};
}
