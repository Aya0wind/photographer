import AnnotationControls from "./AnnotationControls";
import type {ReactNode} from "react";
import CropAspectControls from "./CropAspectControls";
import GeometryPanel from "./GeometryPanel";
import CropDimensions from "./CropDimensions";
import {remapDraftCrop} from "../lib/cropPixels";
import {recipeReducer,initRecipeHistory,rotatedSize} from "../lib/recipe";
import {geometrySize} from "../lib/geometry";
import { useTranslation } from "react-i18next";
import type { EditRecipe,ExportOptions } from "@/ipc/api";
import type { EditorTool } from "./EditorCanvas";
import type { CurveSampling } from "./CurveEditor";
import AdvancedAdjustmentPanel from "./AdvancedAdjustmentPanel";
import { clampCrop, isFullCrop } from "../lib/coords";
import { type RecipeContext } from "../lib/recipe";
import type { AdvancedAction } from "../lib/advancedRecipe";

const BUTTON = "editor-button";
interface ToolPanelProps {
  lensPanel?:ReactNode;
  onLensOpenChange?:(open:boolean)=>void;
  brushSettings?:{erase:boolean;opacity:number};
  onBrushSettings?:(value:{erase:boolean;opacity:number})=>void;
  exportExtras?:ReactNode;
  exportFormat:NonNullable<ExportOptions["format"]>;
  setExportFormat:(value:NonNullable<ExportOptions["format"]>)=>void;
  exportColorSpace:NonNullable<ExportOptions["colorSpace"]>;
  setExportColorSpace:(value:NonNullable<ExportOptions["colorSpace"]>)=>void;
  cancelGesture?:()=>void;
  onCancelCrop:()=>void;
  straightening:boolean;setStraightening:(value:boolean)=>void;
  onModalChange?:(open:boolean)=>void;
  sampling: CurveSampling;
  tool: EditorTool;
  previewSessionId?:string;
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
  exportAlbum:()=>void;
}

