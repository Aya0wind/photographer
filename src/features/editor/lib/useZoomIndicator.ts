import {useCallback,useEffect,useRef,useState} from "react";

/** Only zoom activity wakes the HUD; panning/hover never prolongs it. */
export function useZoomIndicator(zoom:number) {
  const [visible,setVisible]=useState(true);
  const timer=useRef<ReturnType<typeof setTimeout>|null>(null);
  const wake=useCallback(()=>{
    setVisible(true);if(timer.current)clearTimeout(timer.current);
    timer.current=setTimeout(()=>setVisible(false),3000);
  },[]);
  useEffect(()=>{wake();return()=>{if(timer.current)clearTimeout(timer.current);};},[zoom,wake]);
  return {visible,wake};
}
