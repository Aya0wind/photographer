import LensCanvasControls from "./LensCanvasControls";
import type {LensGuide} from "../lib/lensGuides";
import RetouchCanvasControls from "./RetouchCanvasControls";
import type {RetouchBrush} from "./RetouchPanel";
import type {EditRetouchStroke} from "@/ipc/api";
import {geometrySize,planeToSource,sourceToPlane} from "../lib/geometry";
import ComparisonOverlay from "./ComparisonOverlay";
import MaskCanvasControls,{type MaskBrush} from "./MaskCanvasControls";
import StraightenOverlay from "./StraightenOverlay";
import type {SampleGesture} from "../lib/useTargetAdjustment";
import { useEffect, useMemo, useRef, useState } from "react";

import { Circle, Group, Image as KonvaImage, Layer, Line, Rect, Stage, Text as KonvaText, Transformer } from "react-konva";
import type Konva from "konva";

import type { EditRecipe, EditRecipeCrop,EditLocalMask } from "@/ipc/api";
import { type TextLayerPatch } from "../lib/recipe";
import { clamp01, type Size } from "../lib/coords";
import { getPreviewImage } from "../lib/previewImages";
import {
  bakeTextScale,
  cropToRectAttrs,
  linePointsToNorm,
  rectAttrsToCrop,
  strokeWidthPx,
  strokeToLinePoints,
  textLayerToNodeAttrs,
} from "../lib/konvaMapping";

/**
 * 编辑器 Konva 交互层（react-konva；recipe JSON 是唯一真理源，本组件不持有编辑状态）：
 * - 底图：Konva.Image + rotateQuarter 步进旋转变换（不用 Konva 自由角度）；
 * - 画笔：官方 Free Drawing 模式（Stage 采集 pointer → Line 折线，抬起时归一化提交）；
 * - 文字：Konva Text（锚点=左上角左对齐，与契约一致）+ Transformer 拖动/角点缩放
 *   （缩放收尾烘焙进字号 sizeRel）；双击进入官方 demo 式 textarea 覆盖编辑；
 * - 裁剪：Rect + Transformer（预设 keepRatio，自由模式全锚点），暗幕四矩形；
 * - 所有提交都经 konvaMapping 换算回归一化 recipe 字段。
 */

export type EditorTool = "lens" | "retouch" | "view" | "crop" | "text" | "brush" | "mask" | "adjust" | "filters" | "metadata" | "output";

const TEXT_NODE_NAME = "editor-text-layer";
const STAGE_PADDING = 6;

/** HTMLImageElement 加载（react-konva 官方 use-image 的等价内联实现，少一个依赖） */
function useHtmlImage(
  src: string | null,
  onError: () => void,
): HTMLImageElement | null {
  const [image, setImage] = useState<HTMLImageElement | null>(null);
  const errorRef = useRef(onError);
  errorRef.current = onError;
  useEffect(() => {
    if (src === null) {
      setImage(null);
      return;
    }
    const ready = getPreviewImage(src);
    if (ready) { setImage(ready); return; }
    let cancelled = false;
    const img = new Image();
    img.onload = () => {
      if (!cancelled) setImage(img);
    };
    img.onerror = () => {
      if (!cancelled) {
        setImage(null);
        errorRef.current();
      }
    };
    img.src = src;
    return () => {
      cancelled = true;
      img.onload = null;
      img.onerror = null;
      img.removeAttribute?.("src");
    };
  }, [src]);
  return image;
}

/** 旋转后底图的 Konva.Image 属性（绕左上原点顺时针；q 步进表） */
function rotatedImageAttrs(
  baseW: number,
  baseH: number,
  quarter: number,
  scale: number,
): { x: number; y: number; width: number; height: number; rotation: number } {
  const w = baseW * scale;
  const h = baseH * scale;
  switch (((quarter % 4) + 4) % 4) {
    case 1:
      return { x: h, y: 0, width: w, height: h, rotation: 90 };
    case 2:
      return { x: w, y: h, width: w, height: h, rotation: 180 };
    case 3:
      return { x: 0, y: w, width: w, height: h, rotation: 270 };
    default:
      return { x: 0, y: 0, width: w, height: h, rotation: 0 };
  }
}

