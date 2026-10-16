import {useLayoutEffect,useRef} from "react";
export interface WheelValue{hue:number;sat:number;lum:number}
export default function ColorWheel({label,value,onChange,begin,end,cancel,compact=false}:{label:string;value:WheelValue;onChange:(value:WheelValue,record?:boolean)=>void;begin:()=>void;end:()=>void;cancel?:()=>void;compact?:boolean}){
 const active=useRef<{id:number;initial:WheelValue;last:WheelValue}|null>(null);
 // Flush external-value reconciliation before another movement updates last.
 useLayoutEffect(()=>{const last=active.current?.last;if(last&&(last.hue!==value.hue||last.sat!==value.sat||last.lum!==value.lum))active.current=null;},[value]);
 function move(e:React.PointerEvent<HTMLDivElement>){const g=active.current;if(!g)return;const rect=e.currentTarget.getBoundingClientRect();const radius=Math.min(rect.width,rect.height)/2;const x=e.clientX-rect.left-rect.width/2,y=e.clientY-rect.top-rect.height/2;
  let hue=Math.hypot(x,y)<0.01?g.initial.hue:(Math.atan2(y,x)*180/Math.PI+360)%360,sat=Math.min(100,Math.hypot(x,y)/radius*100);
  if(e.shiftKey)hue=g.initial.hue;
  if(e.altKey){const delta=((hue-g.initial.hue+180)%360+360)%360-180;hue=(g.initial.hue+delta*0.1+360)%360;sat=g.initial.sat+(sat-g.initial.sat)*0.1;}
  if(hue===value.hue&&sat===value.sat)return;
  const next={...value,hue,sat};g.last=next;onChange(next,false);
 }
 function finish(){if(active.current){active.current=null;end();}}
 const angle=value.hue*Math.PI/180,r=value.sat/100*50;
 return <div role="application" aria-label={label} tabIndex={0} className={`relative mx-auto aspect-square ${compact?"w-20":"w-44"} max-w-full cursor-crosshair touch-none rounded-full border border-edge outline-none focus:ring-2 focus:ring-accent`}
  style={{background:"radial-gradient(circle,white 0%,transparent 70%),conic-gradient(from 90deg,#f00,#ff0,#0f0,#0ff,#00f,#f0f,#f00)"}}
  onPointerDown={e=>{if(e.button!==0)return;e.preventDefault();e.currentTarget.focus();e.currentTarget.setPointerCapture(e.pointerId);begin();active.current={id:e.pointerId,initial:value,last:value};move(e);}}
  onPointerMove={e=>{if(active.current?.id===e.pointerId)move(e);}}
  onPointerUp={finish} onLostPointerCapture={finish} onPointerCancel={()=>{if(active.current){active.current=null;cancel?.();}}}
  onDoubleClick={()=>{finish();if(value.hue||value.sat)onChange({hue:0,sat:0,lum:value.lum});}}
  onKeyDown={e=>{
   if(e.key==="Escape"&&active.current){e.preventDefault();e.stopPropagation();active.current=null;cancel?.();return;}
   if(["ArrowLeft","ArrowRight","ArrowUp","ArrowDown"].includes(e.key)){e.preventDefault();e.stopPropagation();const step=e.altKey?0.1:e.shiftKey?10:1;onChange({...value,hue:(value.hue+(e.key==="ArrowRight"?step:e.key==="ArrowLeft"?-step:0)+360)%360,sat:Math.max(0,Math.min(100,value.sat+(e.key==="ArrowUp"?step:e.key==="ArrowDown"?-step:0)))});}
  }}>
  <span className="pointer-events-none absolute h-3 w-3 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-white shadow-[0_0_0_1px_black]" style={{left:`${50+Math.cos(angle)*r}%`,top:`${50+Math.sin(angle)*r}%`,background:`hsl(${value.hue} ${value.sat}% 50%)`}}/>
 </div>;
}
