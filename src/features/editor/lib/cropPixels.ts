import type {EditRecipe,EditRecipeCrop} from "@/ipc/api";
import {geometrySize,remapGeometry} from "./geometry";
import type {RecipeContext} from "./recipe";
export function pixelCropSize(crop:EditRecipeCrop,size:{w:number;h:number}){
 const edge=(v:number)=>Math.abs(v-Math.round(v))<1e-7?Math.round(v):v;
 const x=Math.floor(edge(crop.x*size.w)),y=Math.floor(edge(crop.y*size.h));
 return {width:Math.max(1,Math.ceil(edge((crop.x+crop.w)*size.w))-x),height:Math.max(1,Math.ceil(edge((crop.y+crop.h)*size.h))-y)};
}
/** Integer boundaries keep an entered pixel size exact at original resolution. */
export function resizePixelCrop(crop:EditRecipeCrop,size:{w:number;h:number},width:number,height:number):EditRecipeCrop{
 const w=Math.min(size.w,Math.max(1,Math.round(width))),h=Math.min(size.h,Math.max(1,Math.round(height)));
 const x=Math.min(size.w-w,Math.max(0,Math.round((crop.x+crop.w/2)*size.w-w/2)));
 const y=Math.min(size.h-h,Math.max(0,Math.round((crop.y+crop.h/2)*size.h-h/2)));
 return {x:x/size.w,y:y/size.h,w:w/size.w,h:h/size.h};
}
/** Aligns to the nearest horizontal or vertical axis, independent of drag direction. */
export function straightenCorrection(dx:number,dy:number){const degrees=Math.atan2(dy,dx)*180/Math.PI;return -(((degrees+45)%90+90)%90-45);}
export function remapDraftCrop(previous:EditRecipe,next:EditRecipe,draft:EditRecipeCrop|null,context:RecipeContext,ratio?:number|null){
 const crop=remapGeometry({...previous,crop:draft},next,context).crop;
 if(!crop||!ratio)return crop;
 const plane=geometrySize(context,next),normalized=ratio/(plane.w/plane.h);
 const w=Math.min(crop.w,crop.h*normalized),h=w/normalized;
 return {x:crop.x+(crop.w-w)/2,y:crop.y+(crop.h-h)/2,w,h};
}
