import {useRef} from "react";
import {useTranslation} from "react-i18next";
import {straightenCorrection} from "../lib/cropPixels";
export default function StraightenOverlay({width,height,onPreview,onCommit,onCancel}:{width:number;height:number;onPreview:(line:[number,number,number,number]|null)=>void;onCommit:(angle:number)=>void;onCancel?:()=>void}){
 const {t}=useTranslation();const active=useRef<{id:number;from:[number,number];to:[number,number]}|null>(null);
 function point(e:React.PointerEvent<SVGSVGElement>):[number,number]{const rect=e.currentTarget.getBoundingClientRect();return [Math.min(1,Math.max(0,(e.clientX-rect.left)/rect.width)),Math.min(1,Math.max(0,(e.clientY-rect.top)/rect.height))];}
 function finish(commit:boolean){const g=active.current;active.current=null;onPreview(null);if(g&&commit){const dx=(g.to[0]-g.from[0])*width,dy=(g.to[1]-g.from[1])*height;if(Math.hypot(dx,dy)>=8)onCommit(straightenCorrection(dx,dy));}}
 return <svg role="application" aria-label={t("editor.geometry.straighten")} tabIndex={0} className="absolute inset-0 z-20 cursor-crosshair touch-none outline-none" width={width} height={height}
  onPointerDown={e=>{if(e.button!==0)return;e.preventDefault();e.currentTarget.focus();e.currentTarget.setPointerCapture(e.pointerId);const from=point(e);active.current={id:e.pointerId,from,to:from};onPreview([...from,...from]);}}
  onPointerMove={e=>{const g=active.current;if(!g||g.id!==e.pointerId)return;g.to=point(e);onPreview([...g.from,...g.to]);}}
  onPointerUp={e=>{if(active.current?.id===e.pointerId)finish(true);}} onPointerCancel={e=>{if(active.current?.id===e.pointerId)finish(false);}} onLostPointerCapture={e=>{if(active.current?.id===e.pointerId)finish(true);}}
  onKeyDown={e=>{if(e.key==="Escape"){e.preventDefault();e.stopPropagation();finish(false);onCancel?.();}}}/>;
}
