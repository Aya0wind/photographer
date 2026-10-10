import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ipc } from "@/ipc";
import type { EditRecipe } from "@/ipc/api";

interface FrameEvent { sessionId:string; revision?:number; ok:boolean; submitMs?:number; uploadedBytes?:number; readbackBytes?:number; error?:string }

/** Foreground, pointer-transparent native viewport. The DOM remains the
 * input/overlay layer. Only view bounds and recipe parameters cross IPC. */
export function useNativeEditorCanvas(sessionId:string|undefined, recipe:EditRecipe,
  host:RefObject<HTMLElement|null>, enabled:boolean, original:boolean, sourceVersion:string|undefined, overlayVersion?:string) {
  const [active,setActive]=useState(false);
  const revision=useRef(0);
  const send=useRef<(()=>void)|null>(null);
  const latest=useRef({recipe,enabled,original});latest.current={recipe,enabled,original};
  const signature=JSON.stringify({recipe,enabled,original,overlayVersion});
  useEffect(() => {
    if (!sessionId || !isTauri()) return;
    let disposed=false,failed=false,ready=false,scheduled=0;
    let release:(()=>void)|undefined;
    let observer:ResizeObserver|undefined;
    let mutation:MutationObserver|undefined;
    setActive(false);
    function fail(error:unknown) {
      if(disposed) return;
      failed=true;setActive(false);document.documentElement.classList.remove("native-editor-canvas");
      void ipc("edit_native_hide",{sessionId,revision:++revision.current}).catch(() => {});
      console.warn("[editor native] ordinary preview fallback",error);
    }
    async function submit() {
      scheduled=0;
      if(disposed || !ready || failed) return;
      const value=latest.current;
      const frame=host.current?.querySelector<HTMLElement>('[data-testid="editor-canvas-frame"]');
      if(!value.enabled || !frame || !host.current) {
        setActive(false);document.documentElement.classList.remove("native-editor-canvas");
        await ipc("edit_native_hide",{sessionId,revision:++revision.current}).catch(() => {});return;
      }
      const picture=frame.getBoundingClientRect(),viewport=host.current.getBoundingClientRect();
      if(viewport.width<1 || viewport.height<1 || picture.width<1 || picture.height<1) return;
      const scale=window.devicePixelRatio || 1;
      // Keep one fixed native surface. Zoom/pan change GPU coordinates only,
      // never resize/move a visible window containing an older frame.
      const overlays=Array.from(host.current.querySelectorAll<HTMLElement>("[data-native-overlay]")).filter(element=>element.dataset.visible!=="false").map(element=>{
        const bounds=element.getBoundingClientRect();return [Math.round((bounds.left-viewport.left)*scale),Math.round((bounds.top-viewport.top)*scale),Math.round(bounds.width*scale),Math.round(bounds.height*scale),Math.round(16*scale)];
      }).filter(rect=>rect[2]>0&&rect[3]>0);
      const cropGuide=frame.dataset.cropGuide?JSON.parse(frame.dataset.cropGuide) as number[]:null;
      const view={overlays,cropGuide,scale,rect:[Math.round(viewport.left*scale),Math.round(viewport.top*scale),Math.round(viewport.width*scale),Math.round(viewport.height*scale)],
        uv:[0,0,1,1],image:[(picture.left-viewport.left)/viewport.width,(picture.top-viewport.top)/viewport.height,picture.width/viewport.width,picture.height/viewport.height]};
      try {await ipc("edit_native_frame",{sessionId,recipe:value.recipe,view,revision:++revision.current,original:value.original});}
      catch(error) {fail(error);}
    }
    function wake() {if(!scheduled && !disposed) scheduled=requestAnimationFrame(() => void submit());}
    send.current=wake;
    void (async () => {
      release=await listen<FrameEvent>("editor-native-frame",({payload}) => {
        if(disposed || payload.sessionId!==sessionId) return;
        if(!payload.ok) {fail(payload.error);return;}
        if(latest.current.enabled) {
          setActive(true);
          try {performance.clearMeasures("editor.native.frame");performance.measure("editor.native.frame",{start:performance.now(),duration:payload.submitMs??0,detail:payload});} catch { /* diagnostics only */ }
        }
      });
      if(disposed) {release();return;}
      ready=await ipc<boolean>("edit_native_open",{sessionId});
      if(disposed) {await ipc("edit_native_close",{sessionId}).catch(() => {});return;}
      if(!ready) return;
      if(host.current) {
        observer=new ResizeObserver(wake);observer.observe(host.current);
        const observeFrame=() => {const frame=host.current?.querySelector('[data-testid="editor-canvas-frame"]');if(frame) observer?.observe(frame);wake();};
        mutation=new MutationObserver(observeFrame);mutation.observe(host.current,{childList:true,subtree:true,attributes:true,attributeFilter:["data-visible","data-crop-guide"]});
        observeFrame();
      }
      window.addEventListener("resize",wake);window.addEventListener("scroll",wake,true);
      wake();
    })().catch(fail);
    return () => {
      disposed=true;send.current=null;cancelAnimationFrame(scheduled);observer?.disconnect();mutation?.disconnect();release?.();
      window.removeEventListener("resize",wake);window.removeEventListener("scroll",wake,true);
      document.documentElement.classList.remove("native-editor-canvas");
      void ipc("edit_native_close",{sessionId}).catch(() => {});
    };
  },[sessionId,host]);
  useLayoutEffect(() => {
    if (!enabled && sessionId && isTauri()) {
      setActive(false);void ipc("edit_native_hide",{sessionId,revision:++revision.current}).catch(() => {});
    }
  },[enabled,sessionId]);
  useEffect(() => {send.current?.();},[signature,sourceVersion]);
  return active && enabled;
}
