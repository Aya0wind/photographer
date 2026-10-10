import {useRef} from "react";
import {useTranslation} from "react-i18next";
import type {EditRecipe} from "@/ipc/api";
import type {AdvancedAction} from "../lib/advancedRecipe";
import {changeGeometry,DEFAULT_GEOMETRY} from "../lib/geometry";
import type {RecipeContext} from "../lib/recipe";
import Slider from "./AdjustmentSlider";
const BUTTON="rounded-xl border border-edge px-3 py-2 text-xs text-text-secondary hover:border-accent hover:text-accent disabled:opacity-30";
export default function GeometryPanel({recipe,dispatch,context,begin,end,onCropChange}:{recipe:EditRecipe;dispatch:(action:AdvancedAction)=>void;context:RecipeContext;begin:()=>void;end:()=>void;onCropChange?:(crop:EditRecipe["crop"])=>void}) {
 const baseline=useRef<EditRecipe|null>(null);
 const {t}=useTranslation();const g={...DEFAULT_GEOMETRY,...recipe.geometry};
 function change(patch:Partial<typeof g>,record=true){
  const original=!record&&baseline.current?baseline.current:recipe;
  const next=changeGeometry(original,patch,context);dispatch({type:"geometry",patch,record,baseline:!record?baseline.current??undefined:undefined});onCropChange?.(next.crop);
 }
 return <section className="space-y-3" aria-label={t("editor.geometry.title")}>
  <div className="flex flex-wrap gap-2">
    <button type="button" className={BUTTON} aria-pressed={g.flipHorizontal} onClick={()=>change({flipHorizontal:!g.flipHorizontal})}>{t("editor.geometry.flipHorizontal")}</button>
    <button type="button" className={BUTTON} aria-pressed={g.flipVertical} onClick={()=>change({flipVertical:!g.flipVertical})}>{t("editor.geometry.flipVertical")}</button>
  </div>
  <Slider label={t("editor.geometry.angle")} unit="°" value={g.angle} min={-180} max={180} step={0.1} onChange={angle=>change({angle},false)} onSet={angle=>change({angle})} onReset={()=>change({angle:0})} begin={()=>{baseline.current=recipe;begin();}} end={()=>{end();baseline.current=null;}}/>
  <button type="button" className={BUTTON} disabled={!recipe.rotateQuarter&&!recipe.crop&&!g.angle&&!g.flipHorizontal&&!g.flipVertical}
    onClick={()=>{end();dispatch({type:"geometryReset"});onCropChange?.(null);}}>{t("editor.geometry.reset")}</button>
 </section>;
}
