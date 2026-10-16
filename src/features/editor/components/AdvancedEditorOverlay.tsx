import EditorReferenceDialog from "./EditorReferenceDialog";
import EditorProofDialog from "./EditorProofDialog";
import EditorExportSettings from "./EditorExportSettings";
import {DEFAULT_EXPORT_SETTINGS,exportName,exportMetadata} from "../lib/exportSettings";
import EditorFilmstrip from "./EditorFilmstrip";
import EditorVersionsPanel from "./EditorVersionsPanel";
import {editVersionCreate} from "@/ipc/api";
import CanvasNavigator from "./CanvasNavigator";
import BaseProfilePanel from "./BaseProfilePanel";
import {useEditorPreferences,type BaseProfile} from "../lib/editorPreferences";
import {useProjectAutosave} from "../lib/useProjectAutosave";
import {useZoomIndicator} from "../lib/useZoomIndicator";
import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { CurvePicker, HslColorRange,CurveChannel,HslAxis } from "../lib/curves";
import {useTargetAdjustment} from "../lib/useTargetAdjustment";
import {changeGeometry} from "../lib/geometry";
import {remapDraftCrop} from "../lib/cropPixels";
import { useEffect, useMemo, useReducer, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { open } from "@tauri-apps/plugin-dialog";
import {
  editPreviewAutoLevels,editRawlabAvailable, assetDetail, editPreviewPick, albumList, assetAlbums, editProjectOpen, advancedExportFolder, editExportStatus, subscribeAppEvents,
  type AlbumDto, type AssetDto, type EditRecipe, type ExportOptions,
} from "@/ipc/api";
import ActionPopover from "@/shared/components/ActionPopover";
import EditorCanvas, { type EditorTool } from "./EditorCanvas";
import ExportAlbumPicker from "./ExportAlbumPicker";
import EditorExportProgress from "./EditorExportProgress";
import AdvancedToolPanel from "./AdvancedToolPanel";
import MaskPanel from "./MaskPanel";
import {maskGestureOwnsKey,type MaskBrush} from "./MaskCanvasControls";
import { advancedRecipe, advancedReducer,DEFAULT_DEVELOPMENT } from "../lib/advancedRecipe";
import { newLayerId, recipeEquals, recipeForPersist, type RecipeContext,type RecipeHistory } from "../lib/recipe";
import { clampCrop, isFullCrop, type Size } from "../lib/coords";
import { useNativeEditorCanvas } from "../lib/useNativeEditorCanvas";
import { useEditorPreview } from "../lib/useEditorPreview";
import {useMaskOverlay} from "../lib/useMaskOverlay";
import {useClippingOverlay} from "../lib/useClippingOverlay";
import {useSchemeTransfer} from "../lib/useSchemeTransfer";
import {usePixelReadout} from "../lib/usePixelReadout";
import HistogramPanel, {type HistogramGesture} from "./HistogramPanel";
import {useEditorHistogram} from "../lib/useEditorHistogram";
import {pickerAdjustment} from "../lib/pickerResult";
import ErrorModal from "@/shared/components/ErrorModal";
import { ASSET_DRAG_TYPE, useAdvancedEditorStore, type EditorPhoto } from "../lib/advancedEditorStore";

const TOOLS: { id: EditorTool; icon: string; label: string }[] = [
  { id: "crop", icon: "M6 3v15h15M3 6h15v15", label: "editor.tool.cropRotate" },
  { id: "adjust", icon: "M4 7h16M4 12h16M4 17h16M8 4v6M16 9v6M10 14v6", label: "editor.tool.adjust" },
  { id: "text", icon: "M5 5h14M12 5v14M8 19h8", label: "editor.tool.text" },
  { id: "brush", icon: "m5 15 10-11 5 5-11 10H4zM13 6l5 5", label: "editor.tool.brush" },
  { id:"mask",icon:"M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18Zm0 0v18",label:"advancedEditor.masks.title" },
  { id: "output", icon: "M12 3v12m-4-4 4 4 4-4M4 16v5h16v-5", label: "editor.tool.output" },
];
const BUTTON = "rounded-xl border border-edge px-3 py-2 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent aria-pressed:border-accent aria-pressed:bg-accent/10 aria-pressed:text-accent disabled:opacity-40";

export default function AdvancedEditorOverlay({ asset, libraryId, originAlbumId, originSubgroup, onClose }: { asset: AssetDto; libraryId: string; originAlbumId?: number; originSubgroup?: string | null; onClose: () => void }) {
  const { t } = useTranslation();
  const {preferences,update:updatePreferences}=useEditorPreferences();
  const [leftTab,setLeftTab]=useState<"profiles"|"history"|"versions">("profiles");
  // 大一统:无「活动库切换」,编辑期间资产归属不可变;仅防御资产数据异常
  const libraryChanged = asset.libraryId != null && asset.libraryId !== libraryId;
  const context = useRef<RecipeContext>({ width: 0, height: 0 });
  const [history, dispatch] = useReducer(
    (state: Parameters<typeof advancedReducer>[0], action: Parameters<typeof advancedReducer>[1]) => advancedReducer(state, action, context.current),
    undefined, () => ({ past: [], present: advancedRecipe(), future: [] }),
  );
  const recipe = history.present;
  const [tool, setTool] = useState<EditorTool>("adjust");
  const [selectedComponent,setSelectedComponent]=useState<number|null>(null);
  const [selectedMask,setSelectedMask]=useState<string|null>(null);
  useEffect(()=>{setSelectedComponent(null);},[selectedMask]);
  const [maskEditing,setMaskEditing]=useState(true);
  const [showMaskOverlay,setShowMaskOverlay]=useState(true);
  const [maskBrush,setMaskBrush]=useState<MaskBrush>({widthRel:0.05,hardness:1,opacity:1,flow:1,erase:false});
  const activeMask=recipe.masks?.find(mask=>mask.id===selectedMask);
  useEffect(()=>{if(!activeMask)setSelectedMask(recipe.masks?.[0]?.id??null);},[activeMask,recipe.masks]);
  const [nativeActive,setNativeActive]=useState(false);
  const nativeHost=useRef<HTMLElement|null>(null);
  const preview = useEditorPreview(asset.id, tool === "crop" ? { ...recipe, crop: null, textLayers: [], brushStrokes: [] } : recipe, libraryId, nativeActive);
  useEffect(()=>{if(preview.warning)setNotice(preview.warning);},[preview.warning]);
  const sourceSize = useMemo<Size | null>(() => preview.session ? { width: preview.session.width, height: preview.session.height } : null, [preview.session]);
  context.current = sourceSize ?? { width: 0, height: 0 };
  const [zoom, setZoom] = useState(1);
  const [fitScale,setFitScale]=useState(1);
  const [zoomMode,setZoomMode]=useState<"fit"|"100"|"200"|"custom">("fit");
  const [fullscreen,setFullscreen]=useState(false);
  const [exportSettings,setExportSettings]=useState(DEFAULT_EXPORT_SETTINGS);
  const [exportFormat,setExportFormat]=useState<NonNullable<ExportOptions["format"]>>("jpeg");
  const [exportColorSpace,setExportColorSpace]=useState<NonNullable<ExportOptions["colorSpace"]>>("srgb");
  const {visible:zoomVisible,wake:wakeZoom}=useZoomIndicator(zoom);
  const [compare, setCompare] = useState(false);
  const [clipping,setClipping]=useState(0);
  const activeClipping=["adjust","view","output","mask"].includes(tool)&&!(tool==="mask"&&maskEditing)?clipping:0;
  const [splitEnabled,setSplitEnabled]=useState(false),[splitPosition,setSplitPosition]=useState(0.5);
  const compareSplit=splitEnabled&&["adjust","view","output","mask"].includes(tool)&&!(tool==="mask"&&maskEditing)?splitPosition:null;
  const [picker, setPicker] = useState<CurvePicker | null>(null);
  useEffect(()=>{if(picker||!["adjust","view","output","mask"].includes(tool)||tool==="mask"&&maskEditing)setSplitEnabled(false);},[picker,tool,maskEditing]);
  const [sampleSize,setSampleSize]=useState<1|3|5|11>(3);
  const [targetCurveChannel,setTargetCurveChannel]=useState<CurveChannel>("rgb");
  const [targetHslAxis,setTargetHslAxis]=useState<HslAxis>("lightness");
  const [hslRange,setHslRange]=useState<HslColorRange|null>(null);
  const [sample, setSample] = useState<[number, number, number] | null>(null);
  const [samplingError, setSamplingError] = useState<string | null>(null);
  const [samplingBusy,setSamplingBusy]=useState(false);
  const pickBusy=useRef(false);
  const pickSignature=useRef("");pickSignature.current=JSON.stringify({recipe,picker,sampleSize,targetCurveChannel,sessionId:preview.session?.sessionId,maskId:tool==="mask"?selectedMask:null});
  const pickSequence = useRef(0);
  useEffect(() => { pickSequence.current++; }, [recipe, picker,selectedMask,tool,sampleSize,targetCurveChannel]);
  async function samplePhoto(x: number, y: number) {
    const sessionId = preview.session?.sessionId;
    if (!sessionId || !picker || picker==="curveTarget" || picker==="hslTarget" || pickBusy.current || tool==="mask"&&!activeMask) return;
    pickBusy.current=true;setSamplingBusy(true);setSamplingError(null);
    const signature=pickSignature.current;
    const sequence = ++pickSequence.current;
    try {
      const result = await editPreviewPick(sessionId, recipe, x, y, picker,tool==="mask"?activeMask?.id:undefined,targetCurveChannel,sampleSize);
      if (sequence !== pickSequence.current || signature!==pickSignature.current) return;
      setSamplingError(null);
      if(result.whiteBalance&&recipe.colorPipeline){dispatch({type:"colorPipeline",value:{...recipe.colorPipeline,relativeWhiteBalance:result.whiteBalance}});setPicker(null);}
      if (result.sample) {
        setSample(result.sample);
        if(picker==="hsl") {
          setHslRange(result.range??null);
          if(!result.range) setSamplingError(t("advancedEditor.hslNoColor"));
        }
      }
      const advanced=tool==="mask"?activeMask?.advanced:recipe.advanced;
      const patch=pickerAdjustment(picker,result,advanced);
      if (patch) {
        if(tool==="mask"&&activeMask)dispatch({type:"maskUpdate",id:activeMask.id,patch:{advanced:{exposure:0,temperature:0,tint:0,vibrance:0,...advanced,...patch}}});
        else dispatch({type:"advanced",patch});
      }
    } catch (error) { if (sequence === pickSequence.current && signature===pickSignature.current) setSamplingError(String(error)); }
    finally {pickBusy.current=false;if(mounted.current)setSamplingBusy(false);}
  }
  const [cropDraft, setCropDraft] = useState<EditRecipe["crop"]>(null);
  const [cropRatio, setCropRatio] = useState<number | null>(null);
  const [straightening,setStraightening]=useState(false);
  useEffect(()=>{if(tool!=="crop")setStraightening(false);},[tool]);
  const [selectedText, setSelectedText] = useState<string | null>(null);
  const [color, setColor] = useState("#FFFFFF");
  const [brushWidth, setBrushWidth] = useState(0.005);
  const [projectLoading, setProjectLoading] = useState(true);
  const [projectError, setProjectError] = useState<string | null>(null);
  const [projectRetry, setProjectRetry] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const pendingPhoto = useAdvancedEditorStore((s) => s.pending);
  const returnAfterExport = useRef(false);
  const [baseline, setBaseline] = useState(() => advancedRecipe());
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [notice, setNotice] = useState<string | null>(null);
  const maskOverlay=useMaskOverlay(preview.session?.sessionId,recipe,selectedMask,tool==="mask"&&showMaskOverlay&&!compare&&!nativeActive,error=>setNotice(String(error)));
  const [referenceOpen,setReferenceOpen]=useState(false);
  const [proofOpen,setProofOpen]=useState(false);
  const [toolModalOpen,setToolModalOpen]=useState(false);
  const [moreOpen,setMoreOpen]=useState(false);
  const closeAfterTarget=useRef(false);
  const [confirm, setConfirm] = useState<"close" | "reset" | "switch" | null>(null);
  const [albumPicker, setAlbumPicker] = useState(false);
  const [albums, setAlbums] = useState<AlbumDto[]>([]);
  const [sourceAlbums, setSourceAlbums] = useState<AlbumDto[]>([]);
  const albumCloseAfter = useRef(false);
  const [exporting, setExporting] = useState(false);
  const [exportPhase,setExportPhase]=useState("queued");
  const job = useRef<number | null>(null);
  const gesture = useRef<RecipeHistory | null>(null);
  const mounted = useRef(true);
  const effective = useMemo(() => tool === "crop" && cropDraft ? advancedReducer(history, {
    type: "cropApply", crop: isFullCrop(clampCrop(cropDraft,1e-6)) ? null : clampCrop(cropDraft,1e-6),
  }, context.current).present : recipe, [tool, cropDraft, history, recipe, sourceSize]);
  const dirty = !recipeEquals(recipeForPersist(effective), recipeForPersist(baseline));
  const unavailable = libraryChanged || !preview.session || preview.error !== null || projectLoading || projectError !== null;
  const target=useTargetAdjustment({sampleSize,sessionId:preview.session?.sessionId,history,mode:!unavailable&&!compare&&!(tool==="mask"&&!activeMask)&&(picker==="curveTarget"||picker==="hslTarget")?picker:null,maskId:tool==="mask"?activeMask?.id:undefined,channel:targetCurveChannel,axis:targetHslAxis,dispatch,
    onError:error=>setSamplingError(!error?null:error.includes("target-hsl-no-color")?t("advancedEditor.hslNoColor"):error.includes("target-curve-limit")?t("advancedEditor.targetCurveLimit"):error),onSample:(value,range)=>{setSample(value);if(range)setHslRange(range);}});
  const autosave=useProjectAutosave({assetId:asset.id,libraryId,recipe:recipeForPersist(recipe),
    enabled:!projectLoading&&!projectError&&!libraryChanged&&tool!=="crop"&&!target.active&&gesture.current===null&&!confirm&&!busy&&!exporting,
    dirty:!recipeEquals(recipeForPersist(recipe),recipeForPersist(baseline)),onSaved:setBaseline});
  const histogramStart=useRef<EditRecipe|null>(null);
  function dragHistogram(event:HistogramGesture){
    if(event.phase==="start"){beginGesture();histogramStart.current=recipe;return;}
    if(event.phase==="cancel"){histogramStart.current=null;cancelGesture();return;}
    if(event.phase==="end"){histogramStart.current=null;endGesture();return;}
    const start=histogramStart.current;if(!start)return;
    if(event.zone==="exposure")dispatch({type:"advanced",patch:{exposure:Math.max(-5,Math.min(5,(start.advanced?.exposure??0)+event.delta*6))},record:false});
    else dispatch({type:"advanced",patch:{development:{...DEFAULT_DEVELOPMENT,...start.advanced?.development,[event.zone]:Math.max(-100,Math.min(100,(start.advanced?.development?.[event.zone]??0)+event.delta*200))}},record:false});
  }
  const [histogramOpen,setHistogramOpen]=useState(true);
  const histogram=useEditorHistogram(preview.session?.sessionId,recipe,compare,histogramOpen&&!unavailable);
  const readoutRecipe=tool==="crop"?{...recipe,crop:null,textLayers:[],brushStrokes:[]}:recipe;
  const pixel=usePixelReadout(preview.session?.sessionId,compare?{...readoutRecipe,adjustments:undefined,advanced:undefined,legacyAdjustments:undefined,masks:[]}:readoutRecipe,histogramOpen&&!unavailable);
  const [shot,setShot]=useState<{iso?:number|null;aperture?:string|null;shutter?:string|null}|null>(null);
  useEffect(()=>{let cancelled=false;setShot(null);void assetDetail(asset.id).then(value=>{if(!cancelled)setShot(value);}).catch(()=>{});return()=>{cancelled=true;};},[asset.id]);
  const targetSampling={sampleSize,setSampleSize:(size:1|3|5|11)=>{setSampleSize(size);setSample(null);setSamplingError(null);},setTargetCurveChannel:(channel:CurveChannel)=>{setTargetCurveChannel(channel);setSample(null);},targetHslAxis,setTargetHslAxis};
  const schemeDisabled=busy||exporting||libraryChanged||!preview.session||projectLoading||projectError!==null||target.active;
  const scheme=useSchemeTransfer({assetId:asset.id,libraryId,name:asset.name,recipe:recipeForPersist(effective),label:t("advancedEditor.schemeFile"),disabled:schemeDisabled,onBusy:value=>{busyRef.current=value;setBusy(value);},onSaved:()=>setNotice(t("advancedEditor.schemeSaved")),onApply:value=>{chooseTool("adjust");dispatch({type:"applyProject",recipe:value});setSelectedMask(null);setSelectedText(null);preview.reload();setNotice(t("advancedEditor.schemeApplied"));}});

  const canvasRecipe=tool==="crop"?{...recipe,crop:null,textLayers:[],brushStrokes:[]}:recipe;
  const nativeCanvas=useNativeEditorCanvas(preview.session?.sessionId,canvasRecipe,nativeHost,
    (["adjust","view","output","crop","mask"].includes(tool)) && !confirm && !albumPicker && !toolModalOpen && !proofOpen && !referenceOpen && !moreOpen && !scheme.error && !histogram.error && selectedComponent===null,
    compare,preview.session?`${preview.session.sourceUrl}|${!!preview.session.rawlabReady}`:undefined,String(zoomVisible),compareSplit,activeClipping);
  const clippingSrc=useClippingOverlay(preview.session?.sessionId,recipe,false,activeClipping,!nativeCanvas&&!compare,error=>setNotice(String(error)));
  const originalClippingSrc=useClippingOverlay(preview.session?.sessionId,recipe,true,activeClipping,!nativeCanvas&&(compare||compareSplit!==null),error=>setNotice(String(error)));
  useEffect(() => {setNativeActive(nativeCanvas);},[nativeCanvas]);

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  useEffect(() => {
    let cancelled = false;
    void albumList().then((value) => { if (!cancelled) setAlbums(value); }).catch(() => {});
    void assetAlbums(asset.id).then((value) => { if (!cancelled) setSourceAlbums(value); });
    return () => { cancelled = true; };
  }, [asset.id, libraryId]);
  useEffect(() => {
    let cancelled = false;
    let off: (() => void) | null = null;
    void subscribeAppEvents((event) => {
      if (event.type === "exportTaskProgress" && event.jobId === job.current) setExportPhase(event.phase);
      if (event.type === "exportTaskFinished" && event.jobId === job.current) {
        job.current = null;
        setExporting(false);
        setNotice(event.ok ? t("advancedEditor.exported") : event.error ?? t("editor.exportFailed"));
        if (event.ok && returnAfterExport.current) closeEditor();
        returnAfterExport.current = false;
      }
    }).then((value) => { if (cancelled) value(); else off = value; }).catch(() => {});
    return () => { cancelled = true; off?.(); };
  }, [t, onClose]);

  useEffect(() => {
    if (!exporting || libraryChanged) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      const id = job.current;
      if (id === null || cancelled) return;
      try {
        const task = await editExportStatus(asset.id, libraryId, id);
        if (cancelled || job.current !== id) return;
        if(task.phase)setExportPhase(task.phase);
        if (task.status === "done" || task.status === "error" || task.status === "cancelled") {
          job.current = null; setExporting(false);
          setNotice(task.status === "done" ? t("advancedEditor.exported") : task.error ?? t("editor.exportFailed"));
          if (task.status === "done" && returnAfterExport.current) closeEditor();
          returnAfterExport.current = false;
          return;
        }
      } catch { /* 事件通道仍可完成任务；暂时不可读时重试。 */ }
      if (!cancelled) timer = setTimeout(() => void poll(), 1500);
    };
    void poll();
    return () => { cancelled = true; clearTimeout(timer); };
  }, [exporting, libraryChanged, asset.id, libraryId, onClose, t]);

  useEffect(() => {
    let cancelled = false;
    setProjectLoading(true); setProjectError(null);
    void editProjectOpen(asset.id, libraryId).then(async(saved) => {
      if (cancelled) return;
      let loaded = saved ?? advancedRecipe();
      if(!saved&&asset.kind==="raw") {
        const backend=await editRawlabAvailable();
        if(cancelled)return;
        if(!backend.available)throw new Error(t("advancedEditor.baseColor.unavailable"));
        loaded={...loaded,colorPipeline:{version:2,backend:"rawlab",coreRevision:backend.coreRevision,baseProfile:preferences.defaultProfile}};
      }
      dispatch({ type: "loadProject", recipe: loaded }); setBaseline(saved??advancedRecipe());
    }).catch((e: unknown) => { if (!cancelled) setProjectError(String(e)); })
      .finally(() => { if (!cancelled) setProjectLoading(false); });
    return () => { cancelled = true; };
  }, [asset.id, libraryId, projectRetry]);
  useEffect(() => {
    if (!pendingPhoto || busy || exporting || projectLoading) return;
    if (dirty) setConfirm("switch");
    else useAdvancedEditorStore.getState().accept();
  }, [pendingPhoto, dirty, busy, exporting, projectLoading]);
  const previousPipeline=useRef<number|null>(null);
  useEffect(()=>{
    if(projectLoading)return;
    const version=recipe.colorPipeline?.version??0;
    if(previousPipeline.current!==null&&(previousPipeline.current===2)!==(version===2))preview.reload();
    previousPipeline.current=version;
  },[projectLoading,recipe.colorPipeline?.version]);
  useEffect(() => { rootRef.current?.focus(); }, []);
  const requestCloseRef = useRef<() => void>(() => {});
  requestCloseRef.current = requestClose;
  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let release: (() => void) | undefined;
    void getCurrentWindow().onCloseRequested((event) => {
      event.preventDefault(); requestCloseRef.current();
    }).then((off) => { if (disposed) off(); else release = off; });
    return () => { disposed = true; release?.(); };
  }, []);
  useEffect(() => {
    const release = () => setCompare(false);
    window.addEventListener("blur", release);
    window.addEventListener("pointerup", release);
    window.addEventListener("pointercancel", release);
    window.addEventListener("keyup", release);
    return () => { window.removeEventListener("blur", release); window.removeEventListener("pointerup", release); window.removeEventListener("pointercancel", release); window.removeEventListener("keyup", release); };
  }, []);

  function beginGesture() { gesture.current ??= history; }
  function endGesture() {
    if (gesture.current && !recipeEquals(gesture.current.present, recipe)) dispatch({ type: "commitFrom", snapshot: gesture.current.present });
    gesture.current = null;
  }
  function cancelGesture(){const snapshot=gesture.current;gesture.current=null;if(snapshot)dispatch({type:"restoreHistory",history:snapshot});}
  function chooseTool(next: EditorTool) {
    target.cancel();
    endGesture();
    setCompare(false); setPicker(null);setSample(null);
    if (tool === "crop" && next !== "crop") {const crop=cropDraft?clampCrop(cropDraft,1e-6):null;dispatch({type:"cropConfirm",crop:isFullCrop(crop)?null:crop});setCropDraft(null);}
    if (next === "crop"&&tool!=="crop") {dispatch({type:"cropStart"});setCropDraft(recipe.crop ?? { x: 0, y: 0, w: 1, h: 1 });setCropRatio(null);}
    setTool(next);if(!preferences.rightOpen)updatePreferences({rightOpen:true});
  }
  function closeEditor() {
    autosave.cancelPending();
    busyRef.current = true; setBusy(true);
    void preview.close().finally(onClose);
  }
  function cancelCrop(){gesture.current=null;dispatch({type:"cropCancel"});setCropDraft(null);setCropRatio(null);setStraightening(false);setTool("adjust");}
  function undoEditing(){if(tool==="crop")cancelCrop();else{endGesture();dispatch({type:"undo"});}}
  function redoEditing(){if(tool==="crop")cancelCrop();else dispatch({type:"redo"});}
  function requestClose() {if(exporting){setNotice(t("editor.exportCloseHint"));return;}if(target.active){closeAfterTarget.current=true;target.cancel();return;} if (!toolModalOpen && !busyRef.current && !exporting) dirty ? setConfirm("close") : closeEditor(); }
  useEffect(()=>{if(!target.active&&closeAfterTarget.current){closeAfterTarget.current=false;requestClose();}},[target.active]);

  async function saveProject(): Promise<boolean> {
    if (busyRef.current || unavailable) return false;
    busyRef.current = true; setBusy(true);
    const snapshot = recipeForPersist(effective);
    try {
      if(!await autosave.save(snapshot))return false;
      if (!mounted.current) return false;
      if (tool === "crop" && cropDraft) { dispatch({ type: "cropConfirm", crop: snapshot.crop }); setCropDraft(null); setTool("adjust"); }
      setBaseline(snapshot); setNotice(t("advancedEditor.saved"));
      return true;
    } catch (e: unknown) { if (mounted.current) setNotice(String(e)); return false; }
    finally { busyRef.current = false; if (mounted.current) setBusy(false); }
  }
  async function upgradeColor() {
    try {
      const backend=await editRawlabAvailable();
      if(!backend.available)throw new Error(t("advancedEditor.baseColor.unavailable"));
      if(!await saveProject())return;
      await editVersionCreate(asset.id,libraryId,"snapshot",t("advancedEditor.versions.beforeUpgrade"),recipeForPersist(recipe));
      dispatch({type:"colorPipeline",value:{...recipe.colorPipeline,version:2,backend:"rawlab",coreRevision:backend.coreRevision,baseProfile:preferences.defaultProfile}});
    } catch(error){setNotice(String(error));}
  }
  async function autoLevels(){
    if(unavailable||busyRef.current||!preview.session)return;busyRef.current=true;setBusy(true);
    try{const levels=await editPreviewAutoLevels(preview.session.sessionId,recipe);if(mounted.current)dispatch({type:"advanced",patch:{levels}});}
    catch(error){setNotice(String(error));}finally{busyRef.current=false;if(mounted.current)setBusy(false);}
  }
  function changeProfile(value:BaseProfile){
    if(recipe.colorPipeline?.version===2)dispatch({type:"colorPipeline",value:{...recipe.colorPipeline,baseProfile:value}});
  }
  function chooseZoom(mode:"fit"|"100"|"200") {
    setZoomMode(mode);setZoom(mode==="fit"?1:(mode==="100"?1:2)/Math.max(fitScale,0.0001));wakeZoom();
  }
  useEffect(()=>{if(zoomMode!=="custom")setZoom(zoomMode==="fit"?1:(zoomMode==="100"?1:2)/Math.max(fitScale,0.0001));},[fitScale,zoomMode]);
  async function toggleFullscreen(){
    try {if(isTauri()){const win=getCurrentWindow();const value=!await win.isFullscreen();await win.setFullscreen(value);setFullscreen(value);}}
    catch(error){setNotice(String(error));}
  }
  async function returnToLibrary(albumId = originAlbumId ?? (sourceAlbums.length === 1 ? sourceAlbums[0].id : undefined), subgroup: string | null = originSubgroup??null, closeAfter = false) {
    if (exporting || busyRef.current || unavailable) return;
    if (albumId === undefined) {
      albumCloseAfter.current = closeAfter; setAlbumPicker(true); return;
    }
    if(!await saveProject()||!mounted.current)return;
    returnAfterExport.current=closeAfter;
    await startExport({mode:"album",album:{albumId:String(albumId),subgroup}});
  }
  async function startExport(target: Pick<ExportOptions, "mode" | "folder" | "album">) {
    if (busyRef.current || exporting || unavailable) return;
    busyRef.current = true; setBusy(true);
    try {
      const result = await advancedExportFolder({ assetId: asset.id, libraryId, sessionId: preview.session!.sessionId }, recipeForPersist(effective), {
        ...target,format:exportFormat,colorSpace:exportColorSpace, longEdge: recipe.output.longEdge, quality: recipe.output.quality,
        ...exportMetadata(exportSettings),
      });
      if (!mounted.current) return;
      job.current = result.id; setExportPhase("queued");setExporting(true); setNotice(t("advancedEditor.exporting"));
    } catch (e: unknown) { if (mounted.current) setNotice(String(e)); }
    finally { busyRef.current = false; if (mounted.current) setBusy(false); }
  }
  async function exportFolder() {
    if (busyRef.current || exporting) return;
    try {
      const path = await open({ directory: true, multiple: false });
      if (typeof path === "string" && mounted.current) await startExport({ mode: "folder",
        folder: { outputDir: path, fileName: exportName(asset.name,exportFormat,exportSettings) } });
    } catch (e: unknown) { if (mounted.current) setNotice(String(e)); }
  }

  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return;
      if (maskGestureOwnsKey(event)) return;
      if(toolModalOpen||proofOpen||referenceOpen||moreOpen||scheme.error||histogram.error) return;
      if(straightening&&(event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==="z"){event.preventDefault();event.stopImmediatePropagation();setStraightening(false);return;}
      if(target.active&&(event.key==="Escape"||((event.ctrlKey||event.metaKey)&&["z","s"].includes(event.key.toLowerCase())))){event.preventDefault();event.stopImmediatePropagation();if(event.key.toLowerCase()!=="s")target.cancel();return;}
      const input = event.target instanceof HTMLElement && (event.target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(event.target.tagName));
      if (input) return;
      if(tool==="mask"&&event.key.toLowerCase()==="o"&&!event.ctrlKey&&!event.metaKey){event.preventDefault();setShowMaskOverlay(v=>!v);return;}
      if(event.key==="Escape"&&gesture.current&&tool!=="crop"){event.preventDefault();event.stopImmediatePropagation();cancelGesture();return;}
      if (event.key === "\\" && !event.ctrlKey && !event.metaKey && !unavailable) { event.preventDefault(); setCompare(true); return; }
      if (event.key === "Escape") {
        event.preventDefault(); event.stopImmediatePropagation();
        if(straightening)setStraightening(false);
        else if (picker) setPicker(null);
        else if(tool==="crop")cancelCrop();
        else if (confirm) { setConfirm(null); useAdvancedEditorStore.getState().cancel(); }
        else if (albumPicker) setAlbumPicker(false);
        else if (splitEnabled) setSplitEnabled(false);
        else requestClose();
      } else if (!busyRef.current && !exporting && !unavailable && (event.metaKey || event.ctrlKey)) {
        if (event.key.toLowerCase() === "s") { event.preventDefault(); void saveProject(); }
        if (event.key.toLowerCase() === "z") { event.preventDefault(); event.shiftKey?redoEditing():undoEditing(); }
      }
    };
    window.addEventListener("keydown", key, true);
    return () => window.removeEventListener("keydown", key, true);
  });

  return <div ref={rootRef} tabIndex={-1} role="dialog" aria-modal="true" aria-label={t("advancedEditor.title")} className="fixed inset-0 flex flex-col bg-bg outline-none" data-testid="advanced-editor-overlay"
    onDragOver={(e) => { if (e.dataTransfer.types.includes(ASSET_DRAG_TYPE)) { e.preventDefault(); e.dataTransfer.dropEffect = "copy"; } }}
    onDrop={(e) => {
      e.preventDefault();
      if (busy || exporting || projectLoading) return;
      try {
        const raw = e.dataTransfer.getData(ASSET_DRAG_TYPE);
        if (!raw || raw.length > 32768) return;
        const photo = JSON.parse(raw) as EditorPhoto;
        if (photo.libraryId === libraryId && Number.isSafeInteger(photo.asset?.id) && photo.asset.id > 0) useAdvancedEditorStore.getState().request(photo);
      } catch { setNotice(t("advancedEditor.dropFailed")); }
    }}>
    <header className="ui-glass relative z-50 flex min-h-14 shrink-0 flex-wrap items-center gap-2 border-b border-edge px-4 py-2">
      <button className={BUTTON} disabled={busy||exporting} onClick={requestClose}>{t("advancedEditor.returnLibrary")}</button>
      <div className="min-w-0"><h2 className="text-sm font-semibold text-text-primary">{t("advancedEditor.title")} {dirty && <span className="text-accent">•</span>}</h2><p className="truncate text-[11px] text-text-muted">{asset.name} · <span role="status">{t(autosave.status==="saving"?"advancedEditor.autosave.saving":autosave.status==="error"?"advancedEditor.autosave.failed":dirty?"advancedEditor.autosave.pending":"advancedEditor.autosave.saved")}</span></p></div>
      <div className="ml-auto flex flex-wrap items-center justify-end gap-2">
        <button className={BUTTON} disabled={(!history.past.length&&tool!=="crop") || busy || exporting} onClick={undoEditing} title={t("editor.undo")} aria-label={t("editor.undo")}>↶</button>
        <button className={BUTTON} disabled={!history.future.length || tool==="crop" || busy || exporting} onClick={redoEditing} title={t("editor.redo")} aria-label={t("editor.redo")}>↷</button>
        <button className={BUTTON} disabled={busy || unavailable} aria-pressed={compare} title={t("advancedEditor.compareHold")} onPointerDown={(event) => { if (event.button !== 0) return; event.currentTarget.setPointerCapture(event.pointerId); setCompare(true); }} onPointerUp={() => setCompare(false)} onPointerCancel={() => setCompare(false)} onLostPointerCapture={() => setCompare(false)} onBlur={() => setCompare(false)} onKeyDown={(event) => { if (event.key === " " || event.key === "Enter") { event.preventDefault(); setCompare(true); } }} onKeyUp={() => setCompare(false)}>{t("advancedEditor.compare")}</button>
        <button className={BUTTON} disabled={busy||exporting||unavailable||target.active||tool==="crop"} onClick={()=>void autoLevels()}>{t("advancedEditor.autoLevels")}</button>
        <button className={BUTTON} disabled={busy || unavailable || target.active} onClick={() => void saveProject()}>{t("advancedEditor.saveChanges")}</button>
        <button className={BUTTON} disabled={busy || unavailable || target.active} aria-pressed={splitEnabled} title={t("advancedEditor.compareSplitHint")} onClick={()=>{setPicker(null);setSplitEnabled(v=>!v);}}>{t("advancedEditor.compareSplit")}</button>
        <button className="ui-primary rounded-xl px-4 py-2 text-xs disabled:opacity-40" disabled={busy || exporting || unavailable || target.active} onClick={() => chooseTool("output")}>{t("editor.export")}</button>

        <ActionPopover label={t("ui.more")} onOpenChange={setMoreOpen} closeOnAction>
          <button className={BUTTON} disabled={busy||exporting||unavailable||target.active} onClick={()=>void scheme.save()}>{t("advancedEditor.schemeSave")}</button>
          <button className={BUTTON} disabled={schemeDisabled} onClick={()=>void scheme.open()}>{t("advancedEditor.schemeOpen")}</button>
          {([1,2] as const).map(flag=><button key={flag} className={BUTTON} disabled={busy||unavailable} aria-pressed={(clipping&flag)!==0} onClick={()=>setClipping(value=>value^flag)}>{t(flag===1?"advancedEditor.clipShadows":"advancedEditor.clipHighlights")}</button>)}
          <button className={BUTTON} disabled={unavailable||busy||exporting||target.active} onClick={()=>setReferenceOpen(true)}>{t("advancedEditor.reference.title")}</button>
          <button className={BUTTON} disabled={unavailable||busy||exporting||target.active} onClick={()=>setProofOpen(true)}>{t("advancedEditor.proof.title")}</button>
          <button className={BUTTON} aria-pressed={fullscreen} onClick={()=>void toggleFullscreen()}>{t("advancedEditor.layout.fullscreen")}</button>
          <button className={BUTTON} onClick={()=>updatePreferences({leftOpen:false,rightOpen:false})}>{t("advancedEditor.layout.hidePanels")}</button>
          <label className="text-xs">{t("advancedEditor.layout.background")}<select value={preferences.background} onChange={event=>updatePreferences({background:event.target.value as typeof preferences.background})} className="ml-2 rounded border border-edge bg-bg p-1">{["#101010","#202020","#404040"].map(color=><option key={color} value={color}>{color}</option>)}</select></label>
          <button className={BUTTON} disabled={busy || exporting} onClick={() => setConfirm("reset")}>{t("editor.reset")}</button>
        </ActionPopover>
        <button className={BUTTON} aria-pressed={preferences.leftOpen} onClick={()=>updatePreferences({leftOpen:!preferences.leftOpen})}>{t("advancedEditor.layout.left")}</button>
        <button className={BUTTON} aria-pressed={preferences.rightOpen} onClick={()=>updatePreferences({rightOpen:!preferences.rightOpen})}>{t("advancedEditor.layout.right")}</button>
      </div>
    </header>
    {exporting&&job.current!==null&&<EditorExportProgress key={job.current} assetId={asset.id} libraryId={libraryId} jobId={job.current} phase={exportPhase} onError={setNotice}/>}
    <div className="flex min-h-0 flex-1 gap-2 p-2">
      <nav role="tablist" aria-orientation="vertical" aria-label={t("editor.tools")} className="ui-glass flex w-14 shrink-0 flex-col items-center gap-1 rounded-2xl py-2">
        {TOOLS.map(({ id, label, icon }) => <button key={id} role="tab" aria-selected={tool === id} aria-label={t(label)} title={t(label)} disabled={busy || exporting || unavailable} onClick={() => chooseTool(id)} className={`flex h-11 w-11 items-center justify-center rounded-xl transition-colors ${tool === id ? "bg-accent/15 text-accent" : "text-text-secondary hover:bg-panel"}`}><svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={icon} /></svg></button>)}
      </nav>
      {preferences.leftOpen&&<aside aria-label={t("advancedEditor.layout.left")} style={{width:preferences.leftWidth}} className="ui-glass sp-scroll shrink-0 space-y-4 overflow-y-auto rounded-2xl p-3">
        <div className="flex gap-2">{(["profiles","history","versions"] as const).map(tab=><button key={tab} className={BUTTON} aria-pressed={leftTab===tab} onClick={()=>setLeftTab(tab)}>{t(`advancedEditor.sidebar.${tab}`)}</button>)}</div>
        {leftTab==="profiles"?<BaseProfilePanel recipe={recipe} isRaw={asset.kind==="raw"} defaultProfile={preferences.defaultProfile}
          begin={beginGesture} end={endGesture} rawReady={!!preview.session?.rawlabReady&&!preview.preparingWhiteBalance} onWhiteBalance={(rawWhiteBalance,record)=>{if(recipe.colorPipeline)dispatch({type:"colorPipeline",value:{...recipe.colorPipeline,rawWhiteBalance,relativeWhiteBalance:undefined},record});}} onWhiteBalancePicker={()=>{chooseTool("adjust");setPicker("rawWhiteBalance");}} onDefault={defaultProfile=>updatePreferences({defaultProfile})} onChange={changeProfile} onUpgrade={()=>void upgradeColor()} disabled={busy||exporting||unavailable||tool==="crop"||target.active}/>
          :leftTab==="versions"?<EditorVersionsPanel onBusy={value=>{busyRef.current=value;setBusy(value);}} assetId={asset.id} libraryId={libraryId} recipe={recipeForPersist(effective)} revision={baseline} disabled={busy||exporting||unavailable||tool==="crop"||target.active} save={saveProject} onApply={(value,branch)=>{dispatch({type:branch?"loadProject":"applyProject",recipe:value});setBaseline(value);setSelectedMask(null);setSelectedText(null);}}/>:<><p className="text-[11px] text-text-muted">{t("advancedEditor.sidebar.historyHint")}</p><button className={`${BUTTON} w-full`} disabled={unavailable||busy||target.active||tool==="crop"||!history.past.length} onClick={()=>dispatch({type:"restoreHistory",history:{past:[],present:history.past[0],future:[...history.past.slice(1),recipe,...history.future]}})}>{t("advancedEditor.sidebar.initial")}</button>
          {history.past.map((_,index)=><button key={index} disabled={unavailable||busy||target.active||tool==="crop"} className={`${BUTTON} w-full`} onClick={()=>dispatch({type:"restoreHistory",history:{past:history.past.slice(0,index+1),present:history.past[index+1]??recipe,future:[...history.past.slice(index+2),...(index+1<history.past.length?[recipe]:[]),...history.future]}})}>{t("advancedEditor.sidebar.step",{count:index+1})}</button>)}</>}
        <label className="block text-[11px] text-text-muted">{t("advancedEditor.layout.width")}<input aria-label={t("advancedEditor.layout.leftWidth")} type="range" min={180} max={340} value={preferences.leftWidth} onChange={event=>updatePreferences({leftWidth:Number(event.target.value)})} className="w-full accent-accent"/></label>
      </aside>}
      <main ref={nativeHost} data-native-canvas-host className="relative flex min-h-0 min-w-0 flex-1 overflow-hidden rounded-2xl bg-[#202020]" style={{backgroundColor:preferences.background}} data-theme="dark">
        {libraryChanged ? <div role="alert" className="m-auto max-w-sm p-5 text-center text-sm text-text-secondary">{t("advancedEditor.libraryChanged")}</div> : projectError ? <div role="alert" className="m-auto max-w-sm space-y-3 p-5 text-center text-sm text-text-secondary"><p>{projectError}</p><button className={BUTTON} onClick={() => setProjectRetry((v) => v + 1)}>{t("editor.reload")}</button></div> : preview.error ? <div role="alert" className="m-auto max-w-sm space-y-3 p-5 text-center text-sm text-text-secondary"><p>{preview.error}</p><button className={BUTTON} onClick={preview.reload}>{t("editor.reload")}</button></div> : <EditorCanvas
          onFitScale={setFitScale}
          onHoverPixel={pixel.hover}
          compareSplit={compareSplit} onCompareSplit={setSplitPosition} nativeDisplay={nativeCanvas}
          clippingSrc={clippingSrc} originalClippingSrc={originalClippingSrc}
          sampleMode={picker !== null && !compare && compareSplit===null} onSample={samplePhoto} onSampleError={()=>setSamplingError(t("advancedEditor.sampleOutside"))}
          onSampleGesture={!unavailable&&!compare&&(picker==="curveTarget"||picker==="hslTarget")?target.onGesture:undefined}
          src={preview.session?.sourceUrl ?? null} adjustedSrc={preview.url} showOriginal={compare}
          sourceSize={sourceSize} nativeGeometry nativeAnnotations backendAdjustments fallbackSize={sourceSize} zoom={zoom}
          onZoom={(factor) => {wakeZoom();setZoomMode("custom");setZoom((v) => Math.max(0.1, Math.min(Math.max(4,2/Math.max(fitScale,0.0001)), v * factor)));}}
          recipe={compare ? { ...recipe, textLayers: [], brushStrokes: [] } : recipe}
          tool={compare || busy || exporting || unavailable ? "view" : tool==="mask"&&!maskEditing?"adjust":tool} cropRatio={cropRatio} cropDraft={cropDraft} onCropDraftChange={setCropDraft}
          maskControls={tool==="mask"&&activeMask&&!compare?{editing:maskEditing,mask:selectedComponent===null?activeMask:activeMask.components?.[selectedComponent]?.mask??activeMask,brush:maskBrush,overlaySrc:maskOverlay,showOverlay:showMaskOverlay,onCommit:patch=>dispatch({type:"maskUpdate",id:activeMask.id,patch:selectedComponent===null?patch:{components:activeMask.components?.map((component,index)=>index===selectedComponent?{...component,mask:{...component.mask,...patch}}:component)}})}:undefined}
          straightening={straightening} onStraightenCancel={()=>setStraightening(false)} onStraighten={delta=>{setStraightening(false);if(Math.abs(delta)<1e-5)return;const angle=(((recipe.geometry?.angle??0)+delta+180)%360+360)%360-180;const next=changeGeometry(recipe,{angle},context.current);setCropDraft(remapDraftCrop(recipe,next,cropDraft,context.current,cropRatio)??{x:0,y:0,w:1,h:1});dispatch({type:"geometry",patch:{angle}});}}
          brushOptions={{ color, widthRel: brushWidth }} selectedTextId={selectedText} onSelectText={setSelectedText}
          onPlaceText={(pos) => { const id = newLayerId(); dispatch({ type: "textAdd", layer: { id, ...pos, text: "", sizeRel: 0.05, color } }); setSelectedText(id); }}
          onTextChange={(id, patch) => dispatch({ type: "textUpdateLive", id, patch })}
          onStrokeCommit={(points) => dispatch({ type: "strokeAdd", stroke: { id: newLayerId(), points, color, widthRel: brushWidth } })}
          onGestureStart={beginGesture} onGestureEnd={endGesture} onImageReady={() => {}} onImageError={preview.reload} />}
        {asset.kind==="raw"&&recipe.colorPipeline&&(!preview.session?.rawlabReady||preview.preparingWhiteBalance||!!preview.warning)&&<div data-native-overlay className="pointer-events-auto absolute right-3 top-3 z-40 flex max-w-[min(28rem,85%)] items-center gap-2 rounded-lg bg-black/70 px-3 py-2 text-[11px] text-white/80" role="status" aria-live="polite">
          {preview.warning?<><span>{t("advancedEditor.rawPreview.failed")}</span><button className="underline" onClick={preview.reload}>{t("editor.reload")}</button></>:<><span aria-hidden="true" className="h-3 w-3 shrink-0 animate-spin rounded-full border border-white/25 border-t-white/90"/><span>{t("advancedEditor.rawPreview.loading")}</span></>}
        </div>}
        <CanvasNavigator host={nativeHost} src={preview.url??preview.session?.sourceUrl}/>
        <div data-native-overlay data-visible={zoomVisible} data-testid="editor-zoom-hud" aria-hidden={!zoomVisible} inert={!zoomVisible} className={`absolute bottom-4 left-1/2 z-40 -translate-x-1/2 flex items-center gap-1 rounded-2xl border border-white/10 bg-[#202020] p-1.5 shadow-xl backdrop-blur-xl transition-opacity ${zoomVisible ? "opacity-100" : "pointer-events-none opacity-0"}`}>
          <button className="h-8 w-8 rounded-lg text-text-secondary hover:bg-panel" aria-label={t("editor.zoomOut")} onClick={() => {wakeZoom();setZoomMode("custom");setZoom((v) => Math.max(0.1, v / 1.25));}}>−</button>
          <button className="h-8 min-w-16 rounded-lg text-xs tabular-nums text-text-secondary hover:bg-panel" title={t("editor.zoomFit")} onClick={() => chooseZoom("fit")}>{zoomMode==="fit"?t("editor.zoomFit"):`${Math.round(zoom * fitScale * 100)}%`}</button><button className={BUTTON} onClick={()=>chooseZoom("100")}>100%</button><button className={BUTTON} onClick={()=>chooseZoom("200")}>200%</button>
          <button className="h-8 w-8 rounded-lg text-text-secondary hover:bg-panel" aria-label={t("editor.zoomIn")} onClick={() => {wakeZoom();setZoomMode("custom");setZoom((v) => Math.min(Math.max(4,2/Math.max(fitScale,0.0001)), v * 1.25));}}>+</button>
          {preview.pending && <span role="status" aria-label={t("advancedEditor.previewUpdating")} className="mx-2 h-4 w-4 animate-spin rounded-full border border-white/20 border-t-accent" />}
        </div>
        {(preview.loading || projectLoading) && <div className="pointer-events-none absolute inset-0 flex items-center justify-center" role="status" aria-label={t("advancedEditor.loading")}><span className="h-8 w-8 animate-spin rounded-full border-2 border-white/20 border-t-accent" /></div>}

      </main>
      {preferences.rightOpen&&<aside style={{width:preferences.rightWidth}} className="ui-glass sp-scroll shrink-0 overflow-y-auto rounded-2xl p-4">
        <label className="mb-3 block text-[11px] text-text-muted">{t("advancedEditor.layout.width")}<input aria-label={t("advancedEditor.layout.rightWidth")} type="range" min={280} max={480} value={preferences.rightWidth} onChange={event=>updatePreferences({rightWidth:Number(event.target.value)})} className="w-full accent-accent"/></label>
        <HistogramPanel onToneGesture={!unavailable&&!busy&&!exporting&&!compare&&!target.active&&tool!=="crop"?dragHistogram:undefined} bins={histogram.bins} onOpen={setHistogramOpen}>
          <div className="flex justify-between gap-2 text-xs tabular-nums text-text-secondary"><span>{shot?.aperture?`f/${shot.aperture.replace(/^f\/?/i, "")}`:"f/—"}</span><span>{formatEditorShutter(shot?.shutter)}</span><span>ISO {shot?.iso&&shot.iso>0?shot.iso:"—"}</span></div>
          <div className="flex justify-between gap-1 whitespace-nowrap text-[11px] tabular-nums text-text-muted" aria-live="off"><span>RGB {pixel.rgb?.map(v=>Math.round(v)).join(" · ")??"— · — · —"}</span><span>Lab {pixel.lab?.map(v=>v.toFixed(1)).join(" · ")??"— · — · —"}</span></div>
        </HistogramPanel>
        {samplingError && <p role="alert" className="mb-2 text-xs text-amber-500">{samplingError}</p>}
        {preview.session?.warnings.map((warning) => <p key={warning} role="status" className="mb-2 text-xs text-amber-500">{warning}</p>)}
        <fieldset disabled={busy || exporting || compare || unavailable || target.active} className="space-y-3">
          <h3 className="text-sm font-semibold text-text-primary">{t(TOOLS.find((v) => v.id === tool)?.label ?? "advancedEditor.title")}</h3>
          {tool==="mask"?<MaskPanel sessionId={preview.session?.sessionId} selectedComponent={selectedComponent} onSelectComponent={setSelectedComponent} cancel={cancelGesture} recipe={recipe} selected={selectedMask} onSelect={id=>{setSelectedMask(id);setPicker(null);setSample(null);setSamplingError(null);}} editing={maskEditing} onEditing={value=>{setMaskEditing(value);setPicker(null);setSample(null);}} brush={maskBrush} onBrush={setMaskBrush} dispatch={dispatch} begin={beginGesture} end={endGesture} onModalChange={setToolModalOpen}
            showOverlay={showMaskOverlay} onShowOverlay={setShowMaskOverlay} sampling={{...targetSampling,picker,setPicker,sample,hslRange,busy:samplingBusy||target.busy,histogram:[]}}/>:<AdvancedToolPanel exportExtras={<EditorExportSettings value={exportSettings} onChange={setExportSettings}/>} cancelGesture={cancelGesture} straightening={straightening} setStraightening={setStraightening} onModalChange={setToolModalOpen} sampling={{...targetSampling, picker, setPicker, sample, hslRange, busy:samplingBusy||target.busy, histogram: preview.session?.histogram ?? [] }} tool={tool} recipe={recipe} dispatch={dispatch} beginGesture={beginGesture} endGesture={endGesture}
            context={context} cropDraft={cropDraft} setCropDraft={setCropDraft} setTool={setTool} onCancelCrop={cancelCrop} cropRatio={cropRatio} setCropRatio={setCropRatio}
            color={color} setColor={setColor} selectedText={selectedText} setSelectedText={setSelectedText}
            brushWidth={brushWidth} setBrushWidth={setBrushWidth} exporting={exporting} exportAlbum={()=>{albumCloseAfter.current=false;setAlbumPicker(true);}} exportFolder={exportFolder} exportFormat={exportFormat} setExportFormat={setExportFormat} exportColorSpace={exportColorSpace} setExportColorSpace={setExportColorSpace} />}
        </fieldset>
      </aside>}
    </div>
    {autosave.error&&<div role="alert" className="flex items-center justify-between gap-2 px-4 py-2 text-xs text-amber-500"><span>{autosave.error}</span><button className={BUTTON} onClick={()=>void saveProject()}>{t("advancedEditor.autosave.retry")}</button></div>}
    <EditorFilmstrip savedRevision={baseline} asset={asset} recipe={recipeForPersist(effective)} disabled={busy||exporting||unavailable||target.active||!!confirm||toolModalOpen} save={saveProject} onApply={value=>{chooseTool("adjust");dispatch({type:"applyProject",recipe:value});}}/>
    <footer className="flex h-8 shrink-0 items-center gap-3 px-4 text-[11px] text-text-muted"><span>{sourceSize ? `${sourceSize.width} × ${sourceSize.height}` : t("advancedEditor.loading")}</span><span className="truncate" role="status">{notice ?? t("advancedEditor.projectHint")}</span>{preview.session?.rawlabReady&&<span className="ml-auto truncate" title={t("advancedEditor.preview.proxy")}>{t("advancedEditor.preview.rawReady")} · {t("advancedEditor.preview.proxy")}</span>}</footer>
    {referenceOpen&&<EditorReferenceDialog asset={asset} libraryId={libraryId} recipe={recipe} current={preview.url??preview.session?.sourceUrl??null} quick={preview.quickUrl} onClose={()=>setReferenceOpen(false)}/>}
    {proofOpen&&preview.session&&<EditorProofDialog sessionId={preview.session.sessionId} assetId={asset.id} libraryId={libraryId} recipe={recipe} original={preview.url??preview.session.sourceUrl} onClose={()=>setProofOpen(false)}/>}
    <ErrorModal message={scheme.error??histogram.error} onClose={scheme.error?scheme.closeError:histogram.closeError}/>
    {albumPicker && <ExportAlbumPicker albums={albums} onCancel={() => setAlbumPicker(false)} onConfirm={(albumId, subgroup) => { setAlbumPicker(false); void returnToLibrary(albumId, subgroup, albumCloseAfter.current); }} />}
    {confirm && <div className="fixed inset-0 z-[90] flex items-center justify-center bg-black/50"><div role="alertdialog" aria-modal="true" aria-labelledby="advanced-confirm-title" className="ui-glass w-96 space-y-4 rounded-2xl p-5"><h3 id="advanced-confirm-title" className="text-sm font-semibold text-text-primary">{t(confirm === "reset" ? "editor.resetConfirmTitle" : "editor.unsavedTitle")}</h3><p className="text-xs text-text-secondary">{t(confirm === "reset" ? "editor.resetConfirmDesc" : "advancedEditor.unsaved")}</p><div className="flex justify-end gap-2"><button className={BUTTON} onClick={() => { setConfirm(null); useAdvancedEditorStore.getState().cancel(); }}>{t("common.cancel")}</button>{confirm !== "reset" && <button className="ui-primary rounded-xl px-4 py-2 text-xs" disabled={busy || unavailable} onClick={() => { const action = confirm; void saveProject().then((ok) => { if (!ok || !mounted.current) return; setConfirm(null); if (action === "close") closeEditor(); else useAdvancedEditorStore.getState().accept(); }); }}>{t("advancedEditor.saveContinue")}</button>}<button className={BUTTON} disabled={busy} onClick={() => { const action = confirm; setConfirm(null); if (action === "close") closeEditor(); else if (action === "switch") useAdvancedEditorStore.getState().accept(); else { dispatch({ type: "reset" }); setProjectError(null); gesture.current = null; setCropDraft(null); setSelectedText(null); setTool("adjust"); } }}>{t(confirm === "reset" ? "editor.resetConfirmGo" : "editor.unsavedDiscard")}</button></div></div></div>}
  </div>;
}

function formatEditorShutter(value?:string|null){if(!value)return "—";const plain=value.trim().replace(/s$/i, "");const seconds=Number(plain);if(Number.isFinite(seconds)&&seconds>0)return seconds<1?`1/${Math.round(1/seconds)}s`:`${plain}s`;return `${plain}s`;}
