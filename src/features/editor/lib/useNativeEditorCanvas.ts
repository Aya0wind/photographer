import { useEffect, useRef, useState, type RefObject } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ipc } from "@/ipc";
import type { EditRecipe } from "@/ipc/api";

interface FrameEvent { sessionId:string; revision?:number; ok:boolean; submitMs?:number; uploadedBytes?:number; readbackBytes?:number; error?:string }

/** Native compositor underneath a transparent WebView. The DOM remains the
 * input/overlay layer. Only view bounds and recipe parameters cross IPC. */
export function useNativeEditorCanvas(sessionId:string|undefined, recipe:EditRecipe,
  host:RefObject<HTMLElement|null>, enabled:boolean, original:boolean, sourceVersion:string|undefined) {
  const [active,setActive]=useState(false);
  const send=useRef<(()=>void)|null>(null);
  const latest=useRef({recipe,enabled,original});latest.current={recipe,enabled,original};
  const signature=JSON.stringify({recipe,enabled,original});
  useEffect(() => {
    if (!sessionId || !isTauri()) return;
    let disposed=false,failed=false,ready=false,revision=0,scheduled=0;
    let release:(()=>void)|undefined;
    let observer:ResizeObserver|undefined;
    let mutation:MutationObserver|undefined;
    setActive(false);
    function fail(error:unknown) {
      if(disposed) return;
      failed=true;setActive(false);document.documentElement.classList.remove("native-editor-canvas");
      void ipc("edit_native_hide",{sessionId}).catch(() => {});
      console.warn("[editor native] ordinary preview fallback",error);
    }
    async function submit() {
      scheduled=0;
      if(disposed || !ready || failed) return;
      const value=latest.current;
      const frame=host.current?.querySelector<HTMLElement>('[data-testid="editor-canvas-frame"]');
      if(!value.enabled || !frame || !host.current) {
        setActive(false);document.documentElement.classList.remove("native-editor-canvas");
        await ipc("edit_native_hide",{sessionId}).catch(() => {});return;
      }
      const picture=frame.getBoundingClientRect(),viewport=host.current.getBoundingClientRect();
      const x=Math.max(picture.left,viewport.left),y=Math.max(picture.top,viewport.top);
      const w=Math.min(picture.right,viewport.right)-x,h=Math.min(picture.bottom,viewport.bottom)-y;
      if(w<1 || h<1) {await ipc("edit_native_hide",{sessionId}).catch(() => {});return;}
      const scale=window.devicePixelRatio || 1;
      const view={rect:[Math.round(x*scale),Math.round(y*scale),Math.round(w*scale),Math.round(h*scale)],
        uv:[(x-picture.left)/picture.width,(y-picture.top)/picture.height,w/picture.width,h/picture.height]};
      try {await ipc("edit_native_frame",{sessionId,recipe:value.recipe,view,revision:++revision,original:value.original});}
      catch(error) {fail(error);}
    }
    function wake() {if(!scheduled && !disposed) scheduled=requestAnimationFrame(() => void submit());}
    send.current=wake;
    void (async () => {
      release=await listen<FrameEvent>("editor-native-frame",({payload}) => {
        if(disposed || payload.sessionId!==sessionId) return;
        if(!payload.ok) {fail(payload.error);return;}
        if(latest.current.enabled) {
          document.documentElement.classList.add("native-editor-canvas");setActive(true);
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
        mutation=new MutationObserver(observeFrame);mutation.observe(host.current,{childList:true,subtree:true});
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
  useEffect(() => {send.current?.();},[signature,sourceVersion]);
  return active && enabled;
}
