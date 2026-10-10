import {useEffect,useRef,useState} from "react";
import {useTranslation} from "react-i18next";

export default function Slider({ label, value, unit, min = -100, max = 100, step = 1, defaultValue = 0, onChange, onSet, onReset, begin, end }: {
  label: string; value: number; unit?:string; min?: number; max?: number; step?: number; defaultValue?:number;
  onSet:(value:number)=>void;
  onReset:()=>void;
  onChange: (value: number) => void; begin: () => void; end: () => void;
}) {
  const {t}=useTranslation();
  const [draft,setDraft]=useState(String(value));
  const [editing,setEditing]=useState(false);
  const cancelled=useRef(false);
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
      onKeyDown={event=>{if(event.key==="Enter" || event.key==="Escape") {event.preventDefault();event.stopPropagation();cancelled.current=event.key==="Escape";event.currentTarget.blur();}}} />{unit&&<span className="text-text-muted">{unit}</span>}<button type="button" disabled={value===defaultValue}
      aria-label={t("advancedEditor.resetParameter",{parameter:label})} title={t("advancedEditor.resetParameter",{parameter:label})}
      className="h-5 w-5 rounded text-text-muted hover:bg-accent/10 hover:text-accent disabled:opacity-20"
      onClick={event=>{event.preventDefault();event.stopPropagation();onReset();}}>↺</button></span></span>
    <input type="range" aria-label={label} min={min} max={max} step={step} value={value}
      onPointerDown={begin} onPointerUp={end} onPointerCancel={end} onBlur={end}
      onDoubleClick={onReset} onKeyDown={begin} onKeyUp={end} onChange={(e) => onChange(Number(e.target.value))}
      className="block h-4 w-full accent-accent" />
  </div>;
}

