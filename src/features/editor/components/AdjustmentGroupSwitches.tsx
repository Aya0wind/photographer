import {useTranslation} from "react-i18next";
import type {AdvancedAdjustments,EditRecipe} from "@/ipc/api";
import {adjustmentGroupChanged,type AdjustmentGroup} from "../lib/advancedRecipe";
const LABELS={pointColors:"advancedEditor.pointColor.title",primaries:"advancedEditor.primaries.title",light:"advancedEditor.light",color:"advancedEditor.color",curves:"advancedEditor.curves",parametricCurve:"advancedEditor.parametric.title",levels:"advancedEditor.levels",hsl:"advancedEditor.hsl",colorBalance:"advancedEditor.extra.colorBalance",blackWhite:"advancedEditor.extra.blackWhite",selectiveColor:"advancedEditor.extra.selectiveColor",colorGrading:"advancedEditor.grading.title",detail:"advancedEditor.develop.detail",effects:"advancedEditor.develop.effects"} satisfies Partial<Record<AdjustmentGroup,string>>;
export default function AdjustmentGroupSwitches({recipe,onChange}:{recipe:EditRecipe;onChange:(patch:Partial<AdvancedAdjustments>)=>void}){
  const {t}=useTranslation();const disabled=recipe.advanced?.disabledGroups??[];
  return <details className="space-y-2 rounded border border-edge p-2"><summary className="text-xs">{t("advancedEditor.groups.title")}</summary>
    {(Object.keys(LABELS) as (keyof typeof LABELS)[]).map(group=><label key={group} className="flex items-center gap-2 text-xs"><input type="checkbox" checked={!disabled.includes(group)} onChange={e=>onChange({disabledGroups:e.target.checked?disabled.filter(v=>v!==group):[...disabled,group]})}/><span className="flex-1">{t(LABELS[group])}</span>{adjustmentGroupChanged(recipe,group)&&<span title={t("advancedEditor.groups.modified")} className="text-accent">●</span>}</label>)}
    <p className="text-[11px] text-text-muted">{t("advancedEditor.groups.hint")}</p>
  </details>;
}
