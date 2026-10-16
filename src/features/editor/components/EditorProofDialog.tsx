import {useEffect,useRef,useState} from "react";
import {useTranslation} from "react-i18next";
import {open} from "@tauri-apps/plugin-dialog";
import {editPreviewProof,editVersionCreate,type ProofOptions,type EditRecipe} from "@/ipc/api";
const INITIAL:ProofOptions={profile:"srgb",path:null,intent:"relative",bpc:true,simulatePaper:false,gamut:false};
export default function EditorProofDialog({sessionId,assetId,libraryId,recipe,original,onClose}:{sessionId:string;assetId:number;libraryId:string;recipe:EditRecipe;original:string|null;onClose:()=>void}){
 const {t}=useTranslation();const [options,setOptions]=useState(INITIAL),[url,setUrl]=useState<string|null>(null),[busy,setBusy]=useState(false),[error,setError]=useState<string|null>(null),[notice,setNotice]=useState<string|null>(null),[copying,setCopying]=useState(false);
 const owned=useRef<string|null>(null);const signature=JSON.stringify(options);
 useEffect(()=>{const close=(event:KeyboardEvent)=>{if(event.key==="Escape"){event.preventDefault();event.stopImmediatePropagation();onClose();}};window.addEventListener("keydown",close,true);return()=>window.removeEventListener("keydown",close,true);},[onClose]);
 useEffect(()=>{
  let live=true;setBusy(true);setError(null);
  const timer=setTimeout(()=>{void editPreviewProof(sessionId,recipe,options).then(value=>{if(!live){URL.revokeObjectURL(value);return;}const old=owned.current;owned.current=value;setUrl(value);if(old)URL.revokeObjectURL(old);}).catch(e=>{if(live)setError(String(e));}).finally(()=>{if(live)setBusy(false);});},200);
  return()=>{live=false;clearTimeout(timer);};
 },[sessionId,signature,recipe]);
 useEffect(()=>()=>{if(owned.current)URL.revokeObjectURL(owned.current);},[]);
 async function choose(){try{const path=await open({multiple:false,directory:false,filters:[{name:"ICC",extensions:["icc","icm"]}]});if(typeof path==="string")setOptions({...options,profile:"custom",path});}catch(e){setError(String(e));}}
 async function copy(){setCopying(true);try{await editVersionCreate(assetId,libraryId,"copy",t("advancedEditor.proof.copyName",{name:options.path?.split(/[\\/]/).pop()??options.profile}),recipe);setNotice(t("advancedEditor.proof.created"));}catch(e){setError(String(e));}finally{setCopying(false);}}
 return <div className="fixed inset-0 z-[85] flex flex-col bg-bg p-4" role="dialog" aria-modal="true" aria-label={t("advancedEditor.proof.title")}>
  <header className="flex flex-wrap items-center gap-2"><h2 className="mr-auto text-sm font-semibold">{t("advancedEditor.proof.title")}</h2><button className="rounded border border-edge px-3 py-2 text-xs" disabled={copying} onClick={()=>void copy()}>{t("advancedEditor.proof.createCopy")}</button><button className="rounded border border-edge px-3 py-2 text-xs" onClick={onClose}>{t("common.close")}</button></header>
  <div className="my-3 flex flex-wrap items-center gap-3 text-xs">
   <select aria-label={t("advancedEditor.proof.profile")} className="rounded border border-edge bg-panel p-2" value={options.profile} onChange={e=>setOptions({...options,profile:e.target.value as ProofOptions["profile"]})}>{["srgb","displayP3","adobeRgb","proPhotoRgb",...(options.path?["custom"]:[])].map(value=><option key={value} value={value}>{value==="custom"?options.path?.split(/[\\/]/).pop():value}</option>)}</select><button type="button" className="rounded border border-edge p-2" onClick={()=>void choose()}>{t("advancedEditor.proof.chooseIcc")}</button>
   <select aria-label={t("advancedEditor.proof.intent")} className="rounded border border-edge bg-panel p-2" value={options.intent} onChange={e=>setOptions({...options,intent:e.target.value as ProofOptions["intent"]})}>{["relative","perceptual","absolute","saturation"].map(value=><option key={value} value={value}>{t(`advancedEditor.proof.${value}`)}</option>)}</select>
   {(["bpc","simulatePaper","gamut"] as const).map(key=><label key={key} className="flex items-center gap-1"><input type="checkbox" checked={options[key]} onChange={e=>setOptions({...options,[key]:e.target.checked})}/>{t(`advancedEditor.proof.${key}`)}</label>)}
  </div>
  <p className="mb-3 text-[11px] text-text-muted">{t("advancedEditor.proof.hint")}</p>
  <div className="grid min-h-0 flex-1 grid-cols-2 gap-3">{[[original,"before"],[url,"after"]].map(([src,label])=><figure key={label} className="flex min-h-0 flex-col overflow-hidden rounded bg-[#202020]"><figcaption className="p-2 text-center text-[11px] text-white/70">{t(`advancedEditor.proof.${label}`)}</figcaption>{src&&<img src={src} alt={t(`advancedEditor.proof.${label}`)} className="min-h-0 flex-1 object-contain"/>}</figure>)}</div>
  {busy&&<p role="status" className="pt-2 text-xs">{t("advancedEditor.previewUpdating")}</p>}{error&&<p role="alert" className="pt-2 text-xs text-red-400">{error}</p>}{notice&&<p role="status" className="pt-2 text-xs">{notice}</p>}
 </div>;
}
