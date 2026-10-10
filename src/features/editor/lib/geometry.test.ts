import {expect,it} from "vitest";
import {geometrySize,planeToSource,sourceToPlane,changeGeometry} from "./geometry";
import {defaultRecipe,initRecipeHistory,recipeReducer} from "./recipe";
const context={width:400,height:300};
it("matches PhotoCraft's expanded rotation bounds and round-trips every orientation",()=>{
 for(const quarter of [0,1,2,3] as const) for(const flip of [false,true]) {
  const recipe={...defaultRecipe(),rotateQuarter:quarter,geometry:{angle:25,flipHorizontal:flip,flipVertical:!flip}};
  const size=geometrySize(context,recipe);expect(size.w).toBeGreaterThan(300);
  const point=sourceToPlane(0.2,0.7,recipe,context),back=planeToSource(...point,recipe,context);
  expect(back[0]).toBeCloseTo(0.2,10);expect(back[1]).toBeCloseTo(0.7,10);
 }
});
it("horizontal flip moves crop and annotation anchors while preserving scale",()=>{
 const recipe={...defaultRecipe(),crop:{x:0.1,y:0.2,w:0.5,h:0.6},textLayers:[{id:"t",x:0.2,y:0.4,text:"caption",sizeRel:0.1,color:"#ffffff"}],brushStrokes:[{id:"s",color:"#ff0000",widthRel:0.02,points:[{x:0.3,y:0.6}]}]};
 const next=changeGeometry(recipe,{flipHorizontal:true},context);
 expect(next.crop!.x).toBeCloseTo(0.4);expect(next.textLayers[0].x).toBeCloseTo(0.8);
 expect(next.brushStrokes[0].points[0].x).toBeCloseTo(0.7);expect(next.textLayers[0].sizeRel).toBeCloseTo(0.1);
});
it("angle changes and reset remain single undoable operations",()=>{
 const original={...defaultRecipe(),renderer:"photocraft" as const};
 const changed=recipeReducer(initRecipeHistory(original),{type:"geometry",patch:{angle:30}},context);
 expect(changed.past).toHaveLength(1);expect(changed.present.geometry!.angle).toBe(30);
 const reset=recipeReducer(changed,{type:"geometryReset"},context);
 expect(reset.present.geometry).toBeUndefined();expect(reset.present.crop).toBeNull();
 expect(recipeReducer(reset,{type:"undo"},context).present).toEqual(changed.present);
});

it("continuous angle updates use the gesture baseline rather than inflating the crop each step",()=>{
 const original={...defaultRecipe(),crop:{x:0.2,y:0.2,w:0.5,h:0.5}};
 let state=initRecipeHistory(original);
 for(const angle of [1,2,3,4,5]) state=recipeReducer(state,{type:"geometry",patch:{angle},record:false,baseline:original},context);
 const direct=changeGeometry(original,{angle:5},context);
 expect(state.present.crop).toEqual(direct.crop);expect(state.past).toHaveLength(0);
});
