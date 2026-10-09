import { useTranslation } from "react-i18next";
import type { AdvancedAdjustments, EditAdjustments, EditRecipe } from "@/ipc/api";
import { DEFAULT_ADVANCED } from "../lib/advancedRecipe";

function Slider({ label, value, min = -100, max = 100, step = 1, onChange, begin, end }: {
  label: string; value: number; min?: number; max?: number; step?: number;
  onChange: (value: number) => void; begin: () => void; end: () => void;
}) {
  return <label className="block space-y-2">
    <span className="flex justify-between text-xs text-text-secondary"><span>{label}</span><output className="tabular-nums text-text-muted">{value}</output></span>
    <input type="range" min={min} max={max} step={step} value={value}
      onPointerDown={begin} onPointerUp={end} onPointerCancel={end} onBlur={end}
      onKeyDown={begin} onKeyUp={end} onChange={(e) => onChange(Number(e.target.value))}
      className="w-full accent-accent" />
  </label>;
}

const CURVES: Record<string, [number, number][]> = {
  linear: [],
  contrast: [[0, 0], [64, 45], [128, 128], [192, 210], [255, 255]],
  faded: [[0, 20], [64, 70], [128, 135], [192, 198], [255, 245]],
};

export default function AdvancedAdjustmentPanel({ recipe, onBasic, onAdvanced, begin, end }: {
  recipe: EditRecipe; onBasic: (patch: Partial<EditAdjustments>) => void;
  onAdvanced: (patch: Partial<AdvancedAdjustments>, record?: boolean) => void;
  begin: () => void; end: () => void;
}) {
  const { t } = useTranslation();
  const a = { ...DEFAULT_ADVANCED, ...recipe.advanced };
  const basic = { brightness: 0, contrast: 0, saturation: 0, ...recipe.adjustments };
  const points = a.curves.length ? a.curves : [[0, 0], [64, 64], [128, 128], [192, 192], [255, 255]];
  return <div className="space-y-6">
    <section className="space-y-4">
      <h3 className="text-xs font-semibold text-text-primary">{t("advancedEditor.light")}</h3>
      <Slider label={t("advancedEditor.exposure")} value={a.exposure} min={-5} max={5} step={0.1} onChange={(exposure) => onAdvanced({ exposure }, false)} begin={begin} end={end} />
      {(["brightness", "contrast"] as const).map((key) => <Slider key={key} label={t(`editor.adjust.${key}`)} value={basic[key]} onChange={(value) => onBasic({ [key]: value })} begin={begin} end={end} />)}
    </section>
    <section className="space-y-4 border-t border-edge pt-4">
      <h3 className="text-xs font-semibold text-text-primary">{t("advancedEditor.color")}</h3>
      {(["temperature", "tint", "vibrance"] as const).map((key) => <Slider key={key} label={t(`advancedEditor.${key}`)} value={a[key]} onChange={(value) => onAdvanced({ [key]: value }, false)} begin={begin} end={end} />)}
      <Slider label={t("editor.adjust.saturation")} value={basic.saturation} onChange={(saturation) => onBasic({ saturation })} begin={begin} end={end} />
    </section>
    <section className="space-y-3 border-t border-edge pt-4">
      <h3 className="text-xs font-semibold text-text-primary">{t("advancedEditor.curves")}</h3>
      <svg viewBox="0 0 255 255" className="aspect-square w-full rounded-xl border border-edge bg-bg" aria-label={t("advancedEditor.curves")} role="img">
        {[64, 128, 192].map((v) => <path key={v} d={`M${v} 0V255M0 ${v}H255`} stroke="currentColor" className="text-edge" />)}
        <path d="M0 255 255 0" stroke="currentColor" className="text-text-muted/30" />
        <polyline points={points.map(([x, y]) => `${x},${255 - y}`).join(" ")} fill="none" stroke="currentColor" strokeWidth="2" className="text-accent" />
        {points.map(([x, y], i) => <circle key={i} cx={x} cy={255 - y} r="4" fill="currentColor" className="text-accent" />)}
      </svg>
      <div className="flex flex-wrap gap-1.5">{Object.entries(CURVES).map(([key, curves]) => <button key={key} type="button" className="rounded-lg border border-edge px-2 py-1.5 text-xs text-text-secondary hover:border-accent" onClick={() => onAdvanced({ curves })}>{t(`advancedEditor.curve.${key}`)}</button>)}</div>
      <p className="text-[11px] leading-relaxed text-text-muted">{t("advancedEditor.curveHint")}</p>
      {points.slice(1, -1).map(([x, y], i) => <Slider key={x} label={`${t("advancedEditor.tone")} ${x}`} value={y} min={0} max={255} onChange={(value) => {
        const curves: [number, number][] = points.map(([px, py], index) => [px, index === i + 1 ? value : py]);
        onAdvanced({ curves }, false);
      }} begin={begin} end={end} />)}
    </section>
  </div>;
}
