import {useEffect,useLayoutEffect,useRef,type RefObject} from "react";

interface Layout {zoom:number;width:number;height:number;left:number;top:number;viewportWidth:number;viewportHeight:number;scrollLeft:number;scrollTop:number}

/** Keep the same image point under the viewport center before paint. Auto
 * margins disappear when an image overflows, so multiplying scroll alone jumps. */
export function useCanvasZoom(host:RefObject<HTMLElement|null>,zoom:number){
  const previousLayout=useRef<Layout|null>(null);
  useEffect(()=>{
    const view=host.current;if(!view)return;
    const remember=()=>{const previous=previousLayout.current;if(previous){previous.scrollLeft=view.scrollLeft;previous.scrollTop=view.scrollTop;}};
    // Remember before the DOM shrinks: browsers clamp scroll offsets during
    // zoom-out, before layout effects can read the old position.
    view.addEventListener("scroll",remember);
    view.addEventListener("wheel",remember,{capture:true,passive:true});
    return()=>{view.removeEventListener("scroll",remember);view.removeEventListener("wheel",remember,true);};
  },[host]);
  useLayoutEffect(()=>{
    const view=host.current;
    const frame=view?.querySelector<HTMLElement>('[data-testid="editor-canvas-frame"]');
    if(!view||!frame){previousLayout.current=null;return;}
    const previous=previousLayout.current;
    const picture=frame.getBoundingClientRect(),viewport=view.getBoundingClientRect();
    const next={zoom,width:picture.width||frame.offsetWidth,height:picture.height||frame.offsetHeight,left:picture.width?picture.left-viewport.left+view.scrollLeft-view.clientLeft:frame.offsetLeft,top:picture.height?picture.top-viewport.top+view.scrollTop-view.clientTop:frame.offsetTop,viewportWidth:view.clientWidth,viewportHeight:view.clientHeight,scrollLeft:view.scrollLeft,scrollTop:view.scrollTop};
    if(previous&&(previous.zoom!==zoom||zoom>1&&(previous.viewportWidth!==next.viewportWidth||previous.viewportHeight!==next.viewportHeight))&&previous.width>0&&previous.height>0){
      const x=(previous.scrollLeft+previous.viewportWidth/2-previous.left)/previous.width;
      const y=(previous.scrollTop+previous.viewportHeight/2-previous.top)/previous.height;
      view.scrollLeft=next.left+x*next.width-next.viewportWidth/2;
      view.scrollTop=next.top+y*next.height-next.viewportHeight/2;
      next.scrollLeft=view.scrollLeft;next.scrollTop=view.scrollTop;
    }
    previousLayout.current=next;
  });
}
