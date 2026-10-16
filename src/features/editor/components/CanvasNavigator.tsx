import {useEffect,useRef,useState,type RefObject} from "react";
import {useTranslation} from "react-i18next";

/** Viewport geometry only: no second compositor or full-resolution image decode. */
export default function CanvasNavigator({host}:{host:RefObject<HTMLElement|null>}) {
  const {t}=useTranslation();
  const [box,setBox]=useState({x:0,y:0,w:1,h:1,ratio:1.5});
  const pointer=useRef<number|null>(null);
  useEffect(()=>{
    const root=host.current;if(!root)return;
    let frame=0;
    const refresh=()=>{
      frame=0;
      const viewport=root.querySelector<HTMLElement>('[data-testid="editor-canvas-viewport"]');
      const image=root.querySelector<HTMLElement>('[data-testid="editor-canvas-frame"]');
      if(!viewport||!image)return;
      const v=viewport.getBoundingClientRect(),p=image.getBoundingClientRect();
      if(p.width<=0||p.height<=0)return;
      const next={x:Math.max(0,(v.left-p.left)/p.width),y:Math.max(0,(v.top-p.top)/p.height),
        w:Math.min(1,v.width/p.width),h:Math.min(1,v.height/p.height),ratio:p.width/p.height};
      setBox(before=>JSON.stringify(before)===JSON.stringify(next)?before:next);
    };
    const schedule=()=>{if(!frame)frame=requestAnimationFrame(refresh);};
    const observer=new ResizeObserver(schedule);observer.observe(root);
    const mutations=new MutationObserver(()=>{
      const image=root.querySelector('[data-testid="editor-canvas-frame"]');if(image)observer.observe(image);schedule();
    });
    mutations.observe(root,{childList:true,subtree:true,attributes:true,attributeFilter:["style"]});
    root.addEventListener("scroll",schedule,true);schedule();
    return()=>{observer.disconnect();mutations.disconnect();root.removeEventListener("scroll",schedule,true);cancelAnimationFrame(frame);};
  },[host]);
  function navigate(x:number,y:number,element:HTMLElement){
    const viewport=host.current?.querySelector<HTMLElement>('[data-testid="editor-canvas-viewport"]');
    const image=host.current?.querySelector<HTMLElement>('[data-testid="editor-canvas-frame"]');
    if(!viewport||!image)return;
    const v=viewport.getBoundingClientRect(),p=image.getBoundingClientRect(),map=element.getBoundingClientRect();
    viewport.scrollLeft+=(x-map.left)/map.width*p.width-(v.left-p.left)-v.width/2;
    viewport.scrollTop+=(y-map.top)/map.height*p.height-(v.top-p.top)-v.height/2;
  }
  if(box.w>=1&&box.h>=1)return null;
  return <div data-native-overlay className="absolute bottom-4 right-3 z-40 w-32 rounded-lg border border-white/15 bg-[#202020] p-2 text-white/75">
    <span className="mb-1 block text-[10px]">{t("advancedEditor.navigator.title")}</span>
    <div role="group" aria-label={t("advancedEditor.navigator.title")} className="relative cursor-crosshair touch-none overflow-hidden rounded border border-white/20 bg-white/5"
      style={{aspectRatio:Math.max(0.5,Math.min(3,box.ratio))}}
      onPointerDown={event=>{if(event.button!==0)return;event.preventDefault();event.stopPropagation();pointer.current=event.pointerId;event.currentTarget.setPointerCapture(event.pointerId);navigate(event.clientX,event.clientY,event.currentTarget);}}
      onPointerMove={event=>{if(pointer.current===event.pointerId)navigate(event.clientX,event.clientY,event.currentTarget);}}
      onPointerUp={()=>{pointer.current=null;}} onPointerCancel={()=>{pointer.current=null;}} onLostPointerCapture={()=>{pointer.current=null;}}>
      <span className="pointer-events-none absolute border border-accent bg-accent/15" style={{left:`${box.x*100}%`,top:`${box.y*100}%`,width:`${box.w*100}%`,height:`${box.h*100}%`}}/>
    </div>
    <div className="mt-1 flex justify-between">{[[-1,0,"←"],[0,-1,"↑"],[0,1,"↓"],[1,0,"→"]].map(([x,y,label])=><button key={label} type="button" className="h-5 w-6 rounded hover:bg-white/10" aria-label={t("advancedEditor.navigator.move",{direction:label})} onClick={()=>{const view=host.current?.querySelector<HTMLElement>('[data-testid="editor-canvas-viewport"]');if(view)view.scrollBy(Number(x)*view.clientWidth*0.25,Number(y)*view.clientHeight*0.25);}}>{label}</button>)}</div>
  </div>;
}
