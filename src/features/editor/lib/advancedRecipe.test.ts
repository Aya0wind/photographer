import {expect,it} from "vitest";
import {advancedRecipe,advancedReducer,DEFAULT_DEVELOPMENT,adjustmentGroupChanged} from "./advancedRecipe";
import {initRecipeHistory} from "./recipe";
import type {EditLocalMask} from "@/ipc/api";
const context={width:6000,height:4000};
it("reorders the mask stack as an undoable operation while preserving each mask",()=>{
 const recipe=advancedRecipe();const base:EditLocalMask={id:"a",name:"First",kind:"linear",enabled:true,inverted:false,density:100,feather:0,from:{x:0,y:0},to:{x:1,y:1},strokes:[]};
 recipe.masks=[base,{...base,id:"b",name:"Second",advanced:{exposure:1,temperature:0,tint:0,vibrance:0,curves:[]}}];const initial=initRecipeHistory(recipe);
 const changed=advancedReducer(initial,{type:"maskReorder",id:"a",targetId:"b"},context);
 expect(changed.present.masks).toEqual([recipe.masks![1],recipe.masks![0]]);expect(advancedReducer(changed,{type:"undo"},context).present).toEqual(recipe);
 expect(advancedReducer(initial,{type:"maskReorder",id:"missing",targetId:"a"},context)).toBe(initial);
});
it("applying a portable scheme is one undoable edit and cancels a temporary crop session",()=>{
 const original=advancedRecipe(),imported=advancedRecipe();imported.advanced!.exposure=2;
 let initial=initRecipeHistory(original);initial=advancedReducer(initial,{type:"cropStart"},context);initial=advancedReducer(initial,{type:"rotate",delta:1},context);
 const next=advancedReducer(initial,{type:"applyProject",recipe:imported},context);
 expect(next.present).toEqual(imported);expect(next.cropSession).toBeUndefined();expect(next.past).toHaveLength(1);
 expect(advancedReducer(next,{type:"undo"},context).present).toEqual(original);
 expect(advancedReducer(initRecipeHistory(original),{type:"applyProject",recipe:original},context)).toEqual(initRecipeHistory(original));
});
it("Camera Raw groups reset all their controls atomically without changing other groups",()=>{
 const recipe=advancedRecipe();recipe.advanced!.development={...DEFAULT_DEVELOPMENT,highlights:35,shadows:25,whites:-10,blacks:15,texture:20,grainSize:40,sharpenAmount:50};
 expect(adjustmentGroupChanged(recipe,"light")).toBe(true);
 const light=advancedReducer(initRecipeHistory(recipe),{type:"resetAdjustmentGroup",group:"light"},context);
 expect(light.present.advanced!.development).toEqual({...recipe.advanced!.development,highlights:0,shadows:0,whites:0,blacks:0});
 expect(light.past).toHaveLength(1);expect(advancedReducer(light,{type:"undo"},context).present).toEqual(recipe);
 const effects=advancedReducer(initRecipeHistory(recipe),{type:"resetAdjustmentGroup",group:"effects"},context);
 expect(effects.present.advanced!.development).toEqual({...recipe.advanced!.development,texture:0,grainSize:25});
 expect(effects.past).toHaveLength(1);expect(adjustmentGroupChanged(effects.present,"effects")).toBe(false);
 const detail=advancedReducer(initRecipeHistory(recipe),{type:"resetAdjustmentGroup",group:"detail"},context);
 expect(detail.present.advanced!.development).toEqual({...recipe.advanced!.development,sharpenAmount:0});
});
it("local tone reset includes highlights and shadows while preserving global tone and other masks",()=>{
 const recipe=advancedRecipe();recipe.advanced!.development={...DEFAULT_DEVELOPMENT,shadows:80};
 const local=mask();local.advanced!.development={...DEFAULT_DEVELOPMENT,highlights:20,shadows:40,clarity:15};recipe.masks=[local,{...structuredClone(local),id:"m2"}];
 const next=advancedReducer(initRecipeHistory(recipe),{type:"maskResetAdjustmentGroup",id:"m1",group:"light"},context);
 expect(next.present.masks![0].advanced!.development).toEqual({...DEFAULT_DEVELOPMENT,clarity:15});
 expect(next.present.masks![1]).toEqual(recipe.masks[1]);expect(next.present.advanced).toEqual(recipe.advanced);
 expect(advancedReducer(next,{type:"undo"},context).present).toEqual(recipe);
});
it("parametric reset affects only the selected mask and preserves point curves",()=>{
 const recipe=advancedRecipe();recipe.masks=[mask()];const parametric={shadows:30,darks:-15,lights:20,highlights:10,splits:[25,50,75] as [number,number,number]};
 recipe.advanced!.parametricCurve=parametric;recipe.masks[0].advanced!.parametricCurve=parametric;recipe.masks[0].advanced!.curves=[[0,0],[128,140],[255,255]];
 const next=advancedReducer(initRecipeHistory(recipe),{type:"maskResetAdjustmentGroup",id:"m1",group:"parametricCurve"},context);
 expect(next.present.masks![0].advanced!.parametricCurve).toBeUndefined();expect(next.present.masks![0].advanced!.curves).toEqual(recipe.masks[0].advanced!.curves);expect(next.present.advanced!.parametricCurve).toEqual(parametric);
 expect(advancedReducer(next,{type:"undo"},context).present).toEqual(recipe);
});
it("resets color grading in the chosen mask without touching global grading",()=>{
 const recipe=advancedRecipe();recipe.masks=[mask()];
 const grade={shadows:{hue:0,sat:0,lum:0},midtones:{hue:40,sat:30,lum:0},highlights:{hue:0,sat:0,lum:0},global:{hue:0,sat:0,lum:0},blending:50,balance:0};
 recipe.advanced!.colorGrading=grade;recipe.masks[0].advanced!.colorGrading=grade;
 const state=advancedReducer(initRecipeHistory(recipe),{type:"maskResetAdjustmentGroup",id:"m1",group:"colorGrading"},context);
 expect(state.present.masks![0].advanced!.colorGrading).toBeUndefined();expect(state.present.advanced!.colorGrading).toEqual(grade);expect(state.past).toHaveLength(1);
 expect(advancedReducer(state,{type:"undo"},context).present).toEqual(recipe);
});
it("crop session cancels geometry, layers and history without losing previous redo",()=>{
 const recipe=edited(),redo=advancedRecipe();const initial={past:[advancedRecipe()],present:recipe,future:[redo]};
 let state=advancedReducer(initial,{type:"cropStart"},context);
 state=advancedReducer(state,{type:"rotate",delta:1},context);state=advancedReducer(state,{type:"geometry",patch:{angle:15,flipHorizontal:true}},context);
 state=advancedReducer(state,{type:"cropApply",crop:{x:0.2,y:0.3,w:0.5,h:0.4}},context);
 expect(state.past).toEqual(initial.past);expect(state.future).toEqual(initial.future);expect(state.present.rotateQuarter).not.toBe(recipe.rotateQuarter);
 expect(advancedReducer(state,{type:"cropCancel"},context)).toEqual(initial);
});
it("crop session confirms as one undo entry and remains a no-op when unchanged",()=>{
 const recipe=edited(),initial=initRecipeHistory(recipe);
 let state=advancedReducer(initial,{type:"cropStart"},context);state=advancedReducer(state,{type:"rotate",delta:1},context);state=advancedReducer(state,{type:"geometry",patch:{angle:10}},context);
 state=advancedReducer(state,{type:"cropConfirm",crop:{x:0.2,y:0.2,w:0.5,h:0.5}},context);
 expect(state.cropSession).toBeUndefined();expect(state.past).toHaveLength(1);expect(state.present.adjustments).toEqual(recipe.adjustments);
 expect(advancedReducer(state,{type:"undo"},context).present).toEqual(recipe);
 const opened=advancedReducer(initial,{type:"cropStart"},context);expect(advancedReducer(opened,{type:"cropConfirm",crop:recipe.crop},context)).toEqual(initial);
});
function mask():EditLocalMask {return {id:"m1",name:"Local",kind:"brush",enabled:true,inverted:false,density:100,feather:0,from:{x:0.2,y:0.5},to:{x:0.8,y:0.5},strokes:[{points:[{x:0.3,y:0.4}],widthRel:0.1,hardness:1,flow:1,opacity:1,erase:false}],advanced:{exposure:2,temperature:10,tint:0,vibrance:0,curves:[]}};}
it("isolates local reset, duplicates without shared strokes, and restores removal with undo",()=>{
 const global=edited();global.masks=[mask()];let state=initRecipeHistory(global);
 state=advancedReducer(state,{type:"maskDuplicate",id:"m1",newId:"m2",name:"Copy"},context);
 expect(state.present.masks).toHaveLength(2);
 expect(state.present.masks![1].strokes).not.toBe(state.present.masks![0].strokes);
 state=advancedReducer(state,{type:"maskResetAdjustmentGroup",id:"m2",group:"light"},context);
 expect(state.present.masks![1].advanced!.exposure).toBe(0);
 expect(state.present.masks![0].advanced!.exposure).toBe(2);
 expect(state.present.advanced).toEqual(global.advanced);
 const before=state.present;
 state=advancedReducer(state,{type:"maskRemove",id:"m1"},context);
 expect(state.present.masks).toHaveLength(1);
 expect(advancedReducer(state,{type:"undo"},context).present).toEqual(before);
});
it("keeps mask source coordinates unchanged by geometry and supports a single gesture history",()=>{
 const recipe=advancedRecipe();recipe.masks=[mask()];let state=initRecipeHistory(recipe);
 state=advancedReducer(state,{type:"geometry",patch:{angle:30,flipHorizontal:true}},context);
 expect(state.present.masks![0].from).toEqual(recipe.masks[0].from);
 const history=state.past.length;
 state=advancedReducer(state,{type:"maskUpdate",id:"m1",patch:{density:40},record:false},context);
 expect(state.past).toHaveLength(history);expect(state.present.masks![0].density).toBe(40);
 expect(advancedReducer(state,{type:"maskUpdate",id:"missing",patch:{name:"none"}},context)).toBe(state);
});
function edited() {
  const recipe=advancedRecipe();
  recipe.adjustments={brightness:10,contrast:20,saturation:30};
  recipe.advanced={...recipe.advanced!,exposure:2,temperature:15,tint:-5,vibrance:25,
    curves:[[0,0],[128,140],[255,255]],channelCurves:{red:[[0,0],[255,230]]},
    levels:{black:4,white:240,gamma:1.2},hsl:{blues:{hue:10,saturation:20,lightness:30}}};
  recipe.crop={x:0,y:0,w:0.8,h:0.8};
  return recipe;
}
it("resets light atomically, preserves other groups and geometry, and undoes in one step",()=>{
  const original=edited(),state=initRecipeHistory(original);
  const next=advancedReducer(state,{type:"resetAdjustmentGroup",group:"light"},context);
  expect(next.present.adjustments).toEqual({brightness:0,contrast:0,saturation:30});
  expect(next.present.advanced).toEqual({...original.advanced,exposure:0});
  expect(next.present.crop).toEqual(original.crop);expect(next.past).toHaveLength(1);
  expect(advancedReducer(next,{type:"undo"},context).present).toEqual(original);
});
it("resets color without touching light or HSL",()=>{
  const original=edited();const next=advancedReducer(initRecipeHistory(original),{type:"resetAdjustmentGroup",group:"color"},context);
  expect(next.present.adjustments).toEqual({...original.adjustments,saturation:0});
  expect(next.present.advanced).toEqual({...original.advanced,temperature:0,tint:0,vibrance:0});
});
it("clears all curve channels, levels or HSL only within their group",()=>{
  for(const group of ["curves","levels","hsl"] as const) {
    const original=edited();const next=advancedReducer(initRecipeHistory(original),{type:"resetAdjustmentGroup",group},context);
    expect(next.past).toHaveLength(1);expect(next.present.advanced!.exposure).toBe(2);
    if(group==="curves") {expect(next.present.advanced!.curves).toEqual([]);expect(next.present.advanced!.channelCurves).toEqual({});}
    if(group==="levels") expect(next.present.advanced!.levels).toBeUndefined();
    if(group==="hsl") expect(next.present.advanced!.hsl).toEqual({});
  }
});
it("resetting defaults creates no undo step",()=>{
  const state=initRecipeHistory(advancedRecipe());
  for(const group of ["light","color","curves","levels","hsl"] as const)
    expect(advancedReducer(state,{type:"resetAdjustmentGroup",group},context)).toBe(state);
});

