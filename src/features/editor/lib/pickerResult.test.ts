import {it,expect} from "vitest";
import {pickerAdjustment} from "./pickerResult";
import {advancedRecipe,advancedReducer} from "./advancedRecipe";
import {initRecipeHistory} from "./recipe";
it.each(["black","gray","white"])("%s applies its returned correction even when the response also has a readout",picker=>{
 const recipe=advancedRecipe();recipe.advanced!.temperature=10;recipe.advanced!.channelCurves={luminance:[[0,0],[255,230]]};
 const patch=pickerAdjustment(picker,{sample:[64,128,192],red:[[0,0],[64,80],[255,255]],points:[[0,0],[255,255]]},recipe.advanced)!;
 const next=advancedReducer(initRecipeHistory(recipe),{type:"advanced",patch},{width:600,height:400});
 expect(next.present.advanced!.channelCurves!.red).toEqual([[0,0],[64,80],[255,255]]);expect(next.present.advanced!.temperature).toBe(10);expect(next.present.advanced!.channelCurves!.luminance).toEqual(recipe.advanced!.channelCurves!.luminance);expect(next.past).toHaveLength(1);
 expect(advancedReducer(next,{type:"undo"},{width:600,height:400}).present).toEqual(recipe);
});
it("point and HSL readouts do not modify curves",()=>{
 for(const picker of ["point","hsl"])expect(pickerAdjustment(picker,{sample:[64,128,192]},advancedRecipe().advanced)).toBeNull();
});
