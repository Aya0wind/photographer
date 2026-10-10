import {expect,it} from "vitest";
import {advancedRecipe,advancedReducer} from "./advancedRecipe";
import {initRecipeHistory} from "./recipe";
import type {EditLocalMask} from "@/ipc/api";
const context={width:6000,height:4000};
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

it("category reset includes migrated historical tone and preserves unrelated adjustments",()=>{
 const recipe=advancedRecipe();recipe.legacyAdjustments={brightness:30,contrast:20,saturation:-40};
 let state=advancedReducer(initRecipeHistory(recipe),{type:"resetAdjustmentGroup",group:"light"},context);
 expect(state.present.legacyAdjustments).toEqual({brightness:0,contrast:0,saturation:-40});
 expect(advancedReducer(state,{type:"undo"},context).present).toEqual(recipe);
 state=advancedReducer(state,{type:"resetAdjustmentGroup",group:"color"},context);
 expect(state.present.legacyAdjustments).toEqual({brightness:0,contrast:0,saturation:0});
});
