import {useReducer} from "react";
import {act,renderHook,waitFor} from "@testing-library/react";
import {beforeEach,expect,it,vi} from "vitest";
import {editPreviewPick} from "@/ipc/api";
import {useTargetAdjustment} from "./useTargetAdjustment";
import {advancedRecipe,advancedReducer,type AdvancedAction} from "./advancedRecipe";
import {initRecipeHistory,type RecipeHistory} from "./recipe";
vi.mock("@/ipc/api",()=>({editPreviewPick:vi.fn()}));
beforeEach(()=>vi.clearAllMocks());
const event=(phase:"start"|"move"|"end"|"cancel",delta=0)=>({phase,x:0.5,y:0.5,delta});
function setup(mode:"curveTarget"|"hslTarget"="curveTarget",local=false){
 const recipe=advancedRecipe();if(local)recipe.masks=[{id:"m",name:"Area",kind:"brush",enabled:true,inverted:false,density:100,feather:0,from:{x:0,y:0},to:{x:1,y:1},strokes:[]}];
 const initial=initRecipeHistory(recipe),onError=vi.fn();
 return {...renderHook(({mode,sampleSize}: {mode:"curveTarget"|"hslTarget"|null;sampleSize?:1|3|5|11})=>{
  const [history,dispatch]=useReducer((s:RecipeHistory,a:AdvancedAction)=>advancedReducer(s,a,{width:600,height:400}),initial);
  const target=useTargetAdjustment({sampleSize,sessionId:"s",history,mode,maskId:local?"m":undefined,channel:"rgb",axis:"lightness",dispatch,onError,onSample:()=>{}});
  return {history,target,dispatch};
 },{initialProps:{mode:mode as "curveTarget"|"hslTarget"|null,sampleSize:undefined as 1|3|5|11|undefined}}),recipe,onError};
}
it("forwards the selected area and cancels an in-flight gesture when its size changes",async()=>{
 let resolve!:(value:{sample:[number,number,number]})=>void;vi.mocked(editPreviewPick).mockImplementationOnce(()=>new Promise(done=>{resolve=done;}));
 const {result,recipe,rerender}=setup();rerender({mode:"curveTarget",sampleSize:5});
 act(()=>result.current.target.onGesture(event("start")));expect(editPreviewPick).toHaveBeenLastCalledWith("s",expect.anything(),0.5,0.5,"point",undefined,"rgb",5);
 act(()=>result.current.target.onGesture(event("move",0.2)));rerender({mode:"curveTarget",sampleSize:11});await act(async()=>resolve({sample:[128,128,128]}));
 expect(result.current.history.present).toEqual(recipe);expect(result.current.target.active).toBe(false);
});
it("updates from the sampled baseline and commits one undo step per complete drag",async()=>{
 vi.mocked(editPreviewPick).mockResolvedValue({sample:[128,128,128]});const {result,recipe}=setup();
 act(()=>result.current.target.onGesture(event("start")));await waitFor(()=>expect(result.current.target.busy).toBe(false));
 act(()=>result.current.target.onGesture(event("move",0.1)));act(()=>result.current.target.onGesture(event("move",0.2)));
 expect(result.current.history.present.advanced!.curves.find(p=>p[0]===128)![1]).toBe(179);
 expect(result.current.history.past).toHaveLength(0);act(()=>result.current.target.onGesture(event("end",0.2)));expect(result.current.history.past).toHaveLength(1);
 act(()=>result.current.dispatch({type:"undo"}));expect(result.current.history.present).toEqual(recipe);
});
it("cancels an applied drag and ignores a pending sample after switching modes",async()=>{
 vi.mocked(editPreviewPick).mockResolvedValueOnce({sample:[128,128,128]});const {result,recipe,rerender}=setup();
 act(()=>result.current.target.onGesture(event("start")));await waitFor(()=>expect(result.current.target.busy).toBe(false));
 act(()=>result.current.target.onGesture(event("move",0.1)));act(()=>result.current.target.cancel());expect(result.current.history.present).toEqual(recipe);expect(result.current.history.past).toHaveLength(0);
 let resolve!:(v:{sample:[number,number,number]})=>void;vi.mocked(editPreviewPick).mockImplementationOnce(()=>new Promise(r=>resolve=r));
 act(()=>result.current.target.onGesture(event("start")));rerender({mode:null,sampleSize:undefined});await act(async()=>resolve({sample:[80,100,120]}));
 expect(result.current.history.present).toEqual(recipe);expect(result.current.target.active).toBe(false);
});
it("keeps movement while sampling and applies it once even if pointer-up happens first",async()=>{
 let resolve!:(v:{sample:[number,number,number]})=>void;vi.mocked(editPreviewPick).mockImplementationOnce(()=>new Promise(r=>resolve=r));const {result}=setup();
 act(()=>{result.current.target.onGesture(event("start"));result.current.target.onGesture(event("move",0.1));result.current.target.onGesture(event("end",0.1));});
 expect(result.current.history.past).toHaveLength(0);await act(async()=>resolve({sample:[128,128,128]}));
 expect(result.current.history.past).toHaveLength(1);expect(result.current.history.present.advanced!.curves.find(p=>p[0]===128)![1]).toBe(153.5);expect(result.current.target.active).toBe(false);
});
it("changes only the sampled range in the selected mask",async()=>{
 vi.mocked(editPreviewPick).mockResolvedValue({sample:[40,70,220],range:"blues"});const {result}=setup("hslTarget",true);
 act(()=>result.current.target.onGesture(event("start")));await waitFor(()=>expect(result.current.target.busy).toBe(false));
 act(()=>result.current.target.onGesture(event("end",0.2)));
 expect(result.current.history.present.masks![0].advanced!.hsl).toEqual({blues:{hue:0,saturation:0,lightness:20}});expect(result.current.history.present.advanced!.hsl).toBeUndefined();
 expect(editPreviewPick).toHaveBeenCalledWith("s",expect.anything(),0.5,0.5,"hsl","m","rgb",1);
});

it("restores redo history on cancellation",async()=>{
 vi.mocked(editPreviewPick).mockResolvedValueOnce({sample:[128,128,128]});const {result,recipe}=setup();
 const redo={...advancedRecipe(),advanced:{...advancedRecipe().advanced!,exposure:1}};
 act(()=>result.current.dispatch({type:"restoreHistory",history:{past:[],present:recipe,future:[redo]}}));
 act(()=>result.current.target.onGesture(event("start")));await waitFor(()=>expect(result.current.target.busy).toBe(false));
 act(()=>result.current.target.onGesture(event("move",0.1)));expect(result.current.history.future).toHaveLength(0);
 act(()=>result.current.target.cancel());expect(result.current.history.future).toEqual([redo]);
});

it("reports a gray HSL target without changing the recipe",async()=>{
 vi.mocked(editPreviewPick).mockResolvedValue({sample:[128,128,128]});const {result,recipe,onError}=setup("hslTarget");
 act(()=>result.current.target.onGesture(event("start")));await waitFor(()=>expect(result.current.target.active).toBe(false));
 expect(onError).toHaveBeenLastCalledWith(expect.stringContaining("target-hsl-no-color"));
 expect(result.current.history.present).toEqual(recipe);expect(result.current.history.past).toHaveLength(0);
});
