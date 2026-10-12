import {useEffect,useState} from "react";
import {useTranslation} from "react-i18next";
import type {EditRecipe,EditRecipeCrop} from "@/ipc/api";
import {geometrySize} from "../lib/geometry";
import {pixelCropSize,resizePixelCrop} from "../lib/cropPixels";
import type {RecipeContext} from "../lib/recipe";
export default function CropDimensions({recipe,context,crop,onChange,onFree}:{recipe:EditRecipe;context:RecipeContext;crop:EditRecipeCrop|null;onChange:(crop:EditRecipeCrop)=>void;onFree:()=>void}){
 const {t}=useTranslation();const size=geometrySize(context,recipe),rect=crop??{x:0,y:0,w:1,h:1};const pixels=pixelCropSize(rect,size);
 const [width,setWidth]=useState(String(pixels.width)),[height,setHeight]=useState(String(pixels.height));
 useEffect(()=>{setWidth(String(pixels.width));setHeight(String(pixels.height));},[pixels.width,pixels.height]);
 const valid=context.width>0&&context.height>0&&width.trim()!==""&&height.trim()!==""&&Number.isFinite(Number(width))&&Number.isFinite(Number(height))&&Number(width)>0&&Number(height)>0;
 return <form noValidate className="space-y-2" onSubmit={e=>{e.preventDefault();if(!valid)return;const next=resizePixelCrop(rect,size,Number(width),Number(height));onFree();onChange(next);const pixels=pixelCropSize(next,size);setWidth(String(pixels.width));setHeight(String(pixels.height));}}>
  <h4 className="text-xs font-semibold">{t("editor.crop.pixelSize")}</h4>
  <div className="grid grid-cols-2 gap-2">{([["width",width,setWidth,size.w],["height",height,setHeight,size.h]] as const).map(([key,value,set,max])=><label key={key} className="grid gap-1 text-[11px] text-text-secondary"><span>{t(`editor.crop.${key}`)} · px</span><input type="number" min={1} max={max} step={1} aria-label={t(`editor.crop.${key}`)} value={value} className="w-full rounded-lg border border-edge bg-bg px-2 py-1.5" onChange={e=>set(e.target.value)}/></label>)}</div>
  <button type="submit" disabled={!valid} className="w-full rounded-lg border border-edge py-1.5 text-xs text-text-secondary hover:border-accent disabled:opacity-30">{t("editor.crop.setSize")}</button>
 </form>;
}