interface EditorCanvasProps {
  straightening?:boolean;onStraighten?:(angle:number)=>void;onStraightenCancel?:()=>void;
  onSampleGesture?:(event:SampleGesture)=>void;
  lensControls?:{axis:LensGuide["axis"]|null;lines:LensGuide[];grid:boolean;onCommit:(line:LensGuide)=>void};
  retouchControls?:{brush:RetouchBrush;onBrush:(value:RetouchBrush)=>void;onCommit:(value:EditRetouchStroke)=>void};
  maskControls?:{editing?:boolean;mask:EditLocalMask;brush:MaskBrush;onCommit:(patch:Partial<EditLocalMask>)=>void;overlaySrc?:string|null;showOverlay?:boolean};
  showOriginal?: boolean;
  compareSplit?:number|null;
  onCompareSplit?:(value:number)=>void;
  nativeDisplay?:boolean;
  clippingSrc?:string|null;
  originalClippingSrc?:string|null;
  sampleMode?: boolean;
  onHoverPixel?: (point:[number,number]|null)=>void;
  onSample?: (x: number, y: number) => void;
  onSampleError?:()=>void;
  src: string | null;
  /** 后端提供未旋转底图的调整结果；禁止再次叠加浏览器滤镜。 */
  adjustedSrc?: string | null;
  sourceSize?: Size | null;
  backendAdjustments?: boolean;
  nativeGeometry?: boolean;
  nativeAnnotations?: boolean;
  zoom?: number;
  onZoom?: (factor: number) => void;
  onFitScale?:(value:number)=>void;
  /** naturalWidth/Height 为 0 时的兜底尺寸（EXIF 宽高；jsdom 测试路径） */
  fallbackSize: Size | null;
  recipe: EditRecipe;
  tool: EditorTool;
  brushOptions: { color: string; widthRel: number };
  /** 裁剪预设比例（keepRatio）；null=自由 */
  cropRatio: number | null;
  cropDraft: EditRecipeCrop | null;
  onCropDraftChange: (rect: EditRecipeCrop) => void;
  selectedTextId: string | null;
  onSelectText: (id: string | null) => void;
  /** 画布归一化坐标放置文字 */
  onPlaceText: (pos: { x: number; y: number }) => void;
  onTextChange: (id: string, patch: TextLayerPatch) => void;
  /** 画笔一笔提交（归一化点列；颜色/粗细取 brushOptions） */
  onStrokeCommit: (points: { x: number; y: number }[]) => void;
  /** 连续手势（拖动/缩放）起止：父层借机压撤销栈快照 */
  onGestureStart: () => void;
  onGestureEnd: () => void;
  onImageReady: (size: Size) => void;
  onImageError: () => void;
}