export default function AdvancedToolPanel({lensPanel,onLensOpenChange,brushSettings,onBrushSettings, previewSessionId,exportExtras,cancelGesture,onCancelCrop,straightening,setStraightening,onModalChange, sampling, tool, recipe, dispatch, beginGesture, endGesture,
  context, cropDraft, setCropDraft, setTool, cropRatio, setCropRatio, color, setColor,
  selectedText, setSelectedText, brushWidth, setBrushWidth, exporting, exportFolder,exportAlbum,exportFormat,setExportFormat,exportColorSpace,setExportColorSpace }: ToolPanelProps) {
  const { t } = useTranslation();
  const selectedLayer = recipe.textLayers.find((layer) => layer.id === selectedText);
  function rotateCrop(delta:1|-1){endGesture();const next=recipeReducer(initRecipeHistory(recipe),{type:"rotate",delta},context.current).present;setCropDraft(remapDraftCrop(recipe,next,cropDraft,context.current)??{x:0,y:0,w:1,h:1});setCropRatio(null);dispatch({type:"rotate",delta});}
  return <>
          {tool === "adjust" && <AdvancedAdjustmentPanel lensPanel={lensPanel} onLensOpenChange={onLensOpenChange} previewSessionId={previewSessionId} onPipeline={(value,record)=>dispatch({type:"colorPipeline",value,record})} cancel={cancelGesture} onModalChange={onModalChange} sampling={sampling} recipe={recipe} onResetDevelopment={group=>{endGesture();dispatch({type:"resetDevelopmentGroup",group});}} onReset={(group)=>{endGesture();dispatch({type:"resetAdjustmentGroup",group});}} begin={beginGesture} end={endGesture} onBasic={(patch, record=false) => dispatch({ type: "adjust", patch, record })} onAdvanced={(patch, record) => dispatch({ type: "advanced", patch, record })} />}

          {tool === "crop" && <div className="space-y-4">
            <div className="flex gap-2"><button className={BUTTON} onClick={() => rotateCrop(-1)}>{t("editor.rotateCcw")}</button><button className={BUTTON} onClick={() => rotateCrop(1)}>{t("editor.rotateCw")}</button></div>
            <GeometryPanel recipe={recipe} dispatch={dispatch} context={context.current} cropDraft={cropDraft} cropRatio={cropRatio} begin={beginGesture} end={endGesture} onCropChange={crop=>{if(crop===null)setCropRatio(null);setCropDraft(crop??{x:0,y:0,w:1,h:1});}}/>
            <button type="button" className={`${BUTTON} w-full aria-pressed:text-accent aria-pressed:border-accent`} aria-pressed={straightening} onClick={()=>setStraightening(!straightening)}>{t("editor.geometry.straighten")}</button>
            {straightening&&<p className="text-xs text-text-muted">{t("editor.geometry.straightenHint")}</p>}
            <CropDimensions recipe={recipe} context={context.current} crop={cropDraft} onChange={setCropDraft} onFree={()=>setCropRatio(null)}/>
            <CropAspectControls ratio={cropRatio} onRatio={setCropRatio} size={{width:geometrySize(context.current,recipe).w,height:geometrySize(context.current,recipe).h}}
              original={rotatedSize(context.current,recipe.rotateQuarter).w/rotatedSize(context.current,recipe.rotateQuarter).h} onCrop={setCropDraft}/>

            <button className="ui-primary w-full rounded-xl py-2 text-xs" onClick={() => {endGesture();dispatch({ type: "cropConfirm", crop: cropDraft && !isFullCrop(cropDraft) ? clampCrop(cropDraft,1e-6) : null }); setCropDraft(null); setTool("adjust"); }}>{t("editor.crop.apply")}</button>
            <button className={`${BUTTON} w-full`} onClick={onCancelCrop}>{t("editor.crop.cancel")}</button>
          </div>}
          {(tool === "text" || tool === "brush") && <div className="space-y-4">
            <label className="flex items-center justify-between text-xs text-text-secondary">{t("advancedEditor.paintColor")}<input type="color" value={color} onChange={(e) => { setColor(e.target.value); if (tool === "text" && selectedText) dispatch({ type: "textUpdate", id: selectedText, patch: { color: e.target.value } }); }} /></label>
            {tool === "brush" && <label className="block text-xs text-text-secondary">{t("advancedEditor.brushSize")}<input className="mt-1 block h-4 w-full accent-accent" type="range" min={0.001} max={0.05} step={0.001} value={brushWidth} onChange={(e) => setBrushWidth(Number(e.target.value))} /></label>}
            {tool === "text" && <>{selectedLayer && <><textarea className="w-full rounded-xl border border-edge bg-bg p-3 text-xs text-text-primary" value={selectedLayer.text} onFocus={beginGesture} onBlur={endGesture} onChange={(e) => dispatch({ type: "textUpdateLive", id: selectedLayer.id, patch: { text: e.target.value } })} /><button className={BUTTON} onClick={() => { dispatch({ type: "textRemove", id: selectedLayer.id }); setSelectedText(null); }}>{t("advancedEditor.removeText")}</button></>}</>}
          </div>}
          {(tool==="text"||tool==="brush")&&<AnnotationControls recipe={recipe} selected={selectedText} onSelect={setSelectedText} dispatch={dispatch} begin={beginGesture} end={endGesture} mode={tool} brush={brushSettings} onBrush={onBrushSettings}/>}
          {tool === "output" && <div className="space-y-4">
            {exportExtras}
            <label className="block space-y-2 text-xs text-text-secondary"><span>{t("advancedEditor.outputFormat")}</span><select aria-label={t("advancedEditor.outputFormat")} className="w-full rounded-xl border border-edge bg-bg p-2" value={exportFormat} onChange={e=>setExportFormat(e.target.value as NonNullable<ExportOptions["format"]>)}>{["jpeg","png","tiff","pcraft"].map(format=><option key={format} value={format}>{t(`advancedEditor.outputFormats.${format}`)}</option>)}</select></label>
            {exportFormat!=="pcraft"&&<label className="block space-y-2 text-xs text-text-secondary"><span>{t("editor.output.longEdge")}</span><input className="w-full rounded-xl border border-edge bg-bg p-2" type="number" min={1} value={recipe.output.longEdge ?? ""} placeholder={t("editor.output.longEdgePlaceholder")} onChange={(e) => { const value = Number(e.target.value); if (e.target.value === "" || (Number.isInteger(value) && value > 0)) dispatch({ type: "setOutput", patch: { longEdge: e.target.value === "" ? null : value } }); }} /></label>}
            {exportFormat!=="pcraft"&&<label className="block space-y-2 text-xs text-text-secondary"><span>{t("advancedEditor.outputColorSpace")}</span><select aria-label={t("advancedEditor.outputColorSpace")} className="w-full rounded-xl border border-edge bg-bg p-2" value={exportColorSpace} onChange={e=>setExportColorSpace(e.target.value as NonNullable<ExportOptions["colorSpace"]>)}>{(["srgb","displayP3","adobeRgb","proPhotoRgb"] as const).map(space=><option key={space} value={space}>{t(`advancedEditor.outputColorSpaces.${space}`)}</option>)}</select></label>}
            {exportFormat==="pcraft"&&<p className="text-xs leading-relaxed text-text-secondary">{t("advancedEditor.outputDocumentHint")}</p>}
            {exportFormat==="jpeg"&&<label className="grid gap-0.5 text-[11px] leading-[14px] text-text-secondary"><span>{t("editor.output.quality")} · {recipe.output.quality}</span><input className="block h-4 w-full accent-accent" type="range" min={1} max={100} value={recipe.output.quality} onChange={(e) => dispatch({ type: "setOutput", patch: { quality: Number(e.target.value) } })} /></label>}
            <button className={`${BUTTON} w-full`} disabled={exporting} onClick={() => void exportFolder()}>{t("editor.export")}</button>
            <button className={`${BUTTON} w-full`} disabled={exporting||exportFormat==="pcraft"} onClick={exportAlbum}>{t("editor.exportAlbum")}</button>
          </div>}
  </>;
}
