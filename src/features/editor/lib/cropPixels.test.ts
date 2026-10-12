import {expect,it} from "vitest";
import {pixelCropSize,resizePixelCrop,straightenCorrection,remapDraftCrop} from "./cropPixels";
import {changeGeometry,geometrySize} from "./geometry";
import {advancedRecipe,advancedReducer} from "./advancedRecipe";
import {initRecipeHistory} from "./recipe";
it("sets integer dimensions around the current center without leaving the image",()=>{
 const size={w:6000,h:4000};const crop=resizePixelCrop({x:0.1,y:0.2,w:0.8,h:0.6},size,500,333);
 expect(pixelCropSize(crop,size)).toEqual({width:500,height:333});expect(crop.x+crop.w).toBeLessThanOrEqual(1);expect(crop.y+crop.h).toBeLessThanOrEqual(1);
 const edge=resizePixelCrop({x:0.9,y:0.9,w:0.1,h:0.1},size,9000,9000);expect(edge).toEqual({x:0,y:0,w:1,h:1});
});
it("retains a small pixel crop through the PhotoCraft recipe reducer",()=>{
 const recipe=advancedRecipe(),context={width:6000,height:4000};const crop=resizePixelCrop({x:0,y:0,w:1,h:1},{w:6000,h:4000},20,10);
 const state=advancedReducer(initRecipeHistory(recipe),{type:"cropApply",crop},context);
 expect(pixelCropSize(state.present.crop!,{w:6000,h:4000})).toEqual({width:20,height:10});expect(state.past).toHaveLength(1);
 expect(advancedReducer(state,{type:"undo"},context).present).toEqual(recipe);
});
it("straightens reversed horizontal and vertical lines consistently",()=>{expect(straightenCorrection(100,10)).toBeCloseTo(-5.7106,3);expect(straightenCorrection(-100,-10)).toBeCloseTo(-5.7106,3);expect(straightenCorrection(10,100)).toBeCloseTo(5.7106,3);});
it("keeps the active draft instead of the saved crop when geometry changes and respects its ratio",()=>{const recipe=advancedRecipe(),context={width:6000,height:4000};recipe.crop={x:0,y:0,w:0.8,h:0.8};const draft={x:0.3,y:0.3,w:0.2,h:0.2};const next=changeGeometry(recipe,{angle:10},context);const remapped=remapDraftCrop(recipe,next,draft,context,1.5)!;const size=geometrySize(context,next);expect(remapped.w).toBeLessThan(0.3);expect(remapped.w*size.w/(remapped.h*size.h)).toBeCloseTo(1.5,6);});
