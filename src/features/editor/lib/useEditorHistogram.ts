import {useEffect,useRef,useState} from "react";
import {editPreviewHistogram,type EditRecipe} from "@/ipc/api";
interface Input {recipe:EditRecipe;original:boolean}
// 拖动期节流间隔：连续 wake 只推迟到下个节流点，不会无限重置（2026-10-12
// 修复：原纯尾随防抖在持续拖动时永不触发，直方图要等松手才刷新）。
const HISTOGRAM_THROTTLE_MS=150;
export function startHistogramScheduler(sessionId:string,getInput:()=>Input,output:{result:(bins:number[][])=>void;busy:(busy:boolean)=>void;error:(error:string)=>void}){
 let closed=false,running=false,dirty=false,revision=0,lastPump=0;let timer:ReturnType<typeof setTimeout>|undefined;
 function schedule(){if(timer!==undefined)return;timer=setTimeout(()=>{timer=void 0;void pump();},Math.max(0,HISTOGRAM_THROTTLE_MS-(Date.now()-lastPump)));}
 async function pump(){timer=void 0;if(closed||running||!dirty)return;dirty=false;running=true;lastPump=Date.now();const version=revision;const input=getInput();
  try{const bins=await editPreviewHistogram(sessionId,input.recipe,input.original);if(!closed&&version===revision)output.result(bins);}
  catch(error){if(!closed&&version===revision)output.error(error instanceof Error?error.message:String(error));}
  finally{running=false;if(!closed){if(dirty)schedule();else output.busy(false);}}
 }
 return {wake(){if(closed)return;dirty=true;revision++;output.busy(true);schedule();},cancel(){revision++;dirty=false;clearTimeout(timer);timer=void 0;output.busy(false);},close(){closed=true;clearTimeout(timer);timer=void 0;}};
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