export default function EditorCanvas({
  straightening=false,onStraighten,onStraightenCancel,
  onSampleGesture,
  maskControls,
  src,
  showOriginal = false,
  compareSplit=null,onCompareSplit,nativeDisplay=false,
  clippingSrc=null,originalClippingSrc=null,
  sampleMode = false,
  onSample,
  onHoverPixel,
  onSampleError,
  adjustedSrc = null,
  sourceSize = null,
  backendAdjustments = false,
  nativeGeometry = false,
  nativeAnnotations = false,
  zoom = 1,
  onZoom,onFitScale,retouchControls,lensControls,
  fallbackSize,
  recipe,
  tool,
  brushOptions,
  cropRatio,
  cropDraft,
  onCropDraftChange,
  selectedTextId,
  onSelectText,
  onPlaceText,
  onTextChange,
  onStrokeCommit,
  onGestureStart,
  onGestureEnd,
  onImageReady,
  onImageError,
}: EditorCanvasProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const panRef = useRef<{ pointerId: number; x: number; y: number; left: number; top: number } | null>(null);
  const targetRef=useRef<{id:number;lastY:number;height:number;x:number;y:number;delta:number;handler:(event:SampleGesture)=>void}|null>(null);
  const [straightLine,setStraightLine]=useState<[number,number,number,number]|null>(null);
  useEffect(()=>{if(!straightening)setStraightLine(null);},[straightening]);
  function stopTarget(cancelled:boolean){const g=targetRef.current;targetRef.current=null;if(g)g.handler({phase:cancelled?"cancel":"end",x:g.x,y:g.y,delta:g.delta});}
  const [hand,setHand]=useState(false);
  const canPan=hand||!sampleMode && !["crop","brush","text","mask","retouch","lens"].includes(tool);
  const [panning, setPanning] = useState(false);
  function stopPanning(): void {
    const pan = panRef.current;
    panRef.current = null;
    setPanning(false);
    if (pan && containerRef.current?.hasPointerCapture(pan.pointerId)) containerRef.current.releasePointerCapture(pan.pointerId);
  }
  useEffect(() => {
    stopPanning();
    stopTarget(true);
  }, [tool, src, sampleMode,onSampleGesture]);
  useEffect(()=>{
    const down=(event:KeyboardEvent)=>{
      const target=event.target;
      if(target instanceof HTMLElement&&(target.isContentEditable||["INPUT","TEXTAREA","SELECT","BUTTON"].includes(target.tagName)))return;
      if(event.code==="Space"){event.preventDefault();setHand(true);}
    };
    const release=()=>{setHand(false);stopPanning();};
    const up=(event:KeyboardEvent)=>{if(event.code==="Space")release();};
    window.addEventListener("keydown",down);window.addEventListener("keyup",up);window.addEventListener("blur",release);
    return()=>{window.removeEventListener("keydown",down);window.removeEventListener("keyup",up);window.removeEventListener("blur",release);};
  },[]);
  const zoomRef = useRef(onZoom);
  zoomRef.current = onZoom;
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const wheel = (event: WheelEvent) => {
      if (!zoomRef.current || event.deltaY === 0) return;
      event.preventDefault();
      zoomRef.current(event.deltaY < 0 ? 1.1 : 1 / 1.1);
    };
    container.addEventListener("wheel", wheel, { passive: false });
    return () => container.removeEventListener("wheel", wheel);
  }, []);
  const [avail, setAvail] = useState<Size>({ width: 912, height: 600 });
  const image = useHtmlImage(src, onImageError);
  const backendImage = useHtmlImage(adjustedSrc === src ? null : adjustedSrc, onImageError);
  const adjustedImage = useMemo(() => {
    if (showOriginal) return image;
    if (backendAdjustments) return backendImage ?? image;
    if (!image || !recipe.adjustments) return image;
    const a = recipe.adjustments;
    const preview = document.createElement("canvas");
    const ratio = Math.min(1, 2048 / Math.max(image.naturalWidth, image.naturalHeight));
    preview.width = Math.max(1, Math.round(image.naturalWidth * ratio));
    preview.height = Math.max(1, Math.round(image.naturalHeight * ratio));
    const ctx = preview.getContext("2d");
    if (!ctx) return image;
    ctx.filter = `brightness(${1 + a.brightness / 100}) contrast(${1 + a.contrast / 100}) saturate(${1 + a.saturation / 100})`;
    ctx.drawImage(image, 0, 0, preview.width, preview.height);
    return preview;
  }, [image, backendImage, backendAdjustments, recipe.adjustments, showOriginal]);
  const readyRef = useRef(onImageReady);
  readyRef.current = onImageReady;

  // --- 全部钩子必须在 image===null 早退之前：图未加载与加载完成两个渲染分支
  //     的钩子数不一致会触发 React「Rendered more hooks」崩溃 ----------------------
  // 画笔在途一笔（Free Drawing）
  const [drawingPoints, setDrawingPoints] = useState<number[] | null>(null);
  const drawingRef = useRef<number[] | null>(null);
  // 文字双击编辑状态（textarea 覆盖）
  const [editing, setEditing] = useState<{ id: string; x: number; y: number; fontSize: number; color: string; text: string } | null>(null);
  // Transformer/节点引用与选中文字节点
  const textTrRef = useRef<Konva.Transformer | null>(null);
  const cropTrRef = useRef<Konva.Transformer | null>(null);
  const cropRectRef = useRef<Konva.Rect | null>(null);
  const textNodesRef = useRef(new Map<string, Konva.Text>());
  const [selectedTextNode, setSelectedTextNode] = useState<Konva.Text | null>(null);

  // 容器尺寸（ResizeObserver；jsdom stub 下保持缺省值）
  useEffect(() => {
    const el = containerRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => {
      const rect = el.getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) {
        setAvail({ width: rect.width, height: rect.height });
      }
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // 自然尺寸上报（EXIF 兜底）——只报一次每图
  useEffect(() => {
    if (image === null) return;
    const w = sourceSize?.width || image.naturalWidth || fallbackSize?.width || 0;
    const h = sourceSize?.height || image.naturalHeight || fallbackSize?.height || 0;
    if (w > 0 && h > 0) readyRef.current({ width: w, height: h });
  }, [image, fallbackSize, sourceSize]);

  // Transformer 绑定（同样必须早退之前）
  useEffect(() => {
    setSelectedTextNode(
      selectedTextId !== null ? textNodesRef.current.get(selectedTextId) ?? null : null,
    );
  }, [selectedTextId, recipe.textLayers]);
  useEffect(() => {
    if (textTrRef.current === null) return;
    textTrRef.current.nodes(selectedTextNode !== null ? [selectedTextNode] : []);
    textTrRef.current.getLayer()?.batchDraw();
  }, [selectedTextNode, tool, recipe]);
  useEffect(() => {
    if (cropTrRef.current === null || cropRectRef.current === null) return;
    cropTrRef.current.nodes([cropRectRef.current]);
    cropTrRef.current.getLayer()?.batchDraw();
  }, [tool, cropDraft !== null, image]);

  const sourceW=sourceSize?.width||image?.naturalWidth||fallbackSize?.width||1;
  const sourceH=sourceSize?.height||image?.naturalHeight||fallbackSize?.height||1;
  const fitGeometry=geometrySize({width:sourceW,height:sourceH},recipe);
  const fitCrop=tool==="crop"?{w:1,h:1}:recipe.crop??{w:1,h:1};
  const fitScale=Math.min(Math.max(60,avail.width-STAGE_PADDING*2)/(fitGeometry.w*fitCrop.w),Math.max(60,avail.height-STAGE_PADDING*2)/(fitGeometry.h*fitCrop.h));
  useEffect(()=>{onFitScale?.(fitScale);},[fitScale,onFitScale]);
  const previousZoom=useRef(zoom);
  useEffect(()=>{
    const view=containerRef.current;
    if(view&&previousZoom.current!==zoom){
      const ratio=zoom/previousZoom.current;
      view.scrollLeft=(view.scrollLeft+view.clientWidth/2)*ratio-view.clientWidth/2;
      view.scrollTop=(view.scrollTop+view.clientHeight/2)*ratio-view.clientHeight/2;
    }
    previousZoom.current=zoom;
  },[zoom]);
  if (image === null) return <div ref={containerRef} className="flex min-h-0 flex-1" />;

  const baseW = sourceSize?.width || image.naturalWidth || fallbackSize?.width || 1;
  const baseH = sourceSize?.height || image.naturalHeight || fallbackSize?.height || 1;
  const quarter = recipe.rotateQuarter;
  const rot = geometrySize({width:baseW,height:baseH},recipe);
  const pad = STAGE_PADDING;
  const inner: Size = {
    width: Math.max(60, avail.width - pad * 2),
    height: Math.max(60, avail.height - pad * 2),
  };

  // 裁剪工具：整图布局；其余工具：裁剪后画布布局（图层坐标系）
  const crop = recipe.crop ?? { x: 0, y: 0, w: 1, h: 1 };
  let dispW: number;
  let dispH: number;
  let scale: number;
  if (tool === "crop") {
    scale = Math.min(inner.width / rot.w, inner.height / rot.h) * zoom;
    dispW = rot.w * scale;
    dispH = rot.h * scale;
  } else {
    const canvasPxW = crop.w * rot.w;
    const canvasPxH = crop.h * rot.h;
    scale = Math.min(inner.width / canvasPxW, inner.height / canvasPxH) * zoom;
    dispW = canvasPxW * scale;
    dispH = canvasPxH * scale;
  }
  const canvas: Size = { width: dispW, height: dispH };
  const frameW = rot.w * scale;
  const frameH = rot.h * scale;
  const sourcePoint=(x:number,y:number)=>{const [u,v]=sourceToPlane(x,y,recipe,{width:baseW,height:baseH});return [(u-(tool==="crop"?0:crop.x))*frameW,(v-(tool==="crop"?0:crop.y))*frameH];};
  const p0=sourcePoint(0,0),px=sourcePoint(1,0),py=sourcePoint(0,1);
  const sourceTransform=`matrix(${(px[0]-p0[0])/baseW},${(px[1]-p0[1])/baseW},${(py[0]-p0[0])/baseH},${(py[1]-p0[1])/baseH},${p0[0]},${p0[1]})`;

  // --- 画笔（Free Drawing）---------------------------------------------------------
  const brushWidthPx = strokeWidthPx(brushOptions.widthRel, canvas);

  function pointerPos(stage: Konva.Stage): { x: number; y: number } | null {
    return stage.getPointerPosition();
  }

  function handleStageMouseDown(e: Konva.KonvaEventObject<MouseEvent>): void {
    const stage = e.target.getStage();
    if (stage === null || stage === undefined) return;
    if(hand||panning)return;
    if (sampleMode) {
      if(onSampleGesture)return;
      const pos = pointerPos(stage);
      const sampled=pos ? planeToSource(crop.x+pos.x/canvas.width*crop.w,crop.y+pos.y/canvas.height*crop.h,recipe,{width:baseW,height:baseH}) : null;
      const source=sampled && sampled.every(v=>v>=0&&v<=1) ? sampled : null;
      if (source) onSample?.(source[0], source[1]);else onSampleError?.();
      return;
    }
    if (tool === "brush") {
      const pos = pointerPos(stage);
      if (pos === null) return;
      const points = [pos.x, pos.y];
      drawingRef.current = points;
      setDrawingPoints(points);
    } else if (tool === "text" && e.target === stage) {
      // 空白处点按 → 放置文字（点在文字节点上由节点自身处理）
      const pos = pointerPos(stage);
      if (pos === null) return;
      onPlaceText({ x: clamp01(pos.x / canvas.width), y: clamp01(pos.y / canvas.height) });
    }
  }

  function handleStageMouseMove(e: Konva.KonvaEventObject<MouseEvent>): void {
    if (drawingRef.current === null) return;
    const stage = e.target.getStage();
    if (stage === null || stage === undefined) return;
    const pos = pointerPos(stage);
    if (pos === null) return;
    const points = drawingRef.current;
    const lastX = points[points.length - 2];
    const lastY = points[points.length - 1];
    // 距上一采点 >1px 才记录，避免折线堆点
    if (Math.hypot(pos.x - lastX, pos.y - lastY) < 1) return;
    const next = [...points, pos.x, pos.y];
    drawingRef.current = next;
    setDrawingPoints(next);
  }

  function commitStroke(): void {
    const points = drawingRef.current;
    drawingRef.current = null;
    setDrawingPoints(null);
    if (points === null || points.length < 2) return;
    onStrokeCommit(linePointsToNorm(points, canvas));
  }

  // --- 文字编辑（双击 textarea 覆盖，官方 demo 模式） --------------------------------

  function finishTextEditing(commit: boolean): void {
    if (editing === null) return;
    const current = editing;
    setEditing(null);
    if (commit) onTextChange(current.id, { text: current.text });
  }

  // --- Transformer 绑定 --------------------------------------------------------------

  const cropMode = tool === "crop" && cropDraft !== null;
  const cropRect = cropMode ? cropToRectAttrs(cropDraft, canvas) : null;

  function updateCropTransform(e: Konva.KonvaEventObject<Event>) {
    const node=e.target as Konva.Rect;
    const width=Math.min(Math.max(node.width()*node.scaleX(),8),dispW);
    const height=Math.min(Math.max(node.height()*node.scaleY(),8),dispH);
    node.scaleX(1);node.scaleY(1);node.width(width);node.height(height);
    node.x(Math.min(Math.max(node.x(),0),dispW-width));node.y(Math.min(Math.max(node.y(),0),dispH-height));
    onCropDraftChange(rectAttrsToCrop({x:node.x(),y:node.y(),width,height},canvas));
  }

  const cursor =
    hand ? panning?"grabbing":"grab" : sampleMode || tool === "brush" || tool === "text" ? "crosshair" : canPan && zoom > 1 ? panning ? "grabbing" : "grab" : "default";

  return (
    <div ref={containerRef} className="sp-scroll relative flex min-h-0 flex-1 overflow-auto" data-testid="editor-canvas-viewport"
      style={{ cursor, touchAction: canPan ? "none" : undefined }}
      onPointerDownCapture={event=>{
        if(!hand&&event.button!==1)return;
        event.preventDefault();event.stopPropagation();
        const viewport=event.currentTarget;
        panRef.current={pointerId:event.pointerId,x:event.clientX,y:event.clientY,left:viewport.scrollLeft,top:viewport.scrollTop};
        viewport.setPointerCapture(event.pointerId);setPanning(true);
      }}
      onPointerDown={(event) => {
        const viewport = event.currentTarget;
        if(sampleMode&&onSampleGesture&&event.button===0){
          const rect=viewport.querySelector<HTMLElement>('[data-testid="editor-canvas-frame"]')?.getBoundingClientRect();
          if(!rect?.width||!rect.height)return;
          const [x,y]=planeToSource(crop.x+(event.clientX-rect.left)/rect.width*crop.w,crop.y+(event.clientY-rect.top)/rect.height*crop.h,recipe,{width:baseW,height:baseH});
          if(x<0||y<0||x>1||y>1){onSampleError?.();return;}
          event.preventDefault();viewport.setPointerCapture(event.pointerId);
          targetRef.current={id:event.pointerId,lastY:event.clientY,height:rect.height,x,y,delta:0,handler:onSampleGesture};
          onSampleGesture({phase:"start",x,y,delta:0});return;
        }
        if (!canPan || event.button !== 0 || viewport.scrollWidth <= viewport.clientWidth && viewport.scrollHeight <= viewport.clientHeight) return;
        event.preventDefault();
        panRef.current = { pointerId: event.pointerId, x: event.clientX, y: event.clientY, left: viewport.scrollLeft, top: viewport.scrollTop };
        viewport.setPointerCapture(event.pointerId);
        setPanning(true);
      }}
      onPointerMoveCapture={event=>{const pan=panRef.current;if(!pan)return;event.stopPropagation();event.currentTarget.scrollLeft=pan.left+pan.x-event.clientX;event.currentTarget.scrollTop=pan.top+pan.y-event.clientY;}}
      onPointerUpCapture={event=>{if(panRef.current){event.stopPropagation();stopPanning();}}}
      onPointerCancelCapture={()=>{if(panRef.current)stopPanning();}}
      onPointerLeave={()=>onHoverPixel?.(null)}
      onPointerMove={(event) => {
        const rect=event.currentTarget.querySelector<HTMLElement>('[data-testid="editor-canvas-frame"]')?.getBoundingClientRect();
        const x=rect?.width?(event.clientX-rect.left)/rect.width:-1;
        const y=rect?.height?(event.clientY-rect.top)/rect.height:-1;
        onHoverPixel?.(x>=0&&x<=1&&y>=0&&y<=1?[x,y]:null);
        const target=targetRef.current;
        if(target&&target.id===event.pointerId){target.delta+=(target.lastY-event.clientY)/target.height*(event.altKey?0.1:1);target.lastY=event.clientY;target.handler({phase:"move",x:target.x,y:target.y,delta:target.delta});return;}
        const pan = panRef.current;
        if (!pan || event.pointerId !== pan.pointerId) return;
        event.currentTarget.scrollLeft = pan.left + pan.x - event.clientX;
        event.currentTarget.scrollTop = pan.top + pan.y - event.clientY;
      }}
      onPointerUp={event=>{if(targetRef.current?.id===event.pointerId)stopTarget(false);stopPanning();}} onPointerCancel={event=>{if(targetRef.current?.id===event.pointerId)stopTarget(true);stopPanning();}} onLostPointerCapture={event=>{if(targetRef.current?.id===event.pointerId)stopTarget(false);stopPanning();}}
    >
      <div className="relative m-auto shrink-0" style={{ width: dispW, height: dispH }} data-testid="editor-canvas-frame" data-rotate={quarter} data-straighten-guide={straightLine?JSON.stringify(straightLine):undefined} data-crop-guide={cropMode&&cropDraft?JSON.stringify([cropDraft.x,cropDraft.y,cropDraft.w,cropDraft.h]):undefined}>
        {tool==="crop"&&!straightening&&cropDraft&&<svg aria-hidden="true" className="pointer-events-none absolute inset-0 z-10" width={dispW} height={dispH}>
          {[1/3,2/3].map(position=><g key={position} stroke="white" strokeOpacity={0.5} strokeWidth={1}>
            <line x1={(cropDraft.x+cropDraft.w*position)*dispW} x2={(cropDraft.x+cropDraft.w*position)*dispW} y1={cropDraft.y*dispH} y2={(cropDraft.y+cropDraft.h)*dispH}/>
            <line x1={cropDraft.x*dispW} x2={(cropDraft.x+cropDraft.w)*dispW} y1={(cropDraft.y+cropDraft.h*position)*dispH} y2={(cropDraft.y+cropDraft.h*position)*dispH}/>
          </g>)}
        </svg>}
        {tool==="crop"&&straightening&&<><StraightenOverlay width={dispW} height={dispH} onPreview={setStraightLine} onCancel={onStraightenCancel} onCommit={angle=>onStraighten?.(angle)}/>{straightLine&&<svg className="pointer-events-none absolute inset-0 z-10" width={dispW} height={dispH}><line x1={straightLine[0]*dispW} y1={straightLine[1]*dispH} x2={straightLine[2]*dispW} y2={straightLine[3]*dispH} stroke="white" strokeWidth={2}/></svg>}</>}
        {lensControls&&tool==="lens"&&<LensCanvasControls recipe={recipe} size={{width:baseW,height:baseH}} width={dispW} height={dispH} {...lensControls} axis={hand?null:lensControls.axis}/>}
        {retouchControls&&tool==="retouch"&&!hand&&<RetouchCanvasControls recipe={recipe} size={{width:baseW,height:baseH}} width={dispW} height={dispH} src={adjustedSrc??src} {...retouchControls}/>}
        {maskControls&&<MaskCanvasControls {...maskControls} editing={maskControls.editing!==false&&!sampleMode&&!hand} recipe={recipe} size={{width:baseW,height:baseH}} width={dispW} height={dispH}/>}
        <Stage
          width={dispW}
          height={dispH}
          style={{ cursor }}
          onMouseDown={handleStageMouseDown}
          onMouseMove={handleStageMouseMove}
          onMouseUp={commitStroke}
          onMouseLeave={commitStroke}
          data-testid="editor-stage"
        >
          {/* 底图（裁剪模式下显示整图，其余模式平移到裁剪窗口并裁剪） */}
          <Layer clip={cropMode ? undefined : { x: 0, y: 0, width: dispW, height: dispH }}>
            {nativeGeometry && backendImage && !showOriginal ? <KonvaImage opacity={1} image={backendImage} width={dispW} height={dispH} listening={false} /> : <Group x={cropMode ? 0 : -crop.x * frameW} y={cropMode ? 0 : -crop.y * frameH}>
              <KonvaImage
                opacity={1}
                image={adjustedImage ?? image}
                {...rotatedImageAttrs(baseW, baseH, quarter, scale)}
                listening={false}
              />
            </Group>}
          </Layer>

          {cropMode && cropRect !== null ? (
            <>
              {/* 暗幕（选区外四矩形） */}
              <Layer listening={false}>
                <Rect x={0} y={0} width={dispW} height={cropRect.y} fill="rgba(0,0,0,0.55)" />
                <Rect x={0} y={cropRect.y + cropRect.height} width={dispW} height={dispH - cropRect.y - cropRect.height} fill="rgba(0,0,0,0.55)" />
                <Rect x={0} y={cropRect.y} width={cropRect.x} height={cropRect.height} fill="rgba(0,0,0,0.55)" />
                <Rect x={cropRect.x + cropRect.width} y={cropRect.y} width={dispW - cropRect.x - cropRect.width} height={cropRect.height} fill="rgba(0,0,0,0.55)" />
              </Layer>
              <Layer>
                <Rect
                  ref={cropRectRef}
                  x={cropRect.x}
                  y={cropRect.y}
                  width={cropRect.width}
                  height={cropRect.height}
                  stroke="#FFFFFF"
                  strokeWidth={2}
                  draggable
                  onDragMove={(e) => {
                    const node = e.target;
                    node.x(Math.min(Math.max(node.x(), 0), dispW - node.width()));
                    node.y(Math.min(Math.max(node.y(), 0), dispH - node.height()));
                    onCropDraftChange(rectAttrsToCrop({x:node.x(),y:node.y(),width:node.width(),height:node.height()},canvas));
                  }}
                  onDragEnd={(e) => {
                    const node = e.target;
                    onCropDraftChange(rectAttrsToCrop({ x: node.x(), y: node.y(), width: node.width(), height: node.height() }, canvas));
                  }}
                  onTransform={updateCropTransform}
                  onTransformEnd={updateCropTransform}
                  /* live transforms keep the DOM crop guide in sync with Konva */
                  data-testid="editor-crop-rect"
                />
                <Transformer
                  ref={cropTrRef}
                  rotateEnabled={false}
                  keepRatio={cropRatio !== null}
                  enabledAnchors={cropRatio === null ? undefined : ["top-left", "top-right", "bottom-left", "bottom-right"]}
                  anchorFill="#FFFFFF"
                  anchorStroke="#000000"
                  anchorSize={10}
                  borderStroke="#FFFFFF"
                  boundBoxFunc={(_old, box) => {
                    const width = Math.min(Math.max(box.width, 8), dispW);
                    const height = Math.min(Math.max(box.height, 8), dispH);
                    return {
                      ...box,
                      x: Math.min(Math.max(box.x, 0), dispW - width),
                      y: Math.min(Math.max(box.y, 0), dispH - height),
                      width,
                      height,
                      rotation: 0,
                    };
                  }}
                />
              </Layer>
            </>
          ) : (
            <>
              {/* 笔迹（recipe 反序列化 + 在途一笔） */}
              <Layer listening={false}>
                {!nativeAnnotations && recipe.brushStrokes.map((stroke) =>
                  stroke.points.length > 1 ? (
                    <Line
                      key={stroke.id}
                      points={strokeToLinePoints(stroke, canvas)}
                      stroke={stroke.color}
                      strokeWidth={strokeWidthPx(stroke.widthRel, canvas)}
                      lineCap="round"
                      lineJoin="round"
                    />
                  ) : stroke.points.length === 1 ? (
                    <Circle
                      key={stroke.id}
                      x={stroke.points[0].x * canvas.width}
                      y={stroke.points[0].y * canvas.height}
                      radius={strokeWidthPx(stroke.widthRel, canvas) / 2}
                      fill={stroke.color}
                    />
                  ) : null,
                )}
                {drawingPoints !== null && drawingPoints.length >= 2 && (
                  <Line
                    points={drawingPoints}
                    stroke={brushOptions.color}
                    strokeWidth={brushWidthPx}
                    lineCap="round"
                    lineJoin="round"
                  />
                )}
              </Layer>
              {/* 文字层 + Transformer（仅文字工具可交互） */}
              <Layer>
                {recipe.textLayers.map((layer) => {
                  const attrs = textLayerToNodeAttrs(layer, canvas);
                  const isEditing = editing?.id === layer.id;
                  return (
                    <KonvaText
                      key={layer.id}
                      opacity={nativeAnnotations ? 0 : 1}
                      name={TEXT_NODE_NAME}
                      x={attrs.x}
                      y={attrs.y}
                      text={isEditing ? "" : layer.text === "" ? " " : layer.text}
                      fontSize={attrs.fontSize}
                      fill={layer.color}
                      lineHeight={1.25}
                      visible={!isEditing}
                      draggable={tool === "text"}
                      listening={tool === "text"}
                      onClick={() => {
                        if (tool !== "text") return;
                        onSelectText(layer.id);
                      }}
                      onDragStart={() => onGestureStart()}
                      onDragEnd={(e) => {
                        const node = e.target as Konva.Text;
                        onTextChange(layer.id, { x: node.x() / canvas.width, y: node.y() / canvas.height });
                        onGestureEnd();
                      }}
                      onTransformStart={() => onGestureStart()}
                      onTransformEnd={(e) => {
                        const node = e.target as Konva.Text;
                        const baked = bakeTextScale({
                          x: node.x(),
                          y: node.y(),
                          fontSize: node.fontSize(),
                          scaleX: node.scaleX(),
                        });
                        node.scaleX(1);
                        node.scaleY(1);
                        node.fontSize(baked.fontSize);
                        onTextChange(layer.id, {
                          x: baked.x / canvas.width,
                          y: baked.y / canvas.height,
                          sizeRel: baked.fontSize / canvas.width,
                        });
                        onGestureEnd();
                      }}
                      onDblClick={(e) => {
                        const node = e.target as Konva.Text;
                        setEditing({
                          id: layer.id,
                          x: node.x(),
                          y: node.y(),
                          fontSize: node.fontSize(),
                          color: layer.color,
                          text: layer.text,
                        });
                      }}
                      ref={(node) => {
                        if (node === null) textNodesRef.current.delete(layer.id);
                        else textNodesRef.current.set(layer.id, node);
                      }}
                    />
                  );
                })}
                {tool === "text" && selectedTextNode !== null && (
                  <Transformer
                    ref={textTrRef}
                    rotateEnabled={false}
                    enabledAnchors={["top-left", "top-right", "bottom-left", "bottom-right"]}
                    anchorFill="#FFFFFF"
                    anchorStroke="#000000"
                    anchorSize={10}
                    borderStroke="#FFFFFF"
                    boundBoxFunc={(_old, box) => ({ ...box, rotation: 0 })}
                  />
                )}
              </Layer>
            </>
          )}
        </Stage>
        {!nativeDisplay&&clippingSrc&&!showOriginal&&<img src={clippingSrc} alt="" className="pointer-events-none absolute inset-0 h-full w-full"/>}
        {showOriginal&&nativeGeometry&&!nativeDisplay&&<div className="pointer-events-none absolute inset-0 overflow-hidden bg-[#202020]" data-testid="editor-comparison-original"><img src={src??undefined} alt="" draggable={false} style={{position:"absolute",width:baseW,height:baseH,maxWidth:"none",transformOrigin:"0 0",transform:sourceTransform}}/></div>}
        {compareSplit!==null&&!showOriginal&&onCompareSplit&&<>
          {!nativeDisplay&&<div className="pointer-events-none absolute inset-y-0 left-0 overflow-hidden bg-[#202020]" style={{width:`${compareSplit*100}%`}} data-testid="editor-comparison-original"><img src={src??undefined} alt="" draggable={false} style={{position:"absolute",width:baseW,height:baseH,maxWidth:"none",transformOrigin:"0 0",transform:sourceTransform}}/>{originalClippingSrc&&<img src={originalClippingSrc} alt="" className="pointer-events-none absolute left-0 top-0 max-w-none" style={{width:dispW,height:dispH}}/>}</div>}
          <ComparisonOverlay position={compareSplit} onChange={onCompareSplit}/>
        </>}
        {!nativeDisplay&&originalClippingSrc&&showOriginal&&<img src={originalClippingSrc} alt="" className="pointer-events-none absolute inset-0 h-full w-full"/>}

        {/* 双击编辑文字：textarea 覆盖在画布上（官方 demo 模式；Enter 提交 / Esc 取消） */}
        {editing !== null && (
          <textarea
            autoFocus
            value={editing.text}
            onChange={(e) => setEditing({ ...editing, text: e.target.value })}
            onBlur={() => finishTextEditing(true)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                finishTextEditing(true);
              } else if (e.key === "Escape") {
                e.preventDefault();
                finishTextEditing(false);
              }
            }}
            className="absolute z-10 resize-none overflow-hidden border-none bg-black/30 p-0 outline-none"
            style={{
              left: editing.x,
              top: editing.y,
              width: Math.max(120, dispW - editing.x),
              fontSize: editing.fontSize,
              lineHeight: 1.25,
              color: editing.color,
            }}
            data-testid="editor-text-inline-input"
          />
        )}
      </div>
    </div>
  );
}
