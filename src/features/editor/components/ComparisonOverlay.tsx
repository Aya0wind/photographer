import {useRef} from "react";
import {useTranslation} from "react-i18next";
export default function ComparisonOverlay({position,onChange}:{position:number;onChange:(value:number)=>void}){
 const {t}=useTranslation();const pointer=useRef<number|null>(null);
 return <div className="pointer-events-none absolute inset-0 z-20" data-testid="editor-comparison">
  <div className="absolute inset-y-0 w-px bg-white/90" style={{left:`${position*100}%`}}/>
  <div role="slider" tabIndex={0} aria-label={t("advancedEditor.compareDivider")} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(position*100)} aria-orientation="horizontal"
   className="pointer-events-auto absolute inset-y-0 w-7 -translate-x-1/2 cursor-ew-resize touch-none outline-none" style={{left:`${position*100}%`}}
   onPointerDown={e=>{if(e.button!==0)return;e.preventDefault();e.stopPropagation();pointer.current=e.pointerId;e.currentTarget.focus();e.currentTarget.setPointerCapture(e.pointerId);}}
   onPointerMove={e=>{if(pointer.current!==e.pointerId)return;e.stopPropagation();const rect=e.currentTarget.parentElement!.getBoundingClientRect();if(rect.width>0)onChange(Math.max(0,Math.min(1,(e.clientX-rect.left)/rect.width)));}}
   onPointerUp={e=>{e.stopPropagation();pointer.current=null;}} onPointerCancel={()=>{pointer.current=null;}} onLostPointerCapture={()=>{pointer.current=null;}}
   onKeyDown={e=>{if(!["ArrowLeft","ArrowRight","Home","End"].includes(e.key))return;e.preventDefault();e.stopPropagation();onChange(e.key==="Home"?0:e.key==="End"?1:Math.max(0,Math.min(1,position+(e.key==="ArrowLeft"?-1:1)*(e.shiftKey?0.1:0.01))));}}>
   <span data-native-overlay className="absolute left-1/2 top-1/2 flex h-9 w-9 -translate-x-1/2 -translate-y-1/2 items-center justify-center rounded-full border border-white bg-black/70 text-white shadow-lg">↔</span>
  </div>
 </div>;
}
