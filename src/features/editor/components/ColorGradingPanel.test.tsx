import {useReducer,useRef} from "react";
import {beforeEach,afterEach,expect,it,vi} from "vitest";
import {fireEvent,render,screen} from "@testing-library/react";
import {I18nextProvider} from "react-i18next";
import i18n from "@/i18n";
import ColorGradingPanel from "./ColorGradingPanel";
import {advancedRecipe,advancedReducer,type AdvancedAction} from "../lib/advancedRecipe";
import {initRecipeHistory,recipeEquals,type RecipeHistory} from "../lib/recipe";
beforeEach(()=>vi.stubGlobal("PointerEvent",class extends MouseEvent{pointerId:number;constructor(type:string,o:PointerEventInit={}){super(type,o);this.pointerId=o.pointerId??1;}}));afterEach(()=>vi.unstubAllGlobals());
function Harness(){const [history,dispatch]=useReducer((s:RecipeHistory,a:AdvancedAction)=>advancedReducer(s,a,{width:600,height:400}),initRecipeHistory(advancedRecipe()));const snapshot=useRef<RecipeHistory|null>(null);return <I18nextProvider i18n={i18n}><ColorGradingPanel value={history.present.advanced!.colorGrading} onChange={(colorGrading,record)=>dispatch({type:"advanced",patch:{colorGrading},record})} onReset={()=>dispatch({type:"resetAdjustmentGroup",group:"colorGrading"})} begin={()=>{snapshot.current=history;}} end={()=>{if(snapshot.current&&!recipeEquals(snapshot.current.present,history.present))dispatch({type:"commitFrom",snapshot:snapshot.current.present});snapshot.current=null;}} cancel={()=>{if(snapshot.current)dispatch({type:"restoreHistory",history:snapshot.current});snapshot.current=null;}}/><output data-testid="state">{JSON.stringify(history)}</output></I18nextProvider>;}
function setup(){const view=render(<Harness/>);view.container.querySelectorAll("details").forEach(d=>d.open=true);const wheel=screen.getByRole("application",{name:"分级色轮"});wheel.getBoundingClientRect=()=>({left:0,top:0,width:200,height:200} as DOMRect);wheel.setPointerCapture=vi.fn();return {wheel,state:()=>JSON.parse(screen.getByTestId("state").textContent!)};}
it("records one complete color-wheel drag and restores the previous state on cancellation",()=>{
 const {wheel,state}=setup();fireEvent.pointerDown(wheel,{button:0,clientX:150,clientY:100});fireEvent.pointerMove(wheel,{clientX:100,clientY:175});expect(state().past).toHaveLength(0);fireEvent.pointerUp(wheel);
 expect(state().present.advanced.colorGrading.midtones.hue).toBe(90);expect(state().present.advanced.colorGrading.midtones.sat).toBe(75);expect(state().past).toHaveLength(1);
 const before=state();fireEvent.pointerDown(wheel,{button:0,clientX:130,clientY:140});fireEvent.keyDown(wheel,{key:"Escape"});expect(state()).toEqual(before);
});
it("keeps other tonal wheels when resetting the current tone and supports keyboard input",()=>{
 const {wheel,state}=setup();fireEvent.keyDown(wheel,{key:"ArrowUp"});expect(state().present.advanced.colorGrading.midtones.sat).toBe(1);
 fireEvent.change(screen.getByLabelText("分级范围"),{target:{value:"shadows"}});const sat=screen.getByRole("spinbutton",{name:"分级饱和度"});fireEvent.focus(sat);fireEvent.change(sat,{target:{value:"30"}});fireEvent.blur(sat);
 fireEvent.click(screen.getByRole("button",{name:"重置当前色调"}));expect(state().present.advanced.colorGrading.shadows.sat).toBe(0);expect(state().present.advanced.colorGrading.midtones.sat).toBe(1);
 fireEvent.click(screen.getByRole("button",{name:"重置色彩分级"}));expect(state().present.advanced.colorGrading).toBeUndefined();
});
it("supports precision movement, hue lock and avoids edits for a neutral center click",()=>{
 const {wheel,state}=setup();fireEvent.pointerDown(wheel,{button:0,clientX:100,clientY:100});fireEvent.pointerUp(wheel);expect(state().past).toHaveLength(0);
 fireEvent.pointerDown(wheel,{button:0,clientX:100,clientY:150,altKey:true});fireEvent.pointerUp(wheel);expect(state().present.advanced.colorGrading.midtones.hue).toBe(9);expect(state().present.advanced.colorGrading.midtones.sat).toBe(5);
 fireEvent.pointerDown(wheel,{button:0,clientX:150,clientY:100,shiftKey:true});fireEvent.pointerUp(wheel);expect(state().present.advanced.colorGrading.midtones.hue).toBe(9);expect(state().present.advanced.colorGrading.midtones.sat).toBe(50);
});
