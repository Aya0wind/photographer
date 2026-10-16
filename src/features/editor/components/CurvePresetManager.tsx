import {useState} from "react";
import {useTranslation} from "react-i18next";
import type {AdvancedAdjustments} from "@/ipc/api";

type Curves = Pick<AdvancedAdjustments,"curves"|"channelCurves"|"parametricCurve">;
interface Preset {name:string;value:Curves}
const KEY="photographer.editor.curve-presets.v1";
function read():Preset[] {
  try {
    const text=localStorage.getItem(KEY)??"[]";
    if(text.length>256*1024)return [];
    const rows:unknown=JSON.parse(text);
    if(!Array.isArray(rows))return [];
    return rows.filter((row):row is Preset=>{
      if(!row||typeof row.name!=="string"||row.name.length>100||!Array.isArray(row.value?.curves))return false;
      const curves=[row.value.curves,...Object.values(row.value.channelCurves??{})];
      return curves.every(points=>Array.isArray(points)&&points.length<=64&&points.every(p=>Array.isArray(p)&&p.length===2&&p.every(n=>Number.isFinite(n)&&n>=0&&n<=255)));
    }).slice(-30);
  }catch{return [];}
}
export default function CurvePresetManager({value,onChange}:{value:AdvancedAdjustments;onChange:(value:Curves)=>void}) {
  const {t}=useTranslation();
  const [presets,setPresets]=useState(read),[name,setName]=useState(""),[selected,setSelected]=useState(""),[error,setError]=useState<string|null>(null);
  function persist(next:Preset[]){try{localStorage.setItem(KEY,JSON.stringify(next));setPresets(next);setError(null);}catch(e){setError(String(e));}}
  function save(){
    const label=name.trim();if(!label)return;
    const settings:Curves={curves:structuredClone(value.curves),channelCurves:structuredClone(value.channelCurves),parametricCurve:structuredClone(value.parametricCurve)};
    persist([...presets.filter(p=>p.name!==selected&&p.name!==label),{name:label,value:settings}].slice(-30));setSelected(label);
  }
  return <details className="space-y-2 rounded border border-edge p-2">
    <summary className="text-xs">{t("advancedEditor.curvePresets.title")}</summary>
    <select aria-label={t("advancedEditor.curvePresets.title")} value={selected} className="w-full rounded border border-edge bg-bg p-2 text-xs" onChange={e=>{setSelected(e.target.value);const preset=presets.find(p=>p.name===e.target.value);if(preset){setName(preset.name);onChange(structuredClone(preset.value));}}}>
      <option value="">—</option>{presets.map(p=><option key={p.name}>{p.name}</option>)}
    </select>
    <input className="w-full rounded border border-edge bg-bg p-2 text-xs" aria-label={t("advancedEditor.curvePresets.name")} value={name} maxLength={100} onChange={e=>setName(e.target.value)}/>
    <div className="flex gap-2"><button type="button" className="rounded border border-edge p-2 text-xs disabled:opacity-30" disabled={!name.trim()} onClick={save}>{t("advancedEditor.curvePresets.save")}</button><button type="button" className="rounded border border-edge p-2 text-xs disabled:opacity-30" disabled={!selected} onClick={()=>{persist(presets.filter(p=>p.name!==selected));setSelected("");setName("");}}>{t("advancedEditor.lut.remove")}</button></div>
    <p className="text-[11px] text-text-muted">{t("advancedEditor.curvePresets.hint")}</p>{error&&<p role="alert" className="text-xs text-red-400">{error}</p>}
  </details>;
}
