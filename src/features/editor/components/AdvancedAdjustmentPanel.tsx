import { useState } from "react";
import CurveEditor, { type CurveSampling } from "./CurveEditor";
import { useTranslation } from "react-i18next";
import type { AdvancedAdjustments, EditAdjustments, EditRecipe } from "@/ipc/api";
import { DEFAULT_ADVANCED } from "../lib/advancedRecipe";

function Slider({ label, value, min = -100, max = 100, step = 1, onChange, begin, end }: {
  label: string; value: number; min?: number; max?: number; step?: number;
  onChange: (value: number) => void; begin: () => void; end: () => void;
}) {
  return <label className="grid gap-0.5">
    <span className="flex items-center justify-between text-[11px] leading-[14px] text-text-secondary"><span>{label}</span><output className="tabular-nums text-text-muted">{value}</output></span>
    <input type="range" min={min} max={max} step={step} value={value}
      onPointerDown={begin} onPointerUp={end} onPointerCancel={end} onBlur={end}
      onKeyDown={begin} onKeyUp={end} onChange={(e) => onChange(Number(e.target.value))}
      className="block h-4 w-full accent-accent" />
  </label>;
}

export default function AdvancedAdjustmentPanel({ recipe, onBasic, onAdvanced, begin, end, sampling }: {
  recipe: EditRecipe; onBasic: (patch: Partial<EditAdjustments>) => void;
  onAdvanced: (patch: Partial<AdvancedAdjustments>, record?: boolean) => void;
  begin: () => void; end: () => void; sampling: CurveSampling;
}) {
  const { t } = useTranslation();
  const a = { ...DEFAULT_ADVANCED, ...recipe.advanced };
  const basic = { brightness: 0, contrast: 0, saturation: 0, ...recipe.adjustments };
  const [range, setRange] = useState<"reds" | "yellows" | "greens" | "cyans" | "blues" | "magentas">("reds");
  const hsl = { hue: 0, saturation: 0, lightness: 0, ...a.hsl?.[range] };
  const levels = a.levels ?? { black: 0, white: 255, gamma: 1 };
  return <div className="space-y-4">
    <section className="space-y-2.5">
      <h3 className="text-xs font-semibold text-text-primary">{t("advancedEditor.light")}</h3>
      <Slider label={t("advancedEditor.exposure")} value={a.exposure} min={-5} max={5} step={0.1} onChange={(exposure) => onAdvanced({ exposure }, false)} begin={begin} end={end} />
      {(["brightness", "contrast"] as const).map((key) => <Slider key={key} label={t(`editor.adjust.${key}`)} value={basic[key]} onChange={(value) => onBasic({ [key]: value })} begin={begin} end={end} />)}
    </section>
    <section className="space-y-2.5 border-t border-edge pt-3">
      <h3 className="text-xs font-semibold text-text-primary">{t("advancedEditor.color")}</h3>
      {(["temperature", "tint", "vibrance"] as const).map((key) => <Slider key={key} label={t(`advancedEditor.${key}`)} value={a[key]} onChange={(value) => onAdvanced({ [key]: value }, false)} begin={begin} end={end} />)}
      <Slider label={t("editor.adjust.saturation")} value={basic.saturation} onChange={(saturation) => onBasic({ saturation })} begin={begin} end={end} />
    </section>
    <CurveEditor adjustments={a} onChange={onAdvanced} begin={begin} end={end} sampling={sampling} />
    <details className="space-y-2.5 border-t border-edge pt-3">
      <summary className="cursor-pointer text-xs font-semibold">{t("advancedEditor.levels")}</summary>
      <Slider label={t("advancedEditor.blackLevel")} value={levels.black} min={0} max={levels.white - 2} onChange={(black) => onAdvanced({ levels: { ...levels, black } }, false)} begin={begin} end={end} />
      <Slider label={t("advancedEditor.midGamma")} value={levels.gamma} min={.1} max={4} step={.01} onChange={(gamma) => onAdvanced({ levels: { ...levels, gamma } }, false)} begin={begin} end={end} />
      <Slider label={t("advancedEditor.whiteLevel")} value={levels.white} min={levels.black + 2} max={255} onChange={(white) => onAdvanced({ levels: { ...levels, white } }, false)} begin={begin} end={end} />
      <button className="text-xs text-accent" onClick={() => onAdvanced({ levels: { black: 0, white: 255, gamma: 1 } })}>{t("editor.reset")}</button>
    </details>
    <details className="space-y-2.5 border-t border-edge pt-3">
      <summary className="cursor-pointer text-xs font-semibold">HSL</summary>
      <select aria-label={t("advancedEditor.colorRange")} value={range} className="w-full rounded-lg border border-edge bg-bg p-2 text-xs" onChange={(e) => setRange(e.target.value as typeof range)}>
        {(["reds", "yellows", "greens", "cyans", "blues", "magentas"] as const).map((key) => <option key={key} value={key}>{t(`advancedEditor.ranges.${key}`)}</option>)}
      </select>
      {(["hue", "saturation", "lightness"] as const).map((key) => <Slider key={key} label={t(`advancedEditor.hslLabels.${key}`)} value={hsl[key]} min={key === "hue" ? -180 : -100} max={key === "hue" ? 180 : 100} onChange={(value) => onAdvanced({ hsl: { ...a.hsl, [range]: { ...hsl, [key]: value } } }, false)} begin={begin} end={end} />)}
      <button className="text-xs text-accent" onClick={() => onAdvanced({ hsl: { ...a.hsl, [range]: { hue: 0, saturation: 0, lightness: 0 } } })}>{t("editor.reset")}</button>
    </details>
  </div>;
}
