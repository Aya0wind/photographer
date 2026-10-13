import {useEffect,useRef,useState} from "react";
import {useTranslation} from "react-i18next";

export default function Slider({ label, value, unit, gradient, min = -100, max = 100, step = 1, defaultValue = 0, onChange, onSet, onReset, begin, end }: {
  gradient?:string;
  label: string; value: number; unit?:string; min?: number; max?: number; step?: number; defaultValue?:number;
  onSet:(value:number)=>void;
  onReset:()=>void;
  onChange: (value: number) => void; begin: () => void; end: () => void;
}) {
  const {t}=useTranslation();
  const [draft,setDraft]=useState(String(value));
  const [editing,setEditing]=useState(false);
  const cancelled=useRef(false);
  const keyboard=useRef<{value:number;keys:Set<string>}|null>(null);
  const fineDrag=useRef<{id:number;x:number;raw:number;value:number}|null>(null);
  const adjustmentKeys=["ArrowLeft","ArrowRight","ArrowUp","ArrowDown","PageUp","PageDown","Home","End"];
  function increment(value:number,amount:number){return Math.max(min,Math.min(max,Number((value+amount).toFixed(10))));}
  function finishKeyboard(){if(keyboard.current){keyboard.current=null;end();}}
  function finishPointer(){fineDrag.current=null;end();}
  useEffect(()=>{if(keyboard.current&&keyboard.current.value!==value)keyboard.current=null;if(fineDrag.current&&fineDrag.current.value!==value)fineDrag.current=null;},[value]);
  useEffect(()=>{if(!editing) setDraft(String(value));},[value,editing]);
  function commitNumber() {
    setEditing(false);
    const parsed=draft.trim()==="" ? NaN : Number(draft);
    if(!cancelled.current && Number.isFinite(parsed)) onSet(Math.max(min,Math.min(max,parsed)));
    cancelled.current=false;setDraft(String(value));
  }
  return <div className="grid gap-0.5">
    <span className="flex items-center justify-between text-[11px] leading-[14px] text-text-secondary"><span>{label}</span><span className="flex items-center gap-1"><input type="number" aria-label={label} min={min} max={max} step={step} value={draft}
      className="w-14 rounded border border-transparent bg-transparent px-1 text-right tabular-nums text-text-muted focus:border-accent focus:text-text-primary"
      onFocus={()=>setEditing(true)} onChange={event=>setDraft(event.target.value)} onBlur={commitNumber}
      onKeyDown={event=>{
        if(event.key==="ArrowUp"||event.key==="ArrowDown"){
          event.preventDefault();event.stopPropagation();const current=draft.trim()===""?value:Number(draft);
          if(Number.isFinite(current))setDraft(String(increment(current,(event.key==="ArrowUp"?1:-1)*step*(event.altKey?0.1:event.shiftKey?10:1))));
        }
        if(event.key==="Enter" || event.key==="Escape") {event.preventDefault();event.stopPropagation();cancelled.current=event.key==="Escape";event.currentTarget.blur();}
      }} />{unit&&<span className="text-text-muted">{unit}</span>}<button type="button" disabled={value===defaultValue}
      aria-label={t("advancedEditor.resetParameter",{parameter:label})} title={t("advancedEditor.resetParameter",{parameter:label})}
      className="h-5 w-5 rounded text-text-muted hover:bg-accent/10 hover:text-accent disabled:opacity-20"
      onClick={event=>{event.preventDefault();event.stopPropagation();onReset();}}>↺</button></span></span>
    <input type="range" aria-label={label} title={t("advancedEditor.parameterStepHint")} min={min} max={max} step={step/10} value={value}
      onPointerDown={event=>{
        if(event.button!==0)return;
        if(event.altKey){event.preventDefault();event.currentTarget.focus();}
        finishKeyboard();begin();
        if(event.altKey){fineDrag.current={id:event.pointerId,x:event.clientX,raw:value,value};event.currentTarget.setPointerCapture(event.pointerId);}
      }} onPointerMove={event=>{
        const drag=fineDrag.current;if(!drag||drag.id!==event.pointerId)return;
        event.preventDefault();const width=event.currentTarget.getBoundingClientRect().width;if(width<=0)return;
        drag.raw=increment(drag.raw,(event.clientX-drag.x)/width*(max-min)*(event.altKey?0.1:1));drag.x=event.clientX;
        drag.value=increment(0,Math.round(drag.raw/(step/10))*(step/10));onChange(drag.value);
      }} onPointerUp={finishPointer} onPointerCancel={finishPointer} onLostPointerCapture={()=>{if(fineDrag.current)finishPointer();}} onBlur={()=>{if(keyboard.current)finishKeyboard();else finishPointer();}}
      onDoubleClick={onReset} onKeyDown={event=>{
        if(!adjustmentKeys.includes(event.key))return;
        event.preventDefault();event.stopPropagation();
        if(!keyboard.current){keyboard.current={value,keys:new Set()};begin();}
        const gesture=keyboard.current;gesture.keys.add(event.key);
        const direction=["ArrowLeft","ArrowDown","PageDown"].includes(event.key)?-1:1;
        const factor=event.altKey?0.1:event.shiftKey||event.key==="PageUp"||event.key==="PageDown"?10:1;
        gesture.value=event.key==="Home"?min:event.key==="End"?max:increment(gesture.value,direction*step*factor);
        onChange(gesture.value);
      }} onKeyUp={event=>{if(!keyboard.current||!adjustmentKeys.includes(event.key))return;event.stopPropagation();keyboard.current.keys.delete(event.key);if(!keyboard.current.keys.size)finishKeyboard();}} onChange={(e) => {if(!fineDrag.current)onChange(Number(e.target.value));}}
      style={gradient?{background:gradient}:undefined}
      className={gradient?"editor-color-slider block h-2 w-full rounded-full":"block h-4 w-full accent-accent"} />
  </div>;
}

