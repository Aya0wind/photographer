import {useCallback,useEffect,useRef,useState} from "react";
import {editPreviewPick,type EditRecipe} from "@/ipc/api";

type Triple=[number,number,number];
/** sRGB display values to CIE Lab, D65 white. */
export function rgbToLab(rgb:Triple):Triple {
  const [r,g,b]=rgb.map(v=>{const c=v/255;return c<=.04045?c/12.92:((c+.055)/1.055)**2.4;});
  const f=(v:number)=>v>216/24389?Math.cbrt(v):v*841/108+4/29;
  const x=f((r*.4124564+g*.3575761+b*.1804375)/.95047);
  const y=f(r*.2126729+g*.7151522+b*.072175);
  const z=f((r*.0193339+g*.119192+b*.9503041)/1.08883);
  return [116*y-16,500*(x-y),200*(y-z)];
}

export function usePixelReadout(sessionId:string|undefined,recipe:EditRecipe,enabled:boolean){
  const [rgb,setRgb]=useState<Triple|null>(null);
  const point=useRef<[number,number]|null>(null);
  const worker=useRef<(()=>void)|null>(null);
  const latest=useRef({recipe,enabled});latest.current={recipe,enabled};
  const revision=useRef(0);
  const signature=JSON.stringify(recipe);
  useEffect(()=>{
    setRgb(null);point.current=null;revision.current++;
    if(!sessionId)return;
    let disposed=false,running=false,dirty=false;
    let timer:ReturnType<typeof setTimeout>|undefined;
    const wake=()=>{dirty=true;if(!running&&timer===undefined)timer=setTimeout(()=>{timer=undefined;void pump();},80);};
    async function pump(){
      if(disposed||!point.current||!latest.current.enabled)return;
      dirty=false;running=true;const version=revision.current;const [x,y]=point.current;
      try{const result=await editPreviewPick(sessionId!,latest.current.recipe,x,y,"readout");if(!disposed&&version===revision.current)setRgb(result.sample??null);}
      catch{if(!disposed&&version===revision.current)setRgb(null);}
      finally{running=false;if(!disposed&&dirty)wake();}
    }
    worker.current=wake;
    return()=>{disposed=true;clearTimeout(timer);worker.current=null;};
  },[sessionId]);
  useEffect(()=>{revision.current++;if(enabled&&point.current)worker.current?.();else if(!enabled)setRgb(null);},[signature,enabled]);
  const hover=useCallback((value:[number,number]|null)=>{point.current=value;revision.current++;if(value)worker.current?.();else setRgb(null);},[]);
  return {rgb,lab:rgb?rgbToLab(rgb):null,hover};
}
