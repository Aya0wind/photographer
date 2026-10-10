import {useRef,useState} from "react";
import {useTranslation} from "react-i18next";
import type {EditLocalMask,EditRecipe} from "@/ipc/api";
import {planeToSource,sourceToPlane,geometrySize} from "../lib/geometry";
import type {Size} from "../lib/coords";

export interface MaskBrush {widthRel:number;hardness:number;opacity:number;flow:number;erase:boolean}
type Point={x:number;y:number};
const last=(points:Point[])=>points[points.length-1];
export default function MaskCanvasControls({mask,recipe,size,width,height,brush,onCommit,overlaySrc}:{
 mask:EditLocalMask;recipe:EditRecipe;size:Size;width:number;height:number;brush:MaskBrush;
 onCommit:(patch:Partial<EditLocalMask>)=>void;
 overlaySrc?:string|null;
}) {
 const {t}=useTranslation();
 const [points,setPoints]=useState<Point[]>([]);
 const active=useRef<{id:number;points:Point[];handle?:"from"|"to"}|null>(null);
 const crop=recipe.crop??{x:0,y:0,w:1,h:1};
 function project(p:Point){const [x,y]=sourceToPlane(p.x,p.y,recipe,size);return [(x-crop.x)/crop.w*width,(y-crop.y)/crop.h*height];}
 function sample(event:React.PointerEvent<SVGSVGElement>):Point|null {
  const rect=event.currentTarget.getBoundingClientRect();
  const [x,y]=planeToSource(crop.x+(event.clientX-rect.left)/rect.width*crop.w,crop.y+(event.clientY-rect.top)/rect.height*crop.h,recipe,size);
  if(!Number.isFinite(x+y)||x<0||y<0||x>1||y>1)return null;
  return {x,y};
 }
 function finish(commit:boolean){
  const gesture=active.current;active.current=null;setPoints([]);
  if(!commit||!gesture?.points.length)return;
  if(mask.kind==="brush")onCommit({strokes:[...mask.strokes,{...brush,points:gesture.points}]});
  else if(gesture.handle)onCommit({[gesture.handle]:last(gesture.points)});
  else if(gesture.points.length>1)onCommit({from:gesture.points[0],to:last(gesture.points)});
 }
 const from=points.length&&active.current?.handle!=="to"?(active.current?.handle==="from"?last(points):points[0]):mask.from;
 const to=points.length&&active.current?.handle!=="from"?last(points):mask.to;
 const p1=project(from),p2=project(to);
 const plane=geometrySize(size,recipe);
 const brushPixels=brush.widthRel*Math.min(size.width,size.height)*width/(crop.w*plane.w);
 return <svg data-testid="editor-mask-controls" role="application" aria-label={t("advancedEditor.masks.editArea")} tabIndex={0} className="absolute inset-0 z-10 touch-none outline-none" width={width} height={height} viewBox={`0 0 ${width} ${height}`} style={{cursor:"crosshair"}}
  onKeyDown={event=>{if(event.key==="Escape"&&active.current){event.preventDefault();event.stopPropagation();finish(false);}}}
  onPointerDown={event=>{
   if(event.button!==0)return;const p=sample(event);if(!p)return;
   event.preventDefault();event.currentTarget.focus();event.currentTarget.setPointerCapture(event.pointerId);
   const handle=(event.target as SVGElement).getAttribute("data-mask-handle") as "from"|"to"|null;
   active.current={id:event.pointerId,points:[p],handle:handle??undefined};setPoints([p]);
  }}
  onPointerMove={event=>{
   const gesture=active.current;if(!gesture||event.pointerId!==gesture.id)return;
   const p=sample(event);if(!p)return;
   const previous=last(gesture.points);
   if(Math.hypot((p.x-previous.x)*size.width,(p.y-previous.y)*size.height)<1)return;
   gesture.points=mask.kind==="brush"?[...gesture.points,p]:[gesture.points[0],p];setPoints([...gesture.points]);
  }} onPointerUp={()=>finish(true)} onPointerCancel={()=>finish(false)} onLostPointerCapture={()=>finish(true)}>
  {overlaySrc&&<image href={overlaySrc} width={width} height={height} opacity={0.35} pointerEvents="none"/>}
  {mask.kind!=="brush" && <g><line x1={p1[0]} y1={p1[1]} x2={p2[0]} y2={p2[1]} stroke="white" strokeWidth={2}/>
   {mask.kind==="radial"&&<ellipse cx={p1[0]} cy={p1[1]} rx={Math.hypot((to.x-from.x)*size.width,(to.y-from.y)*size.height)*width/(crop.w*plane.w)} ry={Math.hypot((to.x-from.x)*size.width,(to.y-from.y)*size.height)*height/(crop.h*plane.h)} fill="none" stroke="#f97373" strokeWidth={1.5}/>}
   <circle data-mask-handle="from" cx={p1[0]} cy={p1[1]} r={7} fill="#202020" stroke="white" strokeWidth={2}/>
   <circle data-mask-handle="to" cx={p2[0]} cy={p2[1]} r={7} fill="#f97373" stroke="white" strokeWidth={2}/></g>}
  {mask.kind==="brush"&&points.length>0&&<polyline points={points.map(p=>project(p).join(",")).join(" ")} fill="none" stroke={brush.erase?"white":"#f97373"} opacity={0.6} strokeWidth={brushPixels} strokeLinecap="round" strokeLinejoin="round" pointerEvents="none"/>}
  {mask.kind==="brush"&&points.length===1&&<circle cx={project(points[0])[0]} cy={project(points[0])[1]} r={brushPixels/2} fill={brush.erase?"white":"#f97373"} opacity={0.6} pointerEvents="none"/>}
 </svg>;
}
