import {DEFAULT_MASK_RANGE} from "./MaskRangePanel";
import {useState} from "react";
import {useTranslation} from "react-i18next";
import type {EditLocalMask} from "@/ipc/api";
import {newLayerId} from "../lib/recipe";
type Component=NonNullable<EditLocalMask["components"]>[number];
export default function MaskComponentsPanel({mask,others,selected,onSelect,onChange}:{mask:EditLocalMask;others:EditLocalMask[];selected:number|null;onSelect:(value:number|null)=>void;onChange:(value:Component[])=>void}){
 const {t}=useTranslation();const [operation,setOperation]=useState<Component["operation"]>("add"),[source,setSource]=useState("brush");
 const components=mask.components??[];
 function add(){
  const existing=others.find(item=>item.id===source);
  const shape:EditLocalMask=existing?{...structuredClone(existing),id:newLayerId(),enabled:true,advanced:undefined,adjustments:undefined}:{id:newLayerId(),name:t(`advancedEditor.masks.${source}`),kind:source as EditLocalMask["kind"],range:source==="range"?{...DEFAULT_MASK_RANGE}:undefined,enabled:true,inverted:false,density:100,feather:0,from:{x:0.3,y:0.5},to:{x:0.7,y:0.5},strokes:[]};
  onChange([...components,{operation,mask:shape}]);onSelect(components.length);
 }
 return <details open className="space-y-2 rounded border border-edge p-2">
  <summary className="text-xs font-semibold">{t("advancedEditor.masks.components")}</summary>
  <button type="button" aria-pressed={selected===null} className="w-full rounded border border-edge p-2 text-left text-xs aria-pressed:border-accent" onClick={()=>onSelect(null)}>{t("advancedEditor.masks.baseArea")}</button>
  {components.map((component,index)=><div key={component.mask.id} className="flex gap-1"><button type="button" className="min-w-0 flex-1 truncate rounded border border-edge p-2 text-left text-xs aria-pressed:border-accent" aria-pressed={selected===index} onClick={()=>onSelect(index)}>{t(`advancedEditor.masks.${component.operation}`)} · {component.mask.name}</button><button type="button" aria-label={t("advancedEditor.masks.removeComponent")} className="px-2 text-xs" onClick={()=>{onChange(components.filter((_,i)=>i!==index));onSelect(null);}}>×</button></div>)}
  <div className="flex gap-1"><select aria-label={t("advancedEditor.masks.operation")} value={operation} onChange={e=>setOperation(e.target.value as Component["operation"])} className="min-w-0 flex-1 rounded border border-edge bg-bg p-1 text-xs">{["add","subtract","intersect"].map(value=><option key={value} value={value}>{t(`advancedEditor.masks.${value}`)}</option>)}</select>
  <select aria-label={t("advancedEditor.masks.componentSource")} value={source} onChange={e=>setSource(e.target.value)} className="min-w-0 flex-1 rounded border border-edge bg-bg p-1 text-xs">{["brush","linear","radial","range"].map(value=><option key={value} value={value}>{t(`advancedEditor.masks.${value}`)}</option>)}{others.map(item=><option key={item.id} value={item.id}>{item.name}</option>)}</select>
  <button type="button" disabled={components.length>=16} onClick={add} className="rounded border border-edge px-2 text-xs disabled:opacity-30">＋</button></div>
  {selected!==null&&components[selected]&&<label className="flex items-center gap-2 text-xs"><input type="checkbox" checked={components[selected].mask.inverted} onChange={e=>onChange(components.map((item,i)=>i===selected?{...item,mask:{...item.mask,inverted:e.target.checked}}:item))}/>{t("advancedEditor.masks.invertComponent")}</label>}
 </details>;
}
