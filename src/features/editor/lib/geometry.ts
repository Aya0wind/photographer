import type {EditRecipe,EditRecipeCrop} from "@/ipc/api";
import type {RecipeContext} from "./recipe";
import {clamp01} from "./coords";
export const DEFAULT_GEOMETRY={angle:0,flipHorizontal:false,flipVertical:false};
function quarterSize(context:RecipeContext,quarter:number) {
 const swapped=quarter%2!==0;return {w:Math.max(1,swapped?context.height:context.width),h:Math.max(1,swapped?context.width:context.height)};
}
export function geometrySize(context:RecipeContext,recipe:EditRecipe) {
 const {w,h}=quarterSize(context,recipe.rotateQuarter),angle=(recipe.geometry?.angle??0)*Math.PI/180;
 return {w:Math.max(1,Math.ceil(w*Math.abs(Math.cos(angle))+h*Math.abs(Math.sin(angle))-1e-6)),h:Math.max(1,Math.ceil(w*Math.abs(Math.sin(angle))+h*Math.abs(Math.cos(angle))-1e-6))};
}
export function sourceToPlane(x:number,y:number,recipe:EditRecipe,context:RecipeContext):[number,number] {
 let u=x,v=y;
 switch(recipe.rotateQuarter){case 1:u=1-y;v=x;break;case 2:u=1-x;v=1-y;break;case 3:u=y;v=1-x;break;}
 const g={...DEFAULT_GEOMETRY,...recipe.geometry};if(g.flipHorizontal)u=1-u;if(g.flipVertical)v=1-v;
 const base=quarterSize(context,recipe.rotateQuarter),size=geometrySize(context,recipe),a=g.angle*Math.PI/180;
 const px=(u-0.5)*base.w,py=(v-0.5)*base.h;
 return [(Math.cos(a)*px-Math.sin(a)*py)/size.w+0.5,(Math.sin(a)*px+Math.cos(a)*py)/size.h+0.5];
}
export function planeToSource(u:number,v:number,recipe:EditRecipe,context:RecipeContext):[number,number] {
 const g={...DEFAULT_GEOMETRY,...recipe.geometry},base=quarterSize(context,recipe.rotateQuarter),size=geometrySize(context,recipe),a=g.angle*Math.PI/180;
 const px=(u-0.5)*size.w,py=(v-0.5)*size.h;
 let x=(Math.cos(a)*px+Math.sin(a)*py)/base.w+0.5,y=(-Math.sin(a)*px+Math.cos(a)*py)/base.h+0.5;
 if(g.flipHorizontal)x=1-x;if(g.flipVertical)y=1-y;
 switch(recipe.rotateQuarter){case 1:return [y,1-x];case 2:return [1-x,1-y];case 3:return [1-y,x];default:return [x,y];}
}
const FULL={x:0,y:0,w:1,h:1};
export function remapGeometry(previous:EditRecipe,next:EditRecipe,context:RecipeContext,resetCrop=false):EditRecipe {
 const oldCrop=previous.crop??FULL;
 const map=(u:number,v:number)=>{const raw=planeToSource(u,v,previous,context);return sourceToPlane(...raw,next,context);};
 let crop:EditRecipeCrop|null=null;
 if(previous.crop && !resetCrop) {
  const points=[[oldCrop.x,oldCrop.y],[oldCrop.x+oldCrop.w,oldCrop.y],[oldCrop.x,oldCrop.y+oldCrop.h],[oldCrop.x+oldCrop.w,oldCrop.y+oldCrop.h]].map(([u,v])=>map(u,v));
  const x=Math.min(0.999,clamp01(Math.min(...points.map(p=>p[0])))),y=Math.min(0.999,clamp01(Math.min(...points.map(p=>p[1]))));
  crop={x,y,w:Math.max(0.001,clamp01(Math.max(...points.map(p=>p[0])))-x),h:Math.max(0.001,clamp01(Math.max(...points.map(p=>p[1])))-y)};
 }
 const newCrop=crop??FULL;
 const before=geometrySize(context,previous),after=geometrySize(context,next);
 const scale=oldCrop.w*before.w/(newCrop.w*after.w);
 const point=(x:number,y:number)=>{const [u,v]=map(oldCrop.x+x*oldCrop.w,oldCrop.y+y*oldCrop.h);return {x:clamp01((u-newCrop.x)/newCrop.w),y:clamp01((v-newCrop.y)/newCrop.h)};};
 return {...next,crop,textLayers:previous.textLayers.map(layer=>({...layer,...point(layer.x,layer.y),sizeRel:layer.sizeRel*scale})),brushStrokes:previous.brushStrokes.map(stroke=>({...stroke,widthRel:stroke.widthRel*scale,points:stroke.points.map(p=>point(p.x,p.y))}))};
}
export function changeGeometry(recipe:EditRecipe,patch:Partial<NonNullable<EditRecipe["geometry"]>>,context:RecipeContext) {
 return remapGeometry(recipe,{...recipe,geometry:{...DEFAULT_GEOMETRY,...recipe.geometry,...patch}},context);
}
