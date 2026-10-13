import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import EditorCanvas, { type EditorTool } from "./EditorCanvas";
import { defaultRecipe } from "../lib/recipe";
const cropEvents=vi.hoisted(()=>({props:{} as Record<string,any>}));

vi.mock("react-konva", () => {
  const Container = ({ children }: { children?: ReactNode }) => <div>{children}</div>;
  return {...Object.fromEntries(["Circle", "Group", "Image", "Layer", "Line", "Rect", "Stage", "Text", "Transformer"].map((name) => [name, Container])),Rect:(props:Record<string,any>)=>{if(props["data-testid"]==="editor-crop-rect")cropEvents.props=props;return null;}};
});

beforeEach(() => {
  vi.stubGlobal("Image", class {
    naturalWidth = 1200;
    naturalHeight = 800;
    onload: (() => void) | null = null;
    set src(_value: string) { queueMicrotask(() => this.onload?.()); }
  });
  vi.stubGlobal("PointerEvent", class extends MouseEvent {
    pointerId: number;
    constructor(type: string, options: PointerEventInit = {}) {
      super(type, options);
      this.pointerId = options.pointerId ?? 1;
    }
  });
});
afterEach(() => vi.unstubAllGlobals());

async function canvas(tool: EditorTool = "view",sampleMode=false,onSampleGesture?:Parameters<typeof EditorCanvas>[0]["onSampleGesture"],extra:Partial<Parameters<typeof EditorCanvas>[0]>={}) {
  const onZoom = vi.fn();
  const noop = vi.fn();
  render(<EditorCanvas onSampleGesture={onSampleGesture} sampleMode={sampleMode} src="test.jpg" zoom={2} onZoom={onZoom} fallbackSize={null} recipe={defaultRecipe()} tool={tool}
    brushOptions={{ color: "#FF0000", widthRel: 0.01 }} cropRatio={null} cropDraft={null}
    onCropDraftChange={noop} selectedTextId={null} onSelectText={noop} onPlaceText={noop} onTextChange={noop}
    onStrokeCommit={noop} onGestureStart={noop} onGestureEnd={noop} onImageReady={noop} onImageError={noop} {...extra}/>);
  const viewport = await screen.findByTestId("editor-canvas-viewport");
  Object.defineProperties(viewport, {
    scrollWidth: { value: 1800, configurable: true }, clientWidth: { value: 600, configurable: true },
    scrollHeight: { value: 1200, configurable: true }, clientHeight: { value: 400, configurable: true },
    setPointerCapture: { value: vi.fn() }, hasPointerCapture: { value: vi.fn(() => true) }, releasePointerCapture: { value: vi.fn() },
  });
  viewport.scrollLeft = 200;
  viewport.scrollTop = 150;
  return { viewport, onZoom };
}