it("resets new color groups independently in one undo step",()=>{
 const recipe=advancedRecipe();recipe.advanced={...recipe.advanced!,colorBalance:{shadows:[10,0,0],midtones:[0,0,0],highlights:[0,0,0],preserveLuminosity:true},blackWhite:{enabled:true,weights:[40,60,40,60,20,80],tint:null},selectiveColor:{relative:false,ranges:{reds:[20,0,0,0]}}};
 for(const group of ["colorBalance","blackWhite","selectiveColor"] as const){
  const next=advancedReducer(initRecipeHistory(recipe),{type:"resetAdjustmentGroup",group},context);
  expect(next.present.advanced![group]).toBeUndefined();expect(next.past).toHaveLength(1);
  expect(advancedReducer(next,{type:"undo"},context).present).toEqual(recipe);
  for(const other of ["colorBalance","blackWhite","selectiveColor"] as const)if(other!==group)expect(next.present.advanced![other]).toEqual(recipe.advanced[other]);
 }
});

it("local development reset restores only the selected mask and group",()=>{
 const recipe=advancedRecipe();recipe.masks=[mask()];
 const development={highlights:0,shadows:30,whites:0,blacks:0,texture:0,clarity:20,dehaze:0,sharpenAmount:0,sharpenRadius:1,sharpenDetail:25,sharpenMasking:0,noiseLuminance:0,noiseLuminanceDetail:50,noiseColor:0,noiseColorDetail:50,grainAmount:0,grainSize:25,grainRoughness:50,vignetteAmount:0,vignetteMidpoint:50,vignetteRoundness:0,vignetteFeather:50,vignetteHighlights:0,vignetteStyle:"highlightPriority" as const};
 recipe.advanced!.development=development;recipe.masks[0].advanced!.development={...development};
 const state=advancedReducer(initRecipeHistory(recipe),{type:"maskResetDevelopmentGroup",id:"m1",group:"presence"},context);
 expect(state.present.masks![0].advanced!.development!.clarity).toBe(0);expect(state.present.masks![0].advanced!.development!.shadows).toBe(30);
 expect(state.present.advanced!.development!.clarity).toBe(20);expect(state.past).toHaveLength(1);
 expect(advancedReducer(state,{type:"undo"},context).present).toEqual(recipe);
});

it("category reset includes migrated historical tone and preserves unrelated adjustments",()=>{
 const recipe=advancedRecipe();recipe.legacyAdjustments={brightness:30,contrast:20,saturation:-40};
 let state=advancedReducer(initRecipeHistory(recipe),{type:"resetAdjustmentGroup",group:"light"},context);
 expect(state.present.legacyAdjustments).toEqual({brightness:0,contrast:0,saturation:-40});
 expect(advancedReducer(state,{type:"undo"},context).present).toEqual(recipe);
 state=advancedReducer(state,{type:"resetAdjustmentGroup",group:"color"},context);
 expect(state.present.legacyAdjustments).toEqual({brightness:0,contrast:0,saturation:0});
});
