import {useEffect,useRef,useState} from "react";
import {useTranslation} from "react-i18next";
import {moveCurvePoint,type CurvePoint} from "../lib/curves";
function Coordinate({label,value,min,max,onSet}:{label:string;value:number|undefined;min:number;max:number;onSet:(value:number)=>void}) {
 const [draft,setDraft]=useState(value===undefined?"":String(value));const [editing,setEditing]=useState(false);const cancelled=useRef(false);
 useEffect(()=>{if(!editing)setDraft(value===undefined?"":String(value));},[value,editing]);
 function finish(){setEditing(false);const number=draft.trim()===""?NaN:Number(draft);if(!cancelled.current&&Number.isFinite(number)&&value!==number)onSet(number);cancelled.current=false;setDraft(value===undefined?"":String(value));}
 return <label className="flex min-w-0 items-center gap-2 text-[11px] text-text-muted"><span>{label}</span><input className="min-w-0 flex-1 rounded border border-edge bg-bg px-2 py-1 text-right tabular-nums disabled:opacity-30" type="number" aria-label={label} min={min} max={max} step={0.1} value={draft} disabled={value===undefined} onFocus={()=>setEditing(true)} onChange={e=>setDraft(e.target.value)} onBlur={finish}
  onKeyDown={e=>{e.stopPropagation();if(e.key==="Enter"||e.key==="Escape"){e.preventDefault();cancelled.current=e.key==="Escape";e.currentTarget.blur();}}}/></label>;
}
export default function CurveCoordinates({points,selected,onChange}:{points:CurvePoint[];selected:number|null;onChange:(points:CurvePoint[])=>void}){
 const {t}=useTranslation();const point=selected===null?undefined:points[selected];
 const min=selected===null||selected===0?0:points[selected-1]?.[0]+1;
 const max=selected===null||selected===points.length-1?255:points[selected+1]?.[0]-1;
 function set(axis:0|1,value:number){if(selected===null||!point)return;onChange(moveCurvePoint(points,selected,axis===0?value:point[0],axis===1?value:point[1]));}
 return <div className="grid grid-cols-2 gap-3"><Coordinate key={`input:${selected}`} label={t("advancedEditor.inputTone")} value={point?.[0]} min={min} max={max} onSet={v=>set(0,v)}/><Coordinate key={`output:${selected}`} label={t("advancedEditor.outputTone")} value={point?.[1]} min={0} max={255} onSet={v=>set(1,v)}/></div>;
}