describe("编辑器画布缩放与拖动", () => {
  it("updates the crop guide during dragging and resizing, before release",async()=>{
    const change=vi.fn();await canvas("crop",false,undefined,{cropDraft:{x:0,y:0,w:1,h:1},onCropDraftChange:change});
    const values:Record<string,number>={x:20,y:10,width:60,height:40,scaleX:2,scaleY:1.5};
    const node=Object.fromEntries(Object.keys(values).map(key=>[key,(v?:number)=>{if(v!==undefined)values[key]=v;return values[key];}]));
    cropEvents.props.onDragMove({target:node});expect(change).toHaveBeenCalledTimes(1);
    const dragged=change.mock.calls[0][0];
    cropEvents.props.onTransform({target:node});expect(change).toHaveBeenCalledTimes(2);
    const resized=change.mock.calls[1][0];expect(resized.w).toBeCloseTo(dragged.w*2);expect(resized.h).toBeCloseTo(dragged.h*1.5);
    expect(values.scaleX).toBe(1);expect(values.scaleY).toBe(1);
  });
  it("reports pixels relative to the displayed frame and clears outside it",async()=>{
    const hover=vi.fn();const {viewport}=await canvas("view",false,undefined,{onHoverPixel:hover});
    const frame=screen.getByTestId("editor-canvas-frame");
    vi.spyOn(frame,"getBoundingClientRect").mockReturnValue({left:100,top:200,width:400,height:200} as DOMRect);
    fireEvent.pointerMove(viewport,{clientX:300,clientY:250});expect(hover).toHaveBeenLastCalledWith([.5,.25]);
    fireEvent.pointerMove(viewport,{clientX:50,clientY:250});expect(hover).toHaveBeenLastCalledWith(null);
    fireEvent.pointerLeave(viewport);expect(hover).toHaveBeenLastCalledWith(null);
  });
  it("滚轮放大/缩小并阻止滚动页面", async () => {
    const { viewport, onZoom } = await canvas();
    expect(fireEvent.wheel(viewport, { deltaY: -100, cancelable: true })).toBe(false);
    expect(onZoom).toHaveBeenLastCalledWith(1.1);
    fireEvent.wheel(viewport, { deltaY: 100 });
    expect(onZoom).toHaveBeenLastCalledWith(1 / 1.1);
  });

  it("查看模式拖动平移，松开后停止", async () => {
    const { viewport } = await canvas();
    fireEvent.pointerDown(viewport, { pointerId: 1, button: 0, clientX: 400, clientY: 300 });
    fireEvent.pointerMove(viewport, { pointerId: 1, clientX: 250, clientY: 180 });
    expect(viewport.scrollLeft).toBe(350);
    expect(viewport.scrollTop).toBe(270);
    fireEvent.pointerUp(viewport, { pointerId: 1 });
    fireEvent.pointerMove(viewport, { pointerId: 1, clientX: 100, clientY: 100 });
    expect(viewport.scrollLeft).toBe(350);
    expect(viewport.releasePointerCapture).toHaveBeenCalledWith(1);
  });

  it("画笔工具的拖动不平移照片", async () => {
    const { viewport } = await canvas("brush");
    fireEvent.pointerDown(viewport, { pointerId: 1, button: 0, clientX: 400, clientY: 300 });
    fireEvent.pointerMove(viewport, { pointerId: 1, clientX: 250, clientY: 180 });
    expect(viewport.scrollLeft).toBe(200);
    expect(viewport.scrollTop).toBe(150);
  });
});
it.each([false,true])("split comparison keeps its handle and uses a source overlay only outside native display (%s)",async(nativeDisplay)=>{
 const recipe={...defaultRecipe(),geometry:{angle:0,flipHorizontal:true,flipVertical:false},crop:{x:0.1,y:0.2,w:0.8,h:0.7}};
 await canvas("adjust",false,undefined,{recipe,compareSplit:0.6,onCompareSplit:vi.fn(),nativeDisplay,nativeGeometry:true,clippingSrc:"edited-warning.png",originalClippingSrc:"original-warning.png"});
 expect(screen.getByTestId("editor-comparison")).toBeInTheDocument();
 const original=screen.queryByTestId("editor-comparison-original");
 if(nativeDisplay){expect(original).not.toBeInTheDocument();expect(document.querySelector('img[src="edited-warning.png"]')).not.toBeInTheDocument();}else{expect(original).toHaveStyle({width:"60%"});expect(original!.querySelector("img")!.style.transform).toMatch(/^matrix\(-/);expect(original!.querySelector('img[src="original-warning.png"]')).toBeInTheDocument();expect(document.querySelector('img[src="edited-warning.png"]')).toBeInTheDocument();}
});

it.each(["adjust","filters","output","metadata"] as EditorTool[])("%s allows panning without a separate view tool",async(tool)=>{
 const {viewport}=await canvas(tool);
 fireEvent.pointerDown(viewport,{pointerId:1,button:0,clientX:400,clientY:300});
 fireEvent.pointerMove(viewport,{pointerId:1,clientX:250,clientY:180});
 expect(viewport.scrollLeft).toBe(350);expect(viewport.scrollTop).toBe(270);
});
it("sampling never starts pan gestures",async()=>{
 const {viewport}=await canvas("adjust",true);
 fireEvent.pointerDown(viewport,{pointerId:1,button:0,clientX:400,clientY:300});
 fireEvent.pointerMove(viewport,{pointerId:1,clientX:250,clientY:180});
 expect(viewport.scrollLeft).toBe(200);expect(viewport.setPointerCapture).not.toHaveBeenCalled();
});

it("target sampling captures one drag without pan and accumulates fine movement",async()=>{
 const gesture=vi.fn();const {viewport}=await canvas("adjust",true,gesture);
 const frame=screen.getByTestId("editor-canvas-frame");frame.getBoundingClientRect=()=>({left:100,top:50,width:400,height:300} as DOMRect);
 fireEvent.pointerDown(viewport,{pointerId:1,button:0,clientX:300,clientY:200});
 expect(gesture).toHaveBeenLastCalledWith({phase:"start",x:0.5,y:0.5,delta:0});
 fireEvent.pointerMove(viewport,{pointerId:1,clientX:300,clientY:170});
 fireEvent.pointerMove(viewport,{pointerId:1,clientX:300,clientY:140,altKey:true});
 fireEvent.pointerUp(viewport,{pointerId:1});
 expect(gesture).toHaveBeenLastCalledWith(expect.objectContaining({phase:"end",delta:0.11000000000000001}));
 expect(viewport.scrollLeft).toBe(200);expect(viewport.scrollTop).toBe(150);
});
