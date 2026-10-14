import {useEffect,useRef,type MutableRefObject} from "react";
import {editPreviewPrepare,editPreviewActivate,editPreviewRender,type EditRecipe,type EditorPreviewSession} from "@/ipc/api";
import {preparePreviewImage,releasePreviewImage} from "./previewImages";
export function useSmartPreview({session,enabled,latest,onPrepared,onError}:{session:EditorPreviewSession|null;enabled:boolean;latest:MutableRefObject<EditRecipe>;onPrepared:(value:{session:EditorPreviewSession;frame:string})=>void;onError:(error:string)=>void}){
 const prepared=useRef<string|null>(null),image=useRef<string|null>(null);
 const callbacks=useRef({onPrepared,onError});callbacks.current={onPrepared,onError};
  useEffect(()=>{
    if(!enabled||!session||session.rawlabReady||prepared.current===session.sessionId)return;
    let cancelled=false;
    let completed=false;
    let candidate:string|null=null;
    prepared.current=session.sessionId;
    const captured=latest.current;
    void editPreviewPrepare(session.sessionId,captured).then(async ()=>{
      if(cancelled)return;
      const requested=latest.current;
      const value=await editPreviewActivate(session.sessionId,requested);
      if(cancelled)return;
      let frame=value.previewUrl??value.sourceUrl;candidate=frame;
      let used=JSON.stringify(requested);
      // Keep the old frame visible until the prepared source has a frame for
      // the current recipe, including edits made during RAW development.
      await preparePreviewImage(frame);
      while(!cancelled && used!==JSON.stringify(latest.current)){
        const current=latest.current;used=JSON.stringify(current);
        const next=await editPreviewRender(session.sessionId,current);
        releasePreviewImage(frame);frame=next;candidate=frame;await preparePreviewImage(frame);
      }
      if(cancelled){releasePreviewImage(frame);return;}
      completed=true;image.current=frame;candidate=null;callbacks.current.onPrepared({frame,session:value});
    }).catch(e=>{if(candidate)releasePreviewImage(candidate);if(!cancelled)callbacks.current.onError(String(e));});
    return ()=>{cancelled=true;if(!completed&&prepared.current===session.sessionId)prepared.current=null;if(image.current){releasePreviewImage(image.current);image.current=null;}};
  },[session?.sessionId,enabled]);

}
