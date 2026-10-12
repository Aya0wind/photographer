import {beforeEach,afterEach,expect,it,vi} from "vitest";
import {fireEvent,render,screen} from "@testing-library/react";
import type {EditLocalMask} from "@/ipc/api";
import MaskCanvasControls,{maskGestureOwnsKey,type MaskBrush} from "./MaskCanvasControls";
import {advancedRecipe} from "../lib/advancedRecipe";
const mask:EditLocalMask={id:"m",name:"Area",kind:"brush",enabled:true,inverted:false,density:100,feather:0,from:{x:0.2,y:0.5},to:{x:0.8,y:0.5},strokes:[]};
const brush={widthRel:0.1,hardness:1,opacity:1,flow:1,erase:false};
beforeEach(()=>vi.stubGlobal("PointerEvent",class extends MouseEvent {pointerId:number;constructor(type:string,options:PointerEventInit={}){super(type,options);this.pointerId=options.pointerId??1;}}));
afterEach(()=>vi.unstubAllGlobals());
function setup(kind:EditLocalMask["kind"]="brush",rotated=false){
 const recipe=advancedRecipe();if(rotated){recipe.rotateQuarter=1;recipe.crop={x:0.1,y:0.2,w:0.8,h:0.6};}
 const onCommit=vi.fn();const view=render(<MaskCanvasControls mask={{...mask,kind}} recipe={recipe} size={{width:600,height:400}} width={480} height={320} brush={brush} onCommit={onCommit}/>);
 const svg=screen.getByTestId("editor-mask-controls");
 svg.getBoundingClientRect=()=>({left:0,top:0,width:480,height:320} as DOMRect);
 svg.setPointerCapture=vi.fn();
 return {svg,onCommit,rerenderBrush:(next:MaskBrush)=>view.rerender(<MaskCanvasControls mask={{...mask,kind}} recipe={recipe} size={{width:600,height:400}} width={480} height={320} brush={next} onCommit={onCommit}/>)};
}
it("keeps brush settings fixed for the current stroke when tool settings change",()=>{
 const {svg,onCommit,rerenderBrush}=setup();fireEvent.pointerDown(svg,{button:0,clientX:120,clientY:80});
 rerenderBrush({...brush,widthRel:0.2,opacity:0.3,flow:0.4,erase:true});fireEvent.pointerMove(svg,{clientX:240,clientY:160});
 expect(JSON.parse(svg.getAttribute("data-native-mask-stroke")!)).toMatchObject(brush);fireEvent.pointerUp(svg);expect(onCommit.mock.calls[0][0].strokes[0]).toMatchObject(brush);
});
it("commits one brush stroke in original coordinates after rotation and cropping",()=>{
 const {svg,onCommit}=setup("brush",true);
 fireEvent.pointerDown(svg,{button:0,clientX:120,clientY:80,pointerId:1});
 fireEvent.pointerMove(svg,{clientX:240,clientY:160,pointerId:1});
 expect(onCommit).not.toHaveBeenCalled();fireEvent.pointerUp(svg,{pointerId:1});
 expect(svg).not.toHaveAttribute("data-native-mask-stroke");
 expect(onCommit).toHaveBeenCalledTimes(1);
 expect(onCommit.mock.calls[0][0].strokes[0].points[0].x).toBeCloseTo(0.35);
 expect(onCommit.mock.calls[0][0].strokes[0].points[0].y).toBeCloseTo(0.7);
 expect(onCommit.mock.calls[0][0].strokes[0].points[1]).toEqual({x:0.5,y:0.5});
});
it("cancels a stroke without changing the recipe",()=>{
 const {svg,onCommit}=setup();fireEvent.pointerDown(svg,{button:0,clientX:120,clientY:80});
 const preview=JSON.parse(svg.getAttribute("data-native-mask-stroke")!);expect(preview.points).toEqual([{x:0.25,y:0.25}]);expect(preview).toMatchObject(brush);expect(JSON.parse(svg.getAttribute("data-native-mask-guide")!)[6]).toBe(3);
 fireEvent.keyDown(svg,{key:"Escape"});fireEvent.pointerUp(svg);
 expect(svg).not.toHaveAttribute("data-native-mask-stroke");
 expect(onCommit).not.toHaveBeenCalled();
});
it("creates a gradient and can move one endpoint independently",()=>{
 const {svg,onCommit}=setup("linear");
 fireEvent.pointerDown(svg,{button:0,clientX:120,clientY:160});fireEvent.pointerMove(svg,{clientX:360,clientY:160});fireEvent.pointerUp(svg);
 expect(onCommit).toHaveBeenLastCalledWith({from:{x:0.25,y:0.5},to:{x:0.75,y:0.5}});
 const handle=svg.querySelector('[data-mask-handle="to"]')!;
 fireEvent.pointerDown(handle,{button:0,clientX:384,clientY:160});fireEvent.pointerMove(svg,{clientX:240,clientY:80});fireEvent.pointerUp(svg);
 expect(onCommit).toHaveBeenLastCalledWith({to:{x:0.5,y:0.25}});
});
it("exposes projected native gradient guides while preserving the committed mask until pointer-up",()=>{
 const {svg,onCommit}=setup("radial",true);const guide=()=>JSON.parse(svg.getAttribute("data-native-mask-guide")!);
 expect(svg).toHaveAttribute("data-native-mask-id","m");guide().slice(0,4).forEach((value:number,index:number)=>expect(value).toBeCloseTo([0.5,0,0.5,1][index]));expect(guide()[6]).toBe(2);
 fireEvent.pointerDown(svg,{button:0,clientX:120,clientY:80});fireEvent.pointerMove(svg,{clientX:240,clientY:160});guide().slice(0,4).forEach((value:number,index:number)=>expect(value).toBeCloseTo([0.25,0.25,0.5,0.5][index]));expect(onCommit).not.toHaveBeenCalled();
 expect(JSON.parse(svg.getAttribute("data-native-mask-preview")!)).toEqual([0.35,0.7,0.5,0.5]);
 fireEvent.keyDown(svg,{key:"Escape"});guide().slice(0,4).forEach((value:number,index:number)=>expect(value).toBeCloseTo([0.5,0,0.5,1][index]));expect(onCommit).not.toHaveBeenCalled();
 expect(svg).not.toHaveAttribute("data-native-mask-preview");
});
it("reserves cancel/save shortcuts during a gesture before editor-level capture handlers",()=>{
 const {svg,onCommit}=setup("linear"),editor=vi.fn();const capture=(event:KeyboardEvent)=>{if(!maskGestureOwnsKey(event))editor(event);};window.addEventListener("keydown",capture,true);
 try{fireEvent.pointerDown(svg,{button:0,clientX:120,clientY:160});fireEvent.pointerMove(svg,{clientX:360,clientY:160});fireEvent.keyDown(svg,{key:"s",ctrlKey:true});expect(svg).toHaveAttribute("data-mask-gesture","active");
 fireEvent.keyDown(svg,{key:"z",ctrlKey:true});expect(svg).not.toHaveAttribute("data-mask-gesture");fireEvent.pointerUp(svg);expect(onCommit).not.toHaveBeenCalled();expect(editor).not.toHaveBeenCalled();
 fireEvent.keyDown(svg,{key:"Escape"});expect(editor).toHaveBeenCalledTimes(1);
 }finally{window.removeEventListener("keydown",capture,true);}
});
