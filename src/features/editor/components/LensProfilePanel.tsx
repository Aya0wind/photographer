import {useEffect,useState} from "react";
import {useTranslation} from "react-i18next";
import {editLensProfiles,type EditorLensCatalog,type EditorLensProfileSettings} from "@/ipc/api";
export default function LensProfilePanel({assetId,libraryId,isRaw,value,onChange,preparing}:{assetId:number;libraryId:string;isRaw:boolean;value?:EditorLensProfileSettings;onChange:(value:EditorLensProfileSettings|undefined)=>void;preparing:boolean}){
 const {t}=useTranslation();const [catalog,setCatalog]=useState<EditorLensCatalog|null>(null),[error,setError]=useState<string|null>(null),[loading,setLoading]=useState(false),[query,setQuery]=useState("");
 const profileId=value?.profileId??"";
 useEffect(()=>{setCatalog(null);setError(null);if(!isRaw)return;let live=true;setLoading(true);void editLensProfiles(assetId,libraryId,{distortion:true,tca:true,autoCrop:true,profileId}).then(v=>{if(live)setCatalog(v);}).catch(e=>{if(live)setError(String(e));}).finally(()=>{if(live)setLoading(false);});return()=>{live=false;};},[assetId,libraryId,isRaw,profileId]);
 const matchedId=catalog?.profileId;
 const selected=catalog?.profiles.find(p=>p.id===(profileId||matchedId));
 const defaults={distortion:selected?.distortion??true,tca:selected?.tca??false,autoCrop:true,profileId};
 const settings=value??defaults;
 const profiles=catalog?.profiles.filter(p=>`${p.maker} ${p.model}`.toLocaleLowerCase().includes(query.toLocaleLowerCase()))??[];
 return <section className="space-y-2 border-b border-edge pb-3"><h3 className="text-xs font-medium">{t("advancedEditor.lens.profileTitle")}</h3>
  <p className="text-[11px] text-text-muted">{t("advancedEditor.lens.database")}</p>
  {!isRaw?<p className="text-xs text-text-muted">{t("advancedEditor.lens.rawOnly")}</p>:<>
   {loading&&<p role="status" className="text-xs animate-pulse">{t("advancedEditor.lens.matching")}</p>}
   {catalog&&<><p className="text-[11px] text-text-muted">{[catalog.cameraMake,catalog.cameraModel,catalog.lensModel].filter(Boolean).join(" · ")}{catalog.focalLength>0?` · ${catalog.focalLength} mm`:""}</p><p role="status" className="text-xs">{t(`advancedEditor.lens.states.${catalog.state}`)}</p>
   <input aria-label={t("advancedEditor.lens.search")} placeholder={t("advancedEditor.lens.search")} value={query} onChange={e=>setQuery(e.target.value)} className="w-full rounded border border-edge bg-surface px-2 py-1 text-xs"/>
   <select aria-label={t("advancedEditor.lens.profileTitle")} value={profileId} className="w-full rounded border border-edge bg-surface p-1 text-xs" onChange={e=>{const p=catalog.profiles.find(p=>p.id===e.target.value);onChange({...settings,profileId:e.target.value,distortion:p?.distortion??true,tca:p?.tca??false});}}><option value="">{t("advancedEditor.lens.autoMatch")}</option>{profileId&&!profiles.some(p=>p.id===profileId)&&<option value={profileId}>{selected?`${selected.maker} ${selected.model}`:profileId}</option>}{profiles.map(p=><option key={p.id} value={p.id}>{p.matched?"★ ":""}{p.maker} {p.model}</option>)}</select>
   <label className="flex gap-2 text-xs"><input type="checkbox" checked={!!value} disabled={!value&&catalog.state!=="matched"} onChange={e=>onChange(e.target.checked?defaults:undefined)}/>{t("advancedEditor.lens.enableProfile")}</label>
   <fieldset disabled={!value} className="space-y-2 disabled:opacity-40">{(["distortion","tca","autoCrop"] as const).map(key=><label key={key} className="flex gap-2 text-xs"><input type="checkbox" checked={settings[key]} disabled={key!=="autoCrop"&&!selected?.[key]} onChange={e=>onChange({...settings,[key]:e.target.checked})}/>{t(`advancedEditor.lens.profile.${key}`)}</label>)}</fieldset>
   <p className="text-[11px] text-text-muted">{t("advancedEditor.lens.profileHint")}</p></>}
   {preparing&&<p role="status" className="text-xs animate-pulse">{t("advancedEditor.lens.preparing")}</p>}
   {error&&<p role="alert" className="text-xs text-red-400">{error}</p>}
  </>}
 </section>;
}
