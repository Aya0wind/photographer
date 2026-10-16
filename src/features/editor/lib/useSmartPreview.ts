import {useEffect,useRef,type MutableRefObject} from "react";
import {editPreviewPrepare,editPreviewActivate,editPreviewRender,type EditRecipe,type EditorPreviewSession} from "@/ipc/api";
import {preparePreviewImage,releasePreviewImage} from "./previewImages";

export type PreparedFrame = {session:EditorPreviewSession;frame:string;sourceKey?:string};
interface Options {
  session:EditorPreviewSession|null;
  enabled:boolean;
  latest:MutableRefObject<EditRecipe>;
  onPrepared:(value:PreparedFrame)=>void;
  onError:(error:string)=>void;
}

export async function prepareFrame(sessionId:string,latest:MutableRefObject<EditRecipe>,current:()=>boolean,
  beginActivation:()=>boolean,onActivated:(session:EditorPreviewSession)=>void):Promise<PreparedFrame|null> {
  const preparing=latest.current;const sourceKey=JSON.stringify(preparing.colorPipeline?.rawWhiteBalance??null);
  await editPreviewPrepare(sessionId,preparing);
  if(!beginActivation())return null;
  const requested=latest.current;
  const session=await editPreviewActivate(sessionId,requested);
  onActivated(session);
  if(!current())return null;
  let frame=session.previewUrl??session.sourceUrl;
  let used=JSON.stringify(requested);
  try {
    await preparePreviewImage(frame);
    // Activation commits the backend source. Even if film was switched off
    // during IPC, adopt that source and render the current settings before swap.
    while(current()&&used!==JSON.stringify(latest.current)) {
      const recipe=latest.current;used=JSON.stringify(recipe);
      const next=await editPreviewRender(sessionId,recipe);
      releasePreviewImage(frame);frame=next;
      await preparePreviewImage(frame);
    }
    if(!current()){releasePreviewImage(frame);return null;}
    return {frame,session,sourceKey};
  } catch(error) {releasePreviewImage(frame);throw error;}
}

export function useSmartPreview({session,enabled,latest,onPrepared,onError}:Options) {
  const prepared=useRef<string|null>(null),image=useRef<string|null>(null);
  const live=useRef(true),currentSession=useRef(session?.sessionId);
  currentSession.current=session?.sessionId;
  const callbacks=useRef({onPrepared,onError});callbacks.current={onPrepared,onError};
  useEffect(()=>{live.current=true;return()=>{live.current=false;};},[]);
  useEffect(()=>()=>{if(image.current){releasePreviewImage(image.current);image.current=null;}},[session?.sessionId]);
  useEffect(()=>{
    if(!enabled||!session||session.rawlabReady||prepared.current===session.sessionId)return;
    let cancelled=false,completed=false,activating=false;
    let activated:EditorPreviewSession|null=null;
    const current=()=>live.current&&currentSession.current===session.sessionId;
    const beginActivation=()=>{if(cancelled)return false;activating=true;return true;};
    const requestedSourceKey=JSON.stringify(latest.current.colorPipeline?.rawWhiteBalance??null);
    prepared.current=session.sessionId;
    void prepareFrame(session.sessionId,latest,current,beginActivation,value=>{activated=value;}).then(result=>{
      if(!result)return;
      if(!current()){releasePreviewImage(result.frame);return;}
      completed=true;
      if(image.current&&image.current!==result.frame)releasePreviewImage(image.current);
      image.current=result.frame;
      callbacks.current.onPrepared(result);
    }).catch(error=>{
      if(!current()||(cancelled&&!activating))return;
      prepared.current=null;
      // Keep frontend/backend source ownership aligned after a render error.
      if(activated)callbacks.current.onPrepared({frame:image.current??activated.sourceUrl,session:activated,sourceKey:requestedSourceKey});
      callbacks.current.onError(String(error));
    });
    return ()=>{cancelled=true;if(!completed&&!activating&&prepared.current===session.sessionId)prepared.current=null;};
  },[session?.sessionId,enabled]);
}
