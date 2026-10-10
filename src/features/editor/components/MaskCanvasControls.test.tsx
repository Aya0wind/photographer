import {beforeEach,afterEach,expect,it,vi} from "vitest";
import {fireEvent,render,screen} from "@testing-library/react";
import type {EditLocalMask} from "@/ipc/api";
import MaskCanvasControls from "./MaskCanvasControls";
import {advancedRecipe} from "../lib/advancedRecipe";
const mask:EditLocalMask={id:"m",name:"Area",kind:"brush",enabled:true,inverted:false,density:100,feather:0,from:{x:0.2,y:0.5},to:{x:0.8,y:0.5},strokes:[]};
const brush={widthRel:0.1,hardness:1,opacity:1,flow:1,erase:false};
beforeEach(()=>vi.stubGlobal("PointerEvent",class extends MouseEvent {pointerId:number;constructor(type:string,options:PointerEventInit={}){super(type,options);this.pointerId=options.pointerId??1;}}));
afterEach(()=>vi.unstubAllGlobals());
function setup(kind:EditLocalMask["kind"]="brush",rotated=false){
 const recipe=advancedRecipe();if(rotated){recipe.rotateQuarter=1;recipe.crop={x:0.1,y:0.2,w:0.8,h:0.6};}
 const onCommit=vi.fn();render(<MaskCanvasControls mask={{...mask,kind}} recipe={recipe} size={{width:600,height:400}} width={480} height={320} brush={brush} onCommit={onCommit}/>);
 const svg=screen.getByTestId("editor-mask-controls");
 svg.getBoundingClientRect=()=>({left:0,top:0,width:480,height:320} as DOMRect);
 svg.setPointerCapture=vi.fn();
 return {svg,onCommit};
}
it("commits one brush stroke in original coordinates after rotation and cropping",()=>{
 const {svg,onCommit}=setup("brush",true);
 fireEvent.pointerDown(svg,{button:0,clientX:120,clientY:80,pointerId:1});
 fireEvent.pointerMove(svg,{clientX:240,clientY:160,pointerId:1});
 expect(onCommit).not.toHaveBeenCalled();fireEvent.pointerUp(svg,{pointerId:1});
 expect(onCommit).toHaveBeenCalledTimes(1);
 expect(onCommit.mock.calls[0][0].strokes[0].points[0].x).toBeCloseTo(0.35);
 expect(onCommit.mock.calls[0][0].strokes[0].points[0].y).toBeCloseTo(0.7);
 expect(onCommit.mock.calls[0][0].strokes[0].points[1]).toEqual({x:0.5,y:0.5});
});
it("cancels a stroke without changing the recipe",()=>{
 const {svg,onCommit}=setup();fireEvent.pointerDown(svg,{button:0,clientX:120,clientY:80});
 fireEvent.keyDown(svg,{key:"Escape"});fireEvent.pointerUp(svg);
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
