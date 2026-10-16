import {useState} from "react";
import {useTranslation} from "react-i18next";
import {DEFAULT_EXPORT_SETTINGS,type EditorExportSettings as Settings} from "../lib/exportSettings";
const KEY="photographer.editor.export-presets.v1";
interface Preset {name:string;settings:Settings}
function read():Preset[]{try{const value=JSON.parse(localStorage.getItem(KEY)??"[]");return Array.isArray(value)?value.filter(v=>v&&typeof v.name==="string"&&typeof v.settings?.template==="string").slice(0,30):[];}catch{return [];}}
export default function EditorExportSettings({value,onChange}:{value:Settings;onChange:(value:Settings)=>void}) {
 const {t}=useTranslation();const [presets,setPresets]=useState(read),[name,setName]=useState(""),[error,setError]=useState<string|null>(null);
 function save(){try{const next=[...presets.filter(p=>p.name!==name.trim()),{name:name.trim(),settings:value}].slice(-30);localStorage.setItem(KEY,JSON.stringify(next));setPresets(next);setName("");}catch(e){setError(String(e));}}
 return <details open className="space-y-3 border-t border-edge pt-3"><summary className="text-xs font-semibold">{t("advancedEditor.exportSettings.title")}</summary>
  <label className="block space-y-1 text-xs">{t("advancedEditor.exportSettings.presets")}<select className="w-full rounded border border-edge bg-bg p-2" value="" onChange={e=>{const preset=presets.find(p=>p.name===e.target.value);if(preset)onChange({...DEFAULT_EXPORT_SETTINGS,...preset.settings});}}><option value="">—</option>{presets.map(p=><option key={p.name}>{p.name}</option>)}</select></label>
  {(["template","author","copyright","keywords"] as const).map(key=><label key={key} className="block space-y-1 text-xs"><span>{t(`advancedEditor.exportSettings.${key}`)}</span><input className="w-full rounded border border-edge bg-bg p-2" value={value[key]} maxLength={key==="template"?180:1000} onChange={e=>onChange({...value,[key]:e.target.value})}/></label>)}
  <p className="text-[11px] text-text-muted">{t("advancedEditor.exportSettings.templateHint")}</p>
  <label className="block space-y-1 text-xs">{t("advancedEditor.exportSettings.sequence")}<input type="number" min={1} max={999999} className="w-full rounded border border-edge bg-bg p-2" value={value.sequence} onChange={e=>onChange({...value,sequence:Math.max(1,Math.min(999999,Math.trunc(Number(e.target.value)||1)))})}/></label>
  <label className="flex items-center gap-2 text-xs"><input type="checkbox" checked={value.removeGps} onChange={e=>onChange({...value,removeGps:e.target.checked})}/>{t("advancedEditor.exportSettings.removeGps")}</label>
  <p className="text-[11px] text-text-muted">{t("advancedEditor.exportSettings.noUpscale")}</p>
  <div className="flex gap-1"><input aria-label={t("advancedEditor.exportSettings.presetName")} placeholder={t("advancedEditor.exportSettings.presetName")} value={name} maxLength={100} onChange={e=>setName(e.target.value)} className="min-w-0 flex-1 rounded border border-edge bg-bg p-2 text-xs"/><button type="button" disabled={!name.trim()} onClick={save} className="rounded border border-edge p-2 text-xs disabled:opacity-30">{t("advancedEditor.exportSettings.savePreset")}</button></div>
  {error&&<p role="alert" className="text-xs text-red-400">{error}</p>}
 </details>;
}
