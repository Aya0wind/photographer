import {useReducer} from "react";
import {expect,it,vi,afterEach} from "vitest";
import {render,screen,fireEvent,act} from "@testing-library/react";
import {I18nextProvider} from "react-i18next";
import i18n from "@/i18n";
import ParametricCurvePanel,{DEFAULT_PARAMETRIC} from "./ParametricCurvePanel";
import {editParametricSamples} from "@/ipc/api";
vi.mock("@/ipc/api",()=>({editParametricSamples:vi.fn()}));
afterEach(()=>{vi.useRealTimers();vi.clearAllMocks();});
import {advancedRecipe,advancedReducer,type AdvancedAction} from "../lib/advancedRecipe";
import {initRecipeHistory,type RecipeHistory} from "../lib/recipe";
function Harness(){const [history,dispatch]=useReducer((s:RecipeHistory,a:AdvancedAction)=>advancedReducer(s,a,{width:600,height:400}),initRecipeHistory(advancedRecipe()));return <I18nextProvider i18n={i18n}><ParametricCurvePanel value={history.present.advanced!.parametricCurve} onChange={(parametricCurve,record)=>dispatch({type:"advanced",patch:{parametricCurve},record})} onReset={()=>dispatch({type:"resetAdjustmentGroup",group:"parametricCurve"})} begin={()=>{}} end={()=>{}}/><button onClick={()=>dispatch({type:"undo"})}>Undo</button><output data-testid="state">{JSON.stringify(history)}</output></I18nextProvider>;}
function number(label:string,value:string){const input=screen.getByRole("spinbutton",{name:label});fireEvent.focus(input);fireEvent.change(input,{target:{value}});fireEvent.blur(input);}
it("limits neighbouring boundaries, resets single regions and undoes the complete group reset",()=>{
 const view=render(<Harness/>);view.container.querySelectorAll("details").forEach(d=>d.open=true);
 number("阴影分区","30");number("阴影与暗部边界","99");
 let state=JSON.parse(screen.getByTestId("state").textContent!);expect(state.present.advanced.parametricCurve.splits).toEqual([49,50,75]);
 fireEvent.click(screen.getByRole("button",{name:"重置阴影分区"}));state=JSON.parse(screen.getByTestId("state").textContent!);expect(state.present.advanced.parametricCurve.shadows).toBe(0);expect(state.present.advanced.parametricCurve.splits[0]).toBe(49);
 fireEvent.click(screen.getByRole("button",{name:"重置曲线分区"}));expect(JSON.parse(screen.getByTestId("state").textContent!).present.advanced.parametricCurve).toBeUndefined();
 fireEvent.click(screen.getByRole("button",{name:"Undo"}));expect(JSON.parse(screen.getByTestId("state").textContent!).present.advanced.parametricCurve.splits[0]).toBe(49);
});
function panel(value={...DEFAULT_PARAMETRIC,shadows:20},onModalChange=vi.fn()){
 return <I18nextProvider i18n={i18n}><ParametricCurvePanel value={value} onChange={vi.fn()} onReset={vi.fn()} begin={vi.fn()} end={vi.fn()} onModalChange={onModalChange}/></I18nextProvider>;
}
async function toggle(view:ReturnType<typeof render>,open:boolean){await act(async()=>{const details=view.container.querySelector("details")!;details.open=open;fireEvent(details,new Event("toggle"));});}
async function tick(){await act(async()=>{await vi.advanceTimersByTimeAsync(32);});}
it("samples only expanded active curves and rejects outdated results after parameter changes",async()=>{
 vi.useFakeTimers();let resolveOld!:(samples:number[])=>void;
 vi.mocked(editParametricSamples).mockImplementationOnce(()=>new Promise(resolve=>{resolveOld=resolve;})).mockResolvedValueOnce(Array(1024).fill(200));
 const view=render(panel());await tick();expect(editParametricSamples).not.toHaveBeenCalled();
 await toggle(view,true);await tick();expect(editParametricSamples).toHaveBeenCalledTimes(1);
 view.rerender(panel({...DEFAULT_PARAMETRIC,shadows:40}));await tick();
 const graph=()=>view.container.querySelector("polyline")!.getAttribute("points");const current=graph();
 const points=current!.split(" ");expect(points).toHaveLength(1024);expect(points[points.length-1]).toMatch(/^100,/);
 await act(async()=>resolveOld(Array(256).fill(10)));expect(graph()).toBe(current);
 await toggle(view,false);view.rerender(panel({...DEFAULT_PARAMETRIC,shadows:60}));await tick();expect(editParametricSamples).toHaveBeenCalledTimes(2);
});
it("shows failures in an acknowledged modal and releases the native canvas when dismissed",async()=>{
 vi.useFakeTimers();vi.mocked(editParametricSamples).mockRejectedValueOnce(new Error("Curve preview failed"));
 const modal=vi.fn(),view=render(panel(undefined,modal));await toggle(view,true);await tick();
 expect(screen.getByRole("alertdialog")).toHaveTextContent("Curve preview failed");expect(modal).toHaveBeenLastCalledWith(true);
 const editorEscape=vi.fn();window.addEventListener("keydown",editorEscape);
 fireEvent.keyDown(window,{key:"Escape"});window.removeEventListener("keydown",editorEscape);
 expect(editorEscape).not.toHaveBeenCalled();expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();expect(modal).toHaveBeenLastCalledWith(false);
});
it("restores a neutral graph immediately and ignores a previous in-flight response",async()=>{
 vi.useFakeTimers();let resolve!:(samples:number[])=>void;vi.mocked(editParametricSamples).mockImplementationOnce(()=>new Promise(done=>{resolve=done;}));
 const view=render(panel());await toggle(view,true);await tick();view.rerender(panel(DEFAULT_PARAMETRIC));
 const neutral=view.container.querySelector("polyline")!.getAttribute("points");
 await act(async()=>resolve(Array(256).fill(100)));expect(view.container.querySelector("polyline")!.getAttribute("points")).toBe(neutral);
 expect(editParametricSamples).toHaveBeenCalledTimes(1);
});
