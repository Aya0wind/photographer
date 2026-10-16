import RawWhiteBalancePanel from "./RawWhiteBalancePanel";
import {useTranslation} from "react-i18next";
import type {EditRecipe} from "@/ipc/api";
import type {BaseProfile} from "../lib/editorPreferences";

export default function BaseProfilePanel({recipe,isRaw,defaultProfile,onDefault,onChange,onUpgrade,disabled,onWhiteBalance,onWhiteBalancePicker,rawReady=false,begin=()=>{},end=()=>{} }:{
  begin?:()=>void;end?:()=>void;rawReady?:boolean;onWhiteBalance?:(value:NonNullable<EditRecipe["colorPipeline"]>["rawWhiteBalance"],record?:boolean)=>void;onWhiteBalancePicker?:()=>void;
  recipe:EditRecipe;isRaw:boolean;defaultProfile:BaseProfile;onDefault:(value:BaseProfile)=>void;
  onChange:(value:BaseProfile)=>void;onUpgrade:()=>void;disabled:boolean;
}) {
  const {t}=useTranslation();
  const pipeline=recipe.colorPipeline;
  const modern=pipeline?.version===2;
  const covered=modern&&pipeline.film?.enabled&&pipeline.film.amount===100;
  return <section className="space-y-3" aria-label={t("advancedEditor.baseColor.title")}>
    <h3 className="text-xs font-semibold">{t("advancedEditor.baseColor.title")}</h3>
    {!modern&&<p className="text-xs text-text-muted">{t(isRaw?"advancedEditor.baseColor.legacy":"advancedEditor.baseColor.rendered")}</p>}
    {modern&&(["standard","soft","neutral"] as const).map(id=><button key={id} type="button" disabled={disabled||!!covered}
      aria-pressed={pipeline.baseProfile===id} onClick={()=>onChange(id)}
      className="w-full rounded-xl border border-edge p-3 text-left aria-pressed:border-accent aria-pressed:bg-accent/10 disabled:opacity-40">
      <span className="block text-xs font-semibold">{t(`advancedEditor.baseColor.${id}`)}</span>
      <span className="mt-1 block text-[11px] text-text-muted">{t(`advancedEditor.baseColor.${id}Hint`)}</span>
    </button>)}
    {covered&&<p className="text-[11px] text-text-muted">{t("advancedEditor.baseColor.covered")}</p>}
    {isRaw&&!modern&&<button type="button" disabled={disabled} className="w-full rounded-lg border border-edge p-2 text-xs" onClick={onUpgrade}>{t("advancedEditor.baseColor.upgrade")}</button>}
    <label className="block space-y-2 text-xs"><span>{t("advancedEditor.baseColor.default")}</span><select disabled={disabled} value={defaultProfile}
      onChange={event=>onDefault(event.target.value as BaseProfile)} className="w-full rounded-lg border border-edge bg-bg p-2">
      {(["standard","soft","neutral"] as const).map(id=><option key={id} value={id}>{t(`advancedEditor.baseColor.${id}`)}</option>)}
    </select></label>
    <p className="text-[11px] text-text-muted">{t("advancedEditor.baseColor.defaultHint")}</p>
    {modern&&isRaw&&onWhiteBalance&&onWhiteBalancePicker&&<RawWhiteBalancePanel begin={begin} end={end} value={pipeline.rawWhiteBalance} onChange={onWhiteBalance} onPicker={onWhiteBalancePicker} ready={rawReady} disabled={disabled}/>}
  </section>;
}
