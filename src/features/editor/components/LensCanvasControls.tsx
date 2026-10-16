import {useRef,useState} from "react";
import {useTranslation} from "react-i18next";
import type {EditRecipe} from "@/ipc/api";
import {geometrySize,sourceToPlane,planeToSource} from "../lib/geometry";
import type {LensGuide} from "../lib/lensGuides";
type Point={x:number;y:number};
export default function LensCanvasControls({recipe,size,width,height,axis,lines,grid,onCommit}:{recipe:EditRecipe;size:{width:number;height:number};width:number;height:number;axis:LensGuide["axis"]|null;lines:LensGuide[];grid:boolean;onCommit:(line:LensGuide)=>void}){
  const {t}=useTranslation();const [points,setPoints]=useState<Point[]>([]);const active=useRef<{id:number;axis:LensGuide["axis"];points:Point[]}|null>(null);
  const crop=recipe.crop??{x:0,y:0,w:1,h:1},plane=geometrySize(size,recipe);
  const project=(p:Point)=>{const [x,y]=sourceToPlane(p.x,p.y,recipe,size);return [(x-crop.x)/crop.w*width,(y-crop.y)/crop.h*height];};
  function finish(commit:boolean){const g=active.current;active.current=null;setPoints([]);if(!commit||!g||g.points.length<2)return;const [from,to]=g.points;if(Math.hypot((to.x-from.x)*size.width,(to.y-from.y)*size.height)<10)return;onCommit({axis:g.axis,from,to});}
  function sample(event:React.PointerEvent<SVGSVGElement>){const rect=event.currentTarget.getBoundingClientRect();const [x,y]=planeToSource(crop.x+(event.clientX-rect.left)/rect.width*crop.w,crop.y+(event.clientY-rect.top)/rect.height*crop.h,recipe,size);return {x:Math.max(0,Math.min(1,x)),y:Math.max(0,Math.min(1,y))};}
  const visible=[...lines,...(points.length>1&&axis?[{axis,from:points[0],to:points[1]}]:[])];
  return <svg tabIndex={axis?0:-1} role="application" aria-label={t("advancedEditor.lens.guides")} data-mask-gesture={points.length?"active":undefined} className="absolute inset-0 z-20 touch-none outline-none" width={width} height={height} style={{pointerEvents:axis?"auto":"none",cursor:"crosshair"}}
    onPointerDown={e=>{if(!axis||e.button!==0)return;e.preventDefault();e.stopPropagation();e.currentTarget.focus();const point=sample(e);active.current={id:e.pointerId,axis,points:[point]};setPoints([point]);e.currentTarget.setPointerCapture(e.pointerId);}}
    onPointerMove={e=>{const g=active.current;if(!g||g.id!==e.pointerId)return;g.points=[g.points[0],sample(e)];setPoints(g.points);}}
    onPointerUp={e=>{if(active.current?.id===e.pointerId)finish(true);}} onPointerCancel={()=>finish(false)} onLostPointerCapture={()=>finish(true)} onBlur={()=>finish(false)}
    onKeyDown={e=>{if(active.current&&(e.key==="Escape"||(e.ctrlKey||e.metaKey)&&e.key.toLowerCase()==="z")){e.preventDefault();e.stopPropagation();finish(false);}else if(active.current&&(e.ctrlKey||e.metaKey)&&e.key.toLowerCase()==="s"){e.preventDefault();e.stopPropagation();}}}>
    {grid&&Array.from({length:9},(_,i)=><path key={i} d={`M${width*(i+1)/10} 0V${height} M0 ${height*(i+1)/10}H${width}`} stroke="white" opacity={0.25} strokeWidth={0.5} pointerEvents="none"/>)}
    {visible.map((line,index)=>{const a=project(line.from),b=project(line.to);return <g key={index} stroke={line.axis==="vertical"?"#38bdf8":"#fbbf24"} strokeWidth={2} pointerEvents="none"><line x1={a[0]} y1={a[1]} x2={b[0]} y2={b[1]}/><circle cx={a[0]} cy={a[1]} r={4} fill="none"/><circle cx={b[0]} cy={b[1]} r={4} fill="none"/></g>;})}
    <desc>{plane.w} × {plane.h}</desc>
  </svg>;
}
