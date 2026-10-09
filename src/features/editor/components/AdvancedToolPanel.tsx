import { useTranslation } from "react-i18next";
import type { EditRecipe } from "@/ipc/api";
import type { EditorTool } from "./EditorCanvas";
import type { CurveSampling } from "./CurveEditor";
import AdvancedAdjustmentPanel from "./AdvancedAdjustmentPanel";
import { clampCrop, fitCropRect, isFullCrop } from "../lib/coords";
import { rotatedSize, type RecipeContext } from "../lib/recipe";
import type { AdvancedAction } from "../lib/advancedRecipe";

const BUTTON = "rounded-xl border border-edge px-3 py-2 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent disabled:opacity-40";
interface ToolPanelProps {
  sampling: CurveSampling;
  tool: EditorTool;
  recipe: EditRecipe;
  dispatch: (action: AdvancedAction) => void;
  beginGesture: () => void;
  endGesture: () => void;
  context: { current: RecipeContext };
  setCropDraft: (crop: EditRecipe["crop"]) => void;
  cropDraft: EditRecipe["crop"];
  setTool: (tool: EditorTool) => void;
  cropRatio: number | null;
  setCropRatio: (ratio: number | null) => void;
  color: string;
  setColor: (color: string) => void;
  selectedText: string | null;
  setSelectedText: (id: string | null) => void;
  brushWidth: number;
  setBrushWidth: (width: number) => void;
  exporting: boolean;
  exportFolder: () => Promise<void>;
}

export default function AdvancedToolPanel({ sampling, tool, recipe, dispatch, beginGesture, endGesture,
  context, cropDraft, setCropDraft, setTool, cropRatio, setCropRatio, color, setColor,
  selectedText, setSelectedText, brushWidth, setBrushWidth, exporting, exportFolder }: ToolPanelProps) {
  const { t } = useTranslation();
  const selectedLayer = recipe.textLayers.find((layer) => layer.id === selectedText);
  return <>
          {tool === "adjust" && <AdvancedAdjustmentPanel sampling={sampling} recipe={recipe} begin={beginGesture} end={endGesture} onBasic={(patch) => dispatch({ type: "adjust", patch, record: false })} onAdvanced={(patch, record) => dispatch({ type: "advanced", patch, record })} />}
          {tool === "view" && <p className="text-xs leading-relaxed text-text-muted">{t("advancedEditor.viewHint")}</p>}
          {tool === "crop" && <div className="space-y-4">
            <div className="flex gap-2"><button className={BUTTON} onClick={() => { dispatch({ type: "rotate", delta: -1 }); setCropDraft(null); setTool("view"); }}>{t("editor.rotateCcw")}</button><button className={BUTTON} onClick={() => { dispatch({ type: "rotate", delta: 1 }); setCropDraft(null); setTool("view"); }}>{t("editor.rotateCw")}</button></div>
            <div className="flex flex-wrap gap-2">{[null, 1, 4 / 3, 3 / 2, 16 / 9].map((ratio, index) => <button key={index} className={BUTTON} aria-pressed={cropRatio === ratio} onClick={() => {
              setCropRatio(ratio); const size = rotatedSize(context.current, recipe.rotateQuarter);
              setCropDraft(ratio ? fitCropRect(ratio, size.w / size.h) : { x: 0, y: 0, w: 1, h: 1 });
            }}>{[t("editor.crop.free"), "1:1", "4:3", "3:2", "16:9"][index]}</button>)}</div>
            <p className="text-xs text-text-muted">{t("editor.crop.hint")}</p>
            <button className="ui-primary w-full rounded-xl py-2 text-xs" onClick={() => { dispatch({ type: "cropApply", crop: cropDraft && !isFullCrop(cropDraft) ? clampCrop(cropDraft) : null }); setCropDraft(null); setTool("view"); }}>{t("editor.crop.apply")}</button>
            <button className={`${BUTTON} w-full`} onClick={() => { setCropDraft(null); setTool("view"); }}>{t("editor.crop.cancel")}</button>
          </div>}
          {(tool === "text" || tool === "brush") && <div className="space-y-4">
            <label className="flex items-center justify-between text-xs text-text-secondary">{t("advancedEditor.paintColor")}<input type="color" value={color} onChange={(e) => { setColor(e.target.value); if (tool === "text" && selectedText) dispatch({ type: "textUpdate", id: selectedText, patch: { color: e.target.value } }); }} /></label>
            {tool === "brush" && <label className="block text-xs text-text-secondary">{t("advancedEditor.brushSize")}<input className="mt-1 block h-4 w-full accent-accent" type="range" min={0.001} max={0.05} step={0.001} value={brushWidth} onChange={(e) => setBrushWidth(Number(e.target.value))} /></label>}
            {tool === "text" && <><p className="text-xs text-text-muted">{t("advancedEditor.textHint")}</p>{selectedLayer && <><textarea className="w-full rounded-xl border border-edge bg-bg p-3 text-xs text-text-primary" value={selectedLayer.text} onFocus={beginGesture} onBlur={endGesture} onChange={(e) => dispatch({ type: "textUpdateLive", id: selectedLayer.id, patch: { text: e.target.value } })} /><button className={BUTTON} onClick={() => { dispatch({ type: "textRemove", id: selectedLayer.id }); setSelectedText(null); }}>{t("advancedEditor.removeText")}</button></>}</>}
          </div>}
          {tool === "output" && <div className="space-y-4">
            <label className="block space-y-2 text-xs text-text-secondary"><span>{t("editor.output.longEdge")}</span><input className="w-full rounded-xl border border-edge bg-bg p-2" type="number" min={1} value={recipe.output.longEdge ?? ""} placeholder={t("editor.output.longEdgePlaceholder")} onChange={(e) => { const value = Number(e.target.value); if (e.target.value === "" || (Number.isInteger(value) && value > 0)) dispatch({ type: "setOutput", patch: { longEdge: e.target.value === "" ? null : value } }); }} /></label>
            <label className="grid gap-0.5 text-[11px] leading-[14px] text-text-secondary"><span>{t("editor.output.quality")} · {recipe.output.quality}</span><input className="block h-4 w-full accent-accent" type="range" min={1} max={100} value={recipe.output.quality} onChange={(e) => dispatch({ type: "setOutput", patch: { quality: Number(e.target.value) } })} /></label>
            <button className={`${BUTTON} w-full`} disabled={exporting} onClick={() => void exportFolder()}>{t("editor.export")}</button>
          </div>}
  </>;
}
