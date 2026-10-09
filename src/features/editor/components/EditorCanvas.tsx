import { sourceSamplePosition } from "../lib/coords";
import { useEffect, useMemo, useRef, useState } from "react";

import { Circle, Group, Image as KonvaImage, Layer, Line, Rect, Stage, Text as KonvaText, Transformer } from "react-konva";
import type Konva from "konva";

import type { EditRecipe, EditRecipeCrop } from "@/ipc/api";
import { rotatedSize, type RecipeContext, type TextLayerPatch } from "../lib/recipe";
import { clamp01, type Size } from "../lib/coords";
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

export type EditorTool = "view" | "crop" | "text" | "brush" | "adjust" | "filters" | "metadata" | "output";

const TEXT_NODE_NAME = "editor-text-layer";
const STAGE_PADDING = 24;

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
  showOriginal?: boolean;
  sampleMode?: boolean;
  onSample?: (x: number, y: number) => void;
  src: string | null;
  /** 后端提供未旋转底图的调整结果；禁止再次叠加浏览器滤镜。 */
  adjustedSrc?: string | null;
  sourceSize?: Size | null;
  backendAdjustments?: boolean;
  nativeGeometry?: boolean;
  nativeAnnotations?: boolean;
  zoom?: number;
  onZoom?: (factor: number) => void;
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
  src,
  showOriginal = false,
  sampleMode = false,
  onSample,
  adjustedSrc = null,
  sourceSize = null,
  backendAdjustments = false,
  nativeGeometry = false,
  nativeAnnotations = false,
  zoom = 1,
  onZoom,
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
  const [panning, setPanning] = useState(false);
  function stopPanning(): void {
    const pan = panRef.current;
    panRef.current = null;
    setPanning(false);
    if (pan && containerRef.current?.hasPointerCapture(pan.pointerId)) containerRef.current.releasePointerCapture(pan.pointerId);
  }
  useEffect(() => {
    stopPanning();
  }, [tool, src]);
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
  const backendImage = useHtmlImage(adjustedSrc, onImageError);
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
  }, [tool, cropDraft]);

  if (image === null) return <div ref={containerRef} className="flex min-h-0 flex-1" />;

  const baseW = sourceSize?.width || image.naturalWidth || fallbackSize?.width || 1;
  const baseH = sourceSize?.height || image.naturalHeight || fallbackSize?.height || 1;
  const quarter = recipe.rotateQuarter;
  const rot = rotatedSize({ width: baseW, height: baseH } satisfies RecipeContext, quarter);
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

  // --- 画笔（Free Drawing）---------------------------------------------------------
  const brushWidthPx = strokeWidthPx(brushOptions.widthRel, canvas);

  function pointerPos(stage: Konva.Stage): { x: number; y: number } | null {
    return stage.getPointerPosition();
  }

  function handleStageMouseDown(e: Konva.KonvaEventObject<MouseEvent>): void {
    const stage = e.target.getStage();
    if (stage === null || stage === undefined) return;
    if (sampleMode) {
      const pos = pointerPos(stage);
      const source = pos && sourceSamplePosition(pos, { width: frameW, height: frameH }, cropMode ? null : crop, quarter);
      if (source) onSample?.(source[0], source[1]);
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

  const cursor =
    sampleMode || tool === "brush" || tool === "text" ? "crosshair" : tool === "view" && zoom > 1 ? panning ? "grabbing" : "grab" : "default";

  return (
    <div ref={containerRef} className="sp-scroll relative flex min-h-0 flex-1 overflow-auto" data-testid="editor-canvas-viewport"
      style={{ cursor, touchAction: tool === "view" ? "none" : undefined }}
      onPointerDown={(event) => {
        const viewport = event.currentTarget;
        if (sampleMode || tool !== "view" || event.button !== 0 || viewport.scrollWidth <= viewport.clientWidth && viewport.scrollHeight <= viewport.clientHeight) return;
        event.preventDefault();
        panRef.current = { pointerId: event.pointerId, x: event.clientX, y: event.clientY, left: viewport.scrollLeft, top: viewport.scrollTop };
        viewport.setPointerCapture(event.pointerId);
        setPanning(true);
      }}
      onPointerMove={(event) => {
        const pan = panRef.current;
        if (!pan || event.pointerId !== pan.pointerId) return;
        event.currentTarget.scrollLeft = pan.left + pan.x - event.clientX;
        event.currentTarget.scrollTop = pan.top + pan.y - event.clientY;
      }}
      onPointerUp={stopPanning} onPointerCancel={stopPanning} onLostPointerCapture={stopPanning}
    >
      <div className="relative m-auto shrink-0" style={{ width: dispW, height: dispH }} data-testid="editor-canvas-frame" data-rotate={quarter}>
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
            {nativeGeometry && backendImage && !showOriginal ? <KonvaImage image={backendImage} width={dispW} height={dispH} listening={false} /> : <Group x={cropMode ? 0 : -crop.x * frameW} y={cropMode ? 0 : -crop.y * frameH}>
              <KonvaImage
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
                  }}
                  onDragEnd={(e) => {
                    const node = e.target;
                    onCropDraftChange(rectAttrsToCrop({ x: node.x(), y: node.y(), width: node.width(), height: node.height() }, canvas));
                  }}
                  onTransformEnd={(e) => {
                    const node = e.target;
                    // Transformer 改写 scale：烘焙回 width/height 并复位
                    const width = Math.min(Math.max(node.width() * node.scaleX(), 8), dispW);
                    const height = Math.min(Math.max(node.height() * node.scaleY(), 8), dispH);
                    node.scaleX(1);
                    node.scaleY(1);
                    node.width(width);
                    node.height(height);
                    node.x(Math.min(Math.max(node.x(), 0), dispW - width));
                    node.y(Math.min(Math.max(node.y(), 0), dispH - height));
                    onCropDraftChange(rectAttrsToCrop({ x: node.x(), y: node.y(), width, height }, canvas));
                  }}
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
