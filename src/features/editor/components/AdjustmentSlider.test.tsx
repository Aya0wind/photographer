import {useReducer,useRef} from "react";
import {expect,it,vi,beforeEach,afterEach} from "vitest";
import {render,screen,fireEvent} from "@testing-library/react";
import {I18nextProvider} from "react-i18next";
import i18n from "@/i18n";
import Slider from "./AdjustmentSlider";
import {advancedRecipe,advancedReducer,type AdvancedAction} from "../lib/advancedRecipe";
import {initRecipeHistory,recipeEquals,type RecipeHistory} from "../lib/recipe";
beforeEach(()=>vi.stubGlobal("PointerEvent",class extends MouseEvent{pointerId:number;constructor(type:string,o:PointerEventInit={}){super(type,o);this.pointerId=o.pointerId??1;}}));
afterEach(()=>vi.unstubAllGlobals());
function Harness(){
 const [history,dispatch]=useReducer((s:RecipeHistory,a:AdvancedAction)=>advancedReducer(s,a,{width:600,height:400}),initRecipeHistory(advancedRecipe()));
 const snapshot=useRef<RecipeHistory|null>(null);
 const value=history.present.advanced!.exposure;
 return <I18nextProvider i18n={i18n}><Slider label="Exposure" value={value} min={-5} max={5} step={0.1} onChange={exposure=>dispatch({type:"advanced",patch:{exposure},record:false})} onSet={exposure=>dispatch({type:"advanced",patch:{exposure}})} onReset={()=>dispatch({type:"advanced",patch:{exposure:0}})} begin={()=>{snapshot.current??=history;}} end={()=>{if(snapshot.current&&!recipeEquals(snapshot.current.present,history.present))dispatch({type:"commitFrom",snapshot:snapshot.current.present});snapshot.current=null;}}/><button onClick={()=>dispatch({type:"undo"})}>Undo</button><button onClick={()=>{if(snapshot.current)dispatch({type:"restoreHistory",history:snapshot.current});snapshot.current=null;}}>Cancel</button><output data-testid="state">{JSON.stringify(history)}</output></I18nextProvider>;
}
const state=()=>JSON.parse(screen.getByTestId("state").textContent!) as RecipeHistory;
it("centers an asymmetric range and exposes real values to input and keyboard users",()=>{
 const change=vi.fn();
 render(<I18nextProvider i18n={i18n}><Slider label="Exposure" value={0} min={-5} max={7} step={.1} onChange={change} onSet={vi.fn()} onReset={vi.fn()} begin={vi.fn()} end={vi.fn()}/></I18nextProvider>);
 const range=screen.getByRole("slider");
 expect(range).toHaveValue("1");
 expect(range).toHaveAttribute("aria-valuenow","0");
 expect(screen.getByRole("spinbutton")).toHaveValue(0);
 fireEvent.change(range,{target:{value:"7"}});expect(change).toHaveBeenLastCalledWith(7);
 fireEvent.change(range,{target:{value:"-5"}});expect(change).toHaveBeenLastCalledWith(-5);
 fireEvent.keyDown(range,{key:"ArrowRight"});expect(change).toHaveBeenLastCalledWith(.1);
});
function pointerRange(){const range=screen.getByRole("slider");range.getBoundingClientRect=()=>({left:0,top:0,width:100,height:16} as DOMRect);range.setPointerCapture=vi.fn();return range;}
it("supports fine pointer dragging and changing Alt without a value jump",()=>{
 render(<Harness/>);const range=pointerRange();fireEvent.pointerDown(range,{button:0,clientX:20,altKey:true});expect(state().present.advanced!.exposure).toBe(0);
 fireEvent.pointerMove(range,{clientX:30,altKey:true});expect(state().present.advanced!.exposure).toBe(0.1);
 fireEvent.pointerMove(range,{clientX:40,altKey:false});expect(state().present.advanced!.exposure).toBe(1.1);
 fireEvent.pointerMove(range,{clientX:50,altKey:true});expect(state().present.advanced!.exposure).toBe(1.2);expect(state().past).toHaveLength(0);
 fireEvent.pointerUp(range);expect(state().past).toHaveLength(1);fireEvent.click(screen.getByText("Undo"));expect(state().present.advanced!.exposure).toBe(0);
});
it("does not continue a fine drag after external gesture cancellation",()=>{
 render(<Harness/>);const range=pointerRange();fireEvent.pointerDown(range,{button:0,clientX:20,altKey:true});fireEvent.pointerMove(range,{clientX:30,altKey:true});
 fireEvent.click(screen.getByText("Cancel"));expect(state().present.advanced!.exposure).toBe(0);
 fireEvent.pointerMove(range,{clientX:70,altKey:true});fireEvent.pointerUp(range);expect(state().present.advanced!.exposure).toBe(0);expect(state().past).toHaveLength(0);
});
it("uses precise and coarse keyboard steps with one undo for a held key",()=>{
 render(<Harness/>);const range=screen.getByRole("slider");
 fireEvent.keyDown(range,{key:"ArrowRight",altKey:true});fireEvent.keyDown(range,{key:"ArrowRight",altKey:true,repeat:true});
 expect(state().present.advanced!.exposure).toBe(0.02);expect(state().past).toHaveLength(0);
 fireEvent.keyUp(range,{key:"ArrowRight"});expect(state().past).toHaveLength(1);
 fireEvent.click(screen.getByText("Undo"));expect(state().present.advanced!.exposure).toBe(0);
 fireEvent.keyDown(range,{key:"ArrowUp",shiftKey:true});fireEvent.keyUp(range,{key:"ArrowUp"});expect(state().present.advanced!.exposure).toBe(1);
});
it("clamps keyboard navigation and finishes a held gesture on focus loss",()=>{
 render(<Harness/>);const range=screen.getByRole("slider");
 fireEvent.keyDown(range,{key:"End"});fireEvent.keyDown(range,{key:"ArrowRight",shiftKey:true});expect(state().present.advanced!.exposure).toBe(5);
 fireEvent.blur(range);expect(state().past).toHaveLength(1);fireEvent.click(screen.getByText("Undo"));expect(state().present.advanced!.exposure).toBe(0);
 fireEvent.keyDown(range,{key:"Home"});fireEvent.keyUp(range,{key:"Home"});expect(state().present.advanced!.exposure).toBe(-5);
 fireEvent.keyDown(range,{key:"PageUp"});fireEvent.keyUp(range,{key:"PageUp"});expect(state().present.advanced!.exposure).toBe(-4);
});
it("keeps modified numeric steps as a draft until confirmation and cancels on Escape",()=>{
 render(<Harness/>);const input=screen.getByRole("spinbutton");input.focus();
 fireEvent.keyDown(input,{key:"ArrowUp",altKey:true});fireEvent.keyDown(input,{key:"ArrowUp",altKey:true});expect(input).toHaveValue(0.02);expect(state().present.advanced!.exposure).toBe(0);
 fireEvent.keyDown(input,{key:"Enter"});expect(state().present.advanced!.exposure).toBe(0.02);expect(state().past).toHaveLength(1);
 input.focus();fireEvent.keyDown(input,{key:"ArrowDown",shiftKey:true});fireEvent.keyDown(input,{key:"Escape"});expect(state().present.advanced!.exposure).toBe(0.02);expect(state().past).toHaveLength(1);
});
