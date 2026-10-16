import {useEffect,useRef,useState} from "react";
import {useTranslation} from "react-i18next";
import {editPreviewStyle,type EditRecipe} from "@/ipc/api";
import {queueStylePreview} from "../lib/stylePreviewQueue";
export interface StylePreviewContext {sessionId:string;recipe:EditRecipe}
export interface StyleEntry {id:string;name:string;builtin?:boolean;subtitle?:string}
function favorites(key:string):string[]{try{const rows:unknown=JSON.parse(localStorage.getItem(key)??"[]");return Array.isArray(rows)?rows.filter((v):v is string=>typeof v==="string").slice(0,200):[];}catch{return [];}}
function Thumbnail({context,id,kind}:{context:StylePreviewContext;id:string;kind:"film"|"lut"}){
  const host=useRef<HTMLDivElement>(null),url=useRef<string|null>(null);
  const [visible,setVisible]=useState(false),[error,setError]=useState<string|null>(null),[src,setSrc]=useState<string|null>(null);
  const signature=JSON.stringify(context.recipe);
  useEffect(()=>{
    if(!host.current)return;
    const observer=new IntersectionObserver(rows=>setVisible(rows.some(row=>row.isIntersecting)));
    observer.observe(host.current);return()=>observer.disconnect();
  },[]);
  useEffect(()=>{
    if(!visible)return;
    let live=true;setError(null);
    const timer=setTimeout(()=>queueStylePreview({current:()=>live,run:async()=>{
      try {
        const recipe:EditRecipe=JSON.parse(signature);
        if(kind==="film"){
          if(!recipe.colorPipeline)return;
          recipe.colorPipeline={...recipe.colorPipeline,film:{id,amount:100,enabled:true}};
        }else recipe.advanced={exposure:0,temperature:0,tint:0,vibrance:0,curves:[],...recipe.advanced,lookup:{id,amount:100,enabled:true}};
        const next=await editPreviewStyle(context.sessionId,recipe);
        if(!live){URL.revokeObjectURL(next);return;}
        if(url.current)URL.revokeObjectURL(url.current);
        url.current=next;setSrc(next);
      }catch(e){if(live)setError(String(e));}
    }}),400);
    return()=>{live=false;clearTimeout(timer);};
  },[visible,context.sessionId,signature,id,kind]);
  useEffect(()=>()=>{if(url.current)URL.revokeObjectURL(url.current);},[]);
  return <div ref={host} className="aspect-[3/2] overflow-hidden rounded bg-black/30" title={error??undefined}>
    {src?<img src={src} alt="" className="h-full w-full object-cover"/>:<span className="flex h-full items-center justify-center text-text-muted" aria-hidden="true">{error?"!":"…"}</span>}
  </div>;
}
export default function StyleCatalog({kind,entries,value,onSelect,preview,disabled=false}:{kind:"film"|"lut";entries:StyleEntry[];value?:string;onSelect:(id:string)=>void;preview?:StylePreviewContext;disabled?:boolean}){
  const {t}=useTranslation();const key=`photographer.editor.${kind}-favorites.v1`;
  const [favorite,setFavorite]=useState(()=>favorites(key)),[query,setQuery]=useState(""),[category,setCategory]=useState("all"),[page,setPage]=useState(0),[error,setError]=useState<string|null>(null);
  const matching=entries.filter(entry=>(category==="all"||category==="favorites"&&favorite.includes(entry.id)||category==="builtin"&&entry.builtin||category==="imported"&&!entry.builtin)&&`${entry.name} ${entry.subtitle??""}`.toLocaleLowerCase().includes(query.toLocaleLowerCase()));
  function toggle(id:string){try{const next=favorite.includes(id)?favorite.filter(v=>v!==id):[...favorite,id].slice(-200);localStorage.setItem(key,JSON.stringify(next));setFavorite(next);setError(null);}catch(e){setError(String(e));}}
  return <details className="space-y-2 rounded border border-edge p-2">
    <summary className="text-xs">{t("advancedEditor.styleCatalog.title")}</summary>
    <input type="search" className="w-full rounded border border-edge bg-bg p-2 text-xs" aria-label={t("advancedEditor.styleCatalog.search")} placeholder={t("advancedEditor.styleCatalog.search")} value={query} onChange={e=>{setQuery(e.target.value);setPage(0);}}/>
    <select className="w-full rounded border border-edge bg-bg p-2 text-xs" aria-label={t("advancedEditor.styleCatalog.category")} value={category} onChange={e=>{setCategory(e.target.value);setPage(0);}}>{["all","favorites","builtin","imported"].map(key=><option key={key} value={key}>{t(`advancedEditor.styleCatalog.${key}`)}</option>)}</select>
    <div className="grid grid-cols-2 gap-2">{matching.slice(page*12,page*12+12).map(entry=><div key={entry.id} className="min-w-0 rounded border border-edge p-1">
      <button type="button" className="block w-full text-left aria-pressed:text-accent disabled:opacity-30" disabled={disabled} aria-pressed={entry.id===value} onClick={()=>onSelect(entry.id)}>{preview&&<Thumbnail context={preview} id={entry.id} kind={kind}/>}<span className="mt-1 block truncate text-[11px]" title={entry.name}>{entry.name}</span></button>
      <button type="button" className="w-full text-right text-xs aria-pressed:text-amber-400" aria-label={t("advancedEditor.styleCatalog.favorite",{name:entry.name})} aria-pressed={favorite.includes(entry.id)} onClick={()=>toggle(entry.id)}>{favorite.includes(entry.id)?"★":"☆"}</button>
    </div>)}</div>
    {!matching.length&&<p className="text-xs text-text-muted">{t("advancedEditor.styleCatalog.empty")}</p>}
    {matching.length>12&&<div className="flex justify-between text-xs"><button type="button" disabled={page===0} onClick={()=>setPage(v=>Math.max(0,v-1))}>←</button><span>{page+1} / {Math.ceil(matching.length/12)}</span><button type="button" disabled={(page+1)*12>=matching.length} onClick={()=>setPage(v=>v+1)}>→</button></div>}
    {preview&&<p className="text-[11px] text-text-muted">{t("advancedEditor.styleCatalog.hint")}</p>}{error&&<p role="alert" className="text-xs text-red-400">{error}</p>}
  </details>;
}
