import {useEffect,useRef,useState} from "react";
import {useTranslation} from "react-i18next";
import {appForceExit,editDetailOpen,editDetailStatus,editDetailView,editDetailImage,editDetailClose,type EditRecipe,type EditorDetailStatus} from "@/ipc/api";
const BUTTON="rounded border border-edge px-3 py-2 text-xs disabled:opacity-30";
export default function EditorDetailDialog({assetId,libraryId,recipe,preview,initialZoom=1,onClose}:{assetId:number;libraryId:string;recipe:EditRecipe;preview:string|null;initialZoom?:number;onClose:()=>void}){
  const {t}=useTranslation();const [status,setStatus]=useState<EditorDetailStatus|null>(null),[error,setError]=useState<string|null>(null),[src,setSrc]=useState<string|null>(null),[retry,setRetry]=useState(0);
  const [center,setCenter]=useState({x:0.5,y:0.5}),[edges,setEdges]=useState(false),[zoom,setZoom]=useState(initialZoom),[size,setSize]=useState({width:512,height:512}),[confirmExit,setConfirmExit]=useState(false);
  const owned=useRef<string|null>(null);
  const signature=JSON.stringify(recipe);
  useEffect(()=>{
    let live=true;let timer:ReturnType<typeof setTimeout>|undefined;let id:string|null=null;let loaded=0;
    setStatus(null);setError(null);
    async function poll(){
      if(!id||!live)return;
      try{
        const next=await editDetailStatus(id);if(!live)return;setStatus(next);
        if(next.error){setError(next.error);return;}if(next.cancelled)return;
        if(next.revision>loaded){
          try{
            const url=await editDetailImage(id,next.revision);
            if(!live){URL.revokeObjectURL(url);return;}
            if(owned.current)URL.revokeObjectURL(owned.current);owned.current=url;setSrc(url);loaded=next.revision;
          }catch(e){if(!String(e).includes("过期"))throw e;}
        }
      }catch(e){if(live)setError(String(e));}
      if(live)timer=setTimeout(()=>void poll(),250);
    }
    void editDetailOpen(assetId,libraryId,JSON.parse(signature)).then(value=>{
      id=value.id;if(!live){void editDetailClose(value.id).catch(()=>{});return;}
      setStatus(value);void poll();
    }).catch(e=>{if(live)setError(String(e));});
    return()=>{live=false;clearTimeout(timer);if(id)void editDetailClose(id).catch(()=>{});};
  },[assetId,libraryId,signature,retry]);
  useEffect(()=>{
    if(!status?.width||!status.height||status.error||status.cancelled)return;
    let live=true;
    const timer=setTimeout(()=>{void editDetailView(status.id,center.x,center.y,512,edges).catch(e=>{if(live)setError(String(e));});},150);
    return()=>{live=false;clearTimeout(timer);};
  },[status?.id,status?.width,status?.height,center.x,center.y,edges]);
  useEffect(()=>{const key=(event:KeyboardEvent)=>{if(event.key==="Escape"){event.preventDefault();event.stopImmediatePropagation();onClose();}};window.addEventListener("keydown",key,true);return()=>window.removeEventListener("keydown",key,true);},[onClose]);
  useEffect(()=>()=>{if(owned.current)URL.revokeObjectURL(owned.current);},[]);
  const ratio=window.devicePixelRatio||1;
  return <div className="fixed inset-0 z-[85] flex flex-col bg-bg p-4" role="dialog" aria-modal="true" aria-label={t("advancedEditor.detailInspector.title")}>
    <header className="flex flex-wrap items-center gap-3"><h2 className="mr-auto text-sm font-semibold">{t("advancedEditor.detailInspector.title")}</h2><select className="rounded border border-edge bg-panel p-2 text-xs" aria-label={t("editor.zoomIn")} value={zoom} onChange={e=>setZoom(Number(e.target.value))}>{[1,2,4].map(value=><option key={value} value={value}>{value*100}%</option>)}</select><label className="flex items-center gap-2 text-xs"><input type="checkbox" checked={edges} onChange={e=>setEdges(e.target.checked)}/>{t("advancedEditor.detailInspector.edges")}</label><button className={BUTTON} onClick={onClose}>{t("common.close")}</button></header>
    <p className="my-3 text-[11px] text-text-muted">{t("advancedEditor.detailInspector.hint")}</p>
    <div className="flex min-h-0 flex-1 flex-wrap gap-4"><aside className="w-60 space-y-3">
      <button className="relative block w-full overflow-hidden rounded bg-black/30" aria-label={t("advancedEditor.detailInspector.position")} onClick={e=>{const rect=e.currentTarget.getBoundingClientRect();setCenter({x:Math.max(0,Math.min(1,(e.clientX-rect.left)/rect.width)),y:Math.max(0,Math.min(1,(e.clientY-rect.top)/rect.height))});}}>{preview?<img src={preview} alt="" className="block h-auto w-full"/>:<span className="block h-40"/>}<span className="pointer-events-none absolute h-5 w-5 -translate-x-1/2 -translate-y-1/2 border border-white" style={{left:`${center.x*100}%`,top:`${center.y*100}%`}}/></button>
      {(["x","y"] as const).map(axis=><label key={axis} className="block text-xs">{axis.toUpperCase()} · {(center[axis]*100).toFixed(1)}%<input className="block w-full" type="range" min={0} max={100} step={0.1} value={center[axis]*100} onChange={e=>setCenter({...center,[axis]:Number(e.target.value)/100})}/></label>)}
      {status&&<p className="text-[11px] tabular-nums text-text-muted">{status.width} × {status.height}</p>}
      <button className={BUTTON} disabled={!error} onClick={()=>setRetry(v=>v+1)}>{t("advancedEditor.autosave.retry")}</button>
    </aside><div className="sp-scroll flex min-w-0 flex-1 items-center justify-center overflow-auto rounded bg-[#202020] p-4">{src&&<img src={src} alt={t("advancedEditor.detailInspector.title")} onLoad={e=>setSize({width:e.currentTarget.naturalWidth,height:e.currentTarget.naturalHeight})} style={{width:size.width*zoom/ratio,height:size.height*zoom/ratio,maxWidth:"none",imageRendering:"pixelated"}}/>}</div></div>
    {(!status||status.busy)&&!error&&<div role="status" className="mt-3 flex items-center gap-3 text-xs"><span className="h-3 w-3 animate-spin rounded-full border border-edge border-t-accent"/><span>{t("advancedEditor.detailInspector.loading")} · {t(status?.phase==="ready"?"advancedEditor.detailInspector.region":`editor.exportPhase.${status?.phase??"queued"}`,{defaultValue:status?.phase??"…"})}</span><progress className="ml-auto w-40"/></div>}
    {edges&&<p className="mt-2 text-[11px] text-text-muted">{t("advancedEditor.detailInspector.edgesHint")}</p>}{error&&<p role="alert" className="mt-2 text-xs text-red-400">{error}</p>}
    <footer className="mt-3 flex items-center justify-end gap-3"><button className={BUTTON} onClick={onClose}>{t("common.cancel")}</button><button className={BUTTON} onClick={()=>setConfirmExit(true)}>{t("editor.forceExit")}</button></footer>
    {confirmExit&&<div role="alertdialog" aria-label={t("editor.forceExit")} className="mt-2 flex items-center gap-3 text-xs"><span>{t("editor.forceExitWarning")}</span><button className={BUTTON} onClick={()=>setConfirmExit(false)}>{t("common.cancel")}</button><button className={BUTTON} onClick={()=>void appForceExit().catch(e=>setError(String(e)))}>{t("editor.forceExitConfirm")}</button></div>}
  </div>;
}
