import {useTranslation} from "react-i18next";
import {useId,useRef,useState} from "react";
import type {EditRecipe,EditRetouchStroke} from "@/ipc/api";
import {planeToSource,sourceToPlane,geometrySize} from "../lib/geometry";
import {newLayerId} from "../lib/recipe";
import type {RetouchBrush} from "./RetouchPanel";
import type {Size} from "../lib/coords";
export default function RetouchCanvasControls({recipe,size,width,height,brush,onBrush,onCommit,src}:{recipe:EditRecipe;size:Size;width:number;height:number;brush:RetouchBrush;onBrush:(value:RetouchBrush)=>void;onCommit:(value:EditRetouchStroke)=>void;src:string|null}){
 const {t}=useTranslation();const id=useId().replace(/:/g,"");const [points,setPoints]=useState<{x:number;y:number}[]>([]);const active=useRef<{id:number;points:{x:number;y:number}[];brush:RetouchBrush}|null>(null);
 const crop=recipe.crop??{x:0,y:0,w:1,h:1};const plane=geometrySize(size,recipe);
 const project=(p:{x:number;y:number})=>{const [u,v]=sourceToPlane(p.x,p.y,recipe,size);return [(u-crop.x)/crop.w*width,(v-crop.y)/crop.h*height];};
 const source=brush.source?project(brush.source):null;const first=points[0]?project(points[0]):null;
 const diameter=brush.widthRel*Math.min(size.width,size.height)*width/(crop.w*plane.w);
 function finish(commit:boolean){const gesture=active.current;active.current=null;setPoints([]);if(!commit||!gesture?.points.length)return;onCommit({id:newLayerId(),kind:gesture.brush.kind,source:gesture.brush.source??gesture.points[0],points:gesture.points,widthRel:gesture.brush.widthRel,hardness:gesture.brush.hardness,opacity:gesture.brush.opacity});}
 return <svg role="application" aria-label={t("advancedEditor.retouch.title")} data-mask-gesture={points.length?"active":undefined} tabIndex={0} className="absolute inset-0 z-20 touch-none outline-none" width={width} height={height} style={{cursor:"crosshair"}}
  onPointerDown={event=>{if(event.button!==0)return;const rect=event.currentTarget.getBoundingClientRect();const [x,y]=planeToSource(crop.x+(event.clientX-rect.left)/rect.width*crop.w,crop.y+(event.clientY-rect.top)/rect.height*crop.h,recipe,size);if(x<0||y<0||x>1||y>1)return;event.preventDefault();event.stopPropagation();event.currentTarget.focus();
   if(event.altKey){onBrush({...brush,source:{x,y}});return;}if(brush.kind!=="redEye"&&!brush.source)return;
   active.current={id:event.pointerId,points:[{x,y}],brush:{...brush}};setPoints([{x,y}]);event.currentTarget.setPointerCapture(event.pointerId);}}
  onPointerMove={event=>{const g=active.current;if(!g||g.id!==event.pointerId||g.brush.kind==="redEye")return;const rect=event.currentTarget.getBoundingClientRect();const [u,v]=planeToSource(crop.x+(event.clientX-rect.left)/rect.width*crop.w,crop.y+(event.clientY-rect.top)/rect.height*crop.h,recipe,size);const p={x:Math.max(0,Math.min(1,u)),y:Math.max(0,Math.min(1,v))};const last=g.points[g.points.length-1];if(Math.hypot((p.x-last.x)*size.width,(p.y-last.y)*size.height)<1||g.points.length>=2000)return;g.points.push(p);setPoints([...g.points]);}}
  onPointerUp={()=>finish(true)} onPointerCancel={()=>finish(false)} onBlur={()=>finish(false)} onLostPointerCapture={()=>finish(true)} onKeyDown={event=>{if(active.current&&(event.key==="Escape"||(event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==="z")){event.preventDefault();event.stopPropagation();finish(false);}else if(active.current&&(event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==="s"){event.preventDefault();event.stopPropagation();}}}>
  {source&&<g pointerEvents="none" stroke="white" strokeWidth={1}><circle cx={source[0]} cy={source[1]} r={7} fill="none"/><path d={`M${source[0]-12} ${source[1]}h24 M${source[0]} ${source[1]-12}v24`}/></g>}
  {points.length>0&&<><defs><clipPath id={id}><polyline points={points.map(p=>project(p).join(",")).join(" ")} stroke="white" strokeWidth={diameter} strokeLinecap="round" fill="none"/>{points.length===1&&first&&<circle cx={first[0]} cy={first[1]} r={diameter/2}/>}</clipPath></defs>{source&&first&&src&&<image href={src} x={first[0]-source[0]} y={first[1]-source[1]} width={width} height={height} clipPath={`url(#${id})`} opacity={brush.opacity} pointerEvents="none"/>}<polyline points={points.map(p=>project(p).join(",")).join(" ")} stroke="#fff" strokeWidth={1} fill="none" strokeDasharray="4 3" pointerEvents="none"/></>}
 </svg>;
}
