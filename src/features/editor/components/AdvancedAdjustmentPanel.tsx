import Slider from "./AdjustmentSlider";
import HistogramPanel from "./HistogramPanel";
import ExtendedColorPanel from "./ExtendedColorPanel";
import {DevelopmentControls} from "./DevelopmentPanel";
import ColorGradingPanel from "./ColorGradingPanel";
import ParametricCurvePanel from "./ParametricCurvePanel";
import LutPanel from "./LutPanel";
import { useEffect, useState } from "react";
import CurveEditor, { type CurveSampling } from "./CurveEditor";
import { useTranslation } from "react-i18next";
import type { AdvancedAdjustments, EditAdjustments, EditRecipe } from "@/ipc/api";
import { DEFAULT_ADVANCED, adjustmentGroupChanged, type AdjustmentGroup,type DevelopmentGroup } from "../lib/advancedRecipe";

export default function AdvancedAdjustmentPanel({ recipe, onBasic, onAdvanced, onReset, begin, end, sampling, onModalChange,cancel }: {
  cancel?:()=>void;
  onResetDevelopment?:(group:DevelopmentGroup)=>void;
  onModalChange?:(open:boolean)=>void;
  onReset:(group:AdjustmentGroup)=>void;
  recipe: EditRecipe; onBasic: (patch: Partial<EditAdjustments>,record?:boolean) => void;
  onAdvanced: (patch: Partial<AdvancedAdjustments>, record?: boolean) => void;
  begin: () => void; end: () => void; sampling: CurveSampling;
}) {
  const { t } = useTranslation();
  const a = { ...DEFAULT_ADVANCED, ...recipe.advanced };
  const basic = { brightness: 0, contrast: 0, saturation: 0, ...recipe.adjustments };
  const [range, setRange] = useState<"reds" | "yellows" | "greens" | "cyans" | "blues" | "magentas">("reds");
  const hsl = { hue: 0, saturation: 0, lightness: 0, ...a.hsl?.[range] };
  const levels = a.levels ?? { black: 0, white: 255, gamma: 1 };
  useEffect(()=>{if((sampling.picker==="hsl"||sampling.picker==="hslTarget") && sampling.hslRange) setRange(sampling.hslRange);},[sampling.picker,sampling.hslRange,sampling.sample]);
  function resetButton(group:AdjustmentGroup,label:string,changed:boolean) {
    return <button type="button" disabled={!changed} aria-label={t("advancedEditor.resetGroup",{group:label})}
      className="rounded-md px-2 py-1 text-[11px] text-accent hover:bg-accent/10 disabled:opacity-30"
      onClick={event=>{event.preventDefault();event.stopPropagation();onReset(group);}}>{t("editor.reset")}</button>;
  }
  return <div className="space-y-4">
    {sampling.onHistogramOpen&&<HistogramPanel bins={sampling.liveHistogram??[]} busy={sampling.histogramBusy??false} onOpen={sampling.onHistogramOpen}/>}
    {sampling.picker&&sampling.sample&&<div role="status" className="space-y-1 text-[11px] text-text-secondary"><span>{t("advancedEditor.sampleReadout")}</span><div className="flex items-center gap-2 tabular-nums"><span className="h-3 w-3 shrink-0 rounded border border-edge" style={{backgroundColor:`rgb(${sampling.sample.join(",")})`}}/><span>RGB {sampling.sample.map(value=>value.toFixed(1)).join(" · ")}</span></div></div>}
    {sampling.available!==false&&sampling.picker&&sampling.setSampleSize&&<label className="flex items-center justify-between gap-2 text-xs text-text-secondary"><span>{t("advancedEditor.sampleSize")}</span><select aria-label={t("advancedEditor.sampleSize")} value={sampling.sampleSize??1} className="rounded-lg border border-edge bg-bg px-2 py-1.5" onChange={e=>sampling.setSampleSize!(Number(e.target.value) as 1|3|5|11)}>{[1,3,5,11].map(size=><option key={size} value={size}>{size} × {size}</option>)}</select></label>}
    <details open className="space-y-2.5">
      <summary className="flex cursor-pointer items-center justify-between"><h3 className="text-xs font-semibold text-text-primary">{t("advancedEditor.light")}</h3>{resetButton("light",t("advancedEditor.light"),adjustmentGroupChanged(recipe,"light"))}</summary>
      <Slider label={t("advancedEditor.exposure")} value={a.exposure} onSet={exposure=>onAdvanced({exposure})} onReset={()=>onAdvanced({exposure:0})} min={-5} max={5} step={0.1} onChange={(exposure) => onAdvanced({ exposure }, false)} begin={begin} end={end} />
      {(["brightness", "contrast"] as const).map((key) => <Slider key={key} label={t(`editor.adjust.${key}`)} value={key === "contrast" ? Math.max(-50, basic[key]) : basic[key]} min={key === "contrast" ? -50 : -100} onSet={value=>onBasic({[key]:value},true)} onReset={()=>onBasic({[key]:0},true)} onChange={(value) => onBasic({ [key]: value })} begin={begin} end={end} />)}
      <DevelopmentControls group="tone" adjustments={a} onChange={onAdvanced} begin={begin} end={end}/>
    </details>
    <details open className="space-y-2.5 border-t border-edge pt-3">
      <summary className="flex cursor-pointer items-center justify-between"><h3 className="text-xs font-semibold text-text-primary">{t("advancedEditor.color")}</h3>{resetButton("color",t("advancedEditor.color"),!!(a.temperature || a.tint || a.vibrance || basic.saturation || recipe.legacyAdjustments?.saturation))}</summary>
      {(["temperature", "tint", "vibrance"] as const).map((key) => <Slider key={key} label={t(`advancedEditor.${key}`)} value={a[key]} onSet={value=>onAdvanced({[key]:value})} onReset={()=>onAdvanced({[key]:0})} onChange={(value) => onAdvanced({ [key]: value }, false)} begin={begin} end={end} />)}
      <Slider label={t("editor.adjust.saturation")} value={basic.saturation} onSet={saturation=>onBasic({saturation},true)} onReset={()=>onBasic({saturation:0},true)} onChange={(saturation) => onBasic({ saturation })} begin={begin} end={end} />
    </details>
    {(["effects","detail"] as const).map(group=><details key={group} className="space-y-2.5 border-t border-edge pt-3">
      <summary className="flex cursor-pointer items-center justify-between text-xs font-semibold"><span>{t(`advancedEditor.${group}`)}</span>{resetButton(group,t(`advancedEditor.${group}`),adjustmentGroupChanged(recipe,group))}</summary>
      {group==="effects"&&<DevelopmentControls group="presence" adjustments={a} onChange={onAdvanced} begin={begin} end={end}/>}
      <DevelopmentControls group={group} adjustments={a} onChange={onAdvanced} begin={begin} end={end}/>
    </details>)}
    <CurveEditor onReset={()=>onReset("curves")} adjustments={a} onChange={onAdvanced} begin={begin} end={end} sampling={sampling} />
    <ParametricCurvePanel value={a.parametricCurve} onChange={(parametricCurve,record)=>onAdvanced({parametricCurve},record)} onReset={()=>onReset("parametricCurve")} begin={begin} end={end} onModalChange={onModalChange}/>
    <details className="space-y-2.5 border-t border-edge pt-3">
      <summary className="flex cursor-pointer items-center justify-between text-xs font-semibold"><span>{t("advancedEditor.levels")}</span>{resetButton("levels",t("advancedEditor.levels"),!!a.levels)}</summary>
      <Slider label={t("advancedEditor.blackLevel")} value={levels.black} onSet={value=>onAdvanced({levels:{...levels,black:value}})} onReset={()=>onAdvanced({levels:{...levels,black:0}})} min={0} max={levels.white - 2} onChange={(black) => onAdvanced({ levels: { ...levels, black } }, false)} begin={begin} end={end} />
      <Slider label={t("advancedEditor.midGamma")} value={levels.gamma} onSet={value=>onAdvanced({levels:{...levels,gamma:value}})} onReset={()=>onAdvanced({levels:{...levels,gamma:1}})} defaultValue={1} min={.1} max={4} step={.01} onChange={(gamma) => onAdvanced({ levels: { ...levels, gamma } }, false)} begin={begin} end={end} />
      <Slider label={t("advancedEditor.whiteLevel")} value={levels.white} onSet={value=>onAdvanced({levels:{...levels,white:value}})} onReset={()=>onAdvanced({levels:{...levels,white:255}})} defaultValue={255} min={levels.black + 2} max={255} onChange={(white) => onAdvanced({ levels: { ...levels, white } }, false)} begin={begin} end={end} />
    </details>
    <details className="space-y-2.5 border-t border-edge pt-3">
      <summary className="flex cursor-pointer items-center justify-between text-xs font-semibold"><span>HSL</span>{resetButton("hsl","HSL",Object.keys(a.hsl??{}).length>0)}</summary>
      {sampling.available!==false&&<button type="button" disabled={sampling.busy} aria-pressed={sampling.picker==="hsl"} onClick={()=>sampling.setPicker(sampling.picker==="hsl"?null:"hsl")}
        className="flex w-full items-center justify-center gap-2 rounded-lg border border-edge px-3 py-2 text-xs aria-pressed:border-accent aria-pressed:text-accent">
        {sampling.picker==="hsl" && sampling.sample && <span className="h-3 w-3 rounded-full border border-edge" style={{backgroundColor:`rgb(${sampling.sample.join(",")})`}} />}
        {t("advancedEditor.hslPicker")}
      </button>}
      {sampling.available!==false&&sampling.setTargetHslAxis&&<div className="flex gap-2"><select className="min-w-0 flex-1 rounded-lg border border-edge bg-bg p-2 text-xs" aria-label={t("advancedEditor.targetHslAxis")} value={sampling.targetHslAxis??"lightness"} onChange={e=>sampling.setTargetHslAxis!(e.target.value as "hue"|"saturation"|"lightness")}>{["hue","saturation","lightness"].map(key=><option key={key} value={key}>{t(`advancedEditor.hslLabels.${key}`)}</option>)}</select><button type="button" disabled={sampling.busy} className="rounded-lg border border-edge px-2 text-xs aria-pressed:border-accent aria-pressed:text-accent" aria-pressed={sampling.picker==="hslTarget"} onClick={()=>sampling.setPicker(sampling.picker==="hslTarget"?null:"hslTarget")}>{t("advancedEditor.targetHsl")}</button></div>}
      {sampling.picker==="hslTarget"&&<p className="text-[11px] text-text-muted">{t("advancedEditor.targetHint")}</p>}
      {sampling.busy&&<p role="status" className="text-xs text-text-muted">{t("advancedEditor.sampling")}</p>}
      {sampling.picker==="hsl" && <p role="status" className="text-[11px] text-text-muted">{t("advancedEditor.hslPickerHint")}</p>}
      <select aria-label={t("advancedEditor.colorRange")} value={range} className="w-full rounded-lg border border-edge bg-bg p-2 text-xs" onChange={(e) => setRange(e.target.value as typeof range)}>
        {(["reds", "yellows", "greens", "cyans", "blues", "magentas"] as const).map((key) => <option key={key} value={key}>{t(`advancedEditor.ranges.${key}`)}</option>)}
      </select>
      {(["hue", "saturation", "lightness"] as const).map((key) => <Slider key={key} label={t(`advancedEditor.hslLabels.${key}`)} value={hsl[key]} onSet={value=>onAdvanced({hsl:{...a.hsl,[range]:{...hsl,[key]:value}}})} onReset={()=>onAdvanced({hsl:{...a.hsl,[range]:{...hsl,[key]:0}}})} min={key === "hue" ? -180 : -100} max={key === "hue" ? 180 : 100} onChange={(value) => onAdvanced({ hsl: { ...a.hsl, [range]: { ...hsl, [key]: value } } }, false)} begin={begin} end={end} />)}
      <button type="button" disabled={!a.hsl?.[range]} className="text-xs text-accent disabled:opacity-30" onClick={() => {const hsl={...a.hsl};delete hsl[range];onAdvanced({hsl});}}>{t("advancedEditor.resetColorRange")}</button>
    </details>
    <ExtendedColorPanel adjustments={a} onChange={onAdvanced} onReset={onReset} begin={begin} end={end}/>
    <ColorGradingPanel value={a.colorGrading} onChange={(colorGrading,record)=>onAdvanced({colorGrading},record)} onReset={()=>onReset("colorGrading")} begin={begin} end={end} cancel={cancel}/>
    <LutPanel onModalChange={onModalChange} value={a.lookup} onChange={(lookup,record)=>onAdvanced({lookup},record)} begin={begin} end={end}/>
  </div>;
}
