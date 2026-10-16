import {useTranslation} from "react-i18next";
import {rawSourceKey} from "./rawSourceKey";
import { useEffect, useMemo, useRef, useState } from "react";
import { editPreviewSourceAvailable,editPreviewClose, editPreviewOpen, type EditRecipe, type EditorPreviewSession } from "@/ipc/api";

import {releasePreviewImage} from "./previewImages";
import {useRawSourcePreview} from "./useRawSourcePreview";
import {useSmartPreview} from "./useSmartPreview";
import { startPreviewScheduler } from "./previewScheduler";

/** Edit a stable cached proxy; original-file development is not part of opening the editor. */
export function useEditorPreview(assetId: number, recipe: EditRecipe, libraryId: string, nativeCanvas = false) {
  const {t}=useTranslation();
  const [retry, setRetry] = useState(0);
  const [session, setSession] = useState<EditorPreviewSession | null>(null);
  const [quickUrl,setQuickUrl]=useState<string|null>(null);
  const [url, setUrl] = useState<string | null>(null);
  useEffect(()=>()=>{if(url)releasePreviewImage(url);},[url]);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const latest = useRef(recipe);
  latest.current = recipe;
  const signature = useMemo(() => JSON.stringify({ ...recipe, output: undefined }), [recipe]);
  const openedRequest = useRef<Promise<EditorPreviewSession> | null>(null);
  const scheduler = useRef<(() => void) | null>(null);
  const closeScheduler=useRef<(()=>void)|null>(null);
  const enabled = recipe.renderer === "photocraft";
  const rawlab = !!recipe.colorPipeline;
  const [offline,setOffline]=useState(false);
  const [sourceKey,setSourceKey]=useState<string|null>(null);
  const [prepareError,setPrepareError]=useState<string|null>(null);

  useEffect(() => {
    if (!enabled) { setSession(null); setUrl(null); return; }
    let cancelled = false;
    let id: string | null = null;
    setSession(null); setUrl(null); setError(null);setPrepareError(null);setSourceKey(null);setQuickUrl(null);
    // Pipeline toggles change the recipe/source in this session. Reopening it
    // would discard the displayed frame and rebuild the native child viewport.
    const opening = editPreviewOpen(assetId, libraryId,!!latest.current.colorPipeline);
    openedRequest.current = opening;
    void opening.then((value) => {
      id = value.sessionId;
      if (cancelled) void editPreviewClose(id).catch(() => {});
      else {
        setQuickUrl(value.bitDepth==="8 bit preview"?value.sourceUrl:null);
        setOffline(value.sourceAvailable===false);setSession(value);if(value.rawlabReady)setSourceKey("[null,null]");

      }
    }).catch((e: unknown) => { if (!cancelled) setError(String(e)); });
    return () => { cancelled = true; if (openedRequest.current === opening) openedRequest.current = null; if (id) void editPreviewClose(id).catch(() => {});const active=openedRequest.current;if(active&&active!==opening)void active.then(value=>editPreviewClose(value.sessionId)).catch(()=>{}); };
  }, [assetId, libraryId, retry, enabled]);

  useSmartPreview({session,enabled:rawlab&&!offline,latest,onPrepared:value=>{closeScheduler.current?.();setSourceKey(value.sourceKey??rawSourceKey(latest.current));setUrl(value.frame);setSession(value.session);},onError:setPrepareError});

  const balance=useRawSourcePreview({assetId,libraryId,session:offline?null:session,sourceKey,latest,onPrepared:value=>{
    const previous=session?.sessionId;closeScheduler.current?.();openedRequest.current=Promise.resolve(value.session);
    setSourceKey(value.sourceKey??null);setUrl(value.frame);setSession(value.session);
    if(previous)void editPreviewClose(previous).catch(()=>{});
  }});

  useSourceAvailability(session,setOffline,()=>setRetry(n=>n+1));
  useEffect(() => {
    if (!session || nativeCanvas) { setPending(false); return; }
    const schedulerHandle = startPreviewScheduler(session.sessionId, () => latest.current, { setUrl, setError, setPending },session.sourceUrl);
    scheduler.current = schedulerHandle.wake;closeScheduler.current=schedulerHandle.close;
    return () => { scheduler.current = null; closeScheduler.current=null;schedulerHandle.close(); };
  }, [session, nativeCanvas]);
  useEffect(() => { scheduler.current?.(); }, [signature]);
  return { enabled, session, quickUrl,url: url ?? session?.sourceUrl ?? null, pending,
    loading: enabled && !session && !error, error, warning:offline?t("advancedEditor.offline"):balance.error??prepareError,offline,preparingWhiteBalance:balance.busy, close: async () => {
      const request = openedRequest.current;
      if (request) { try { const value = await request; await editPreviewClose(value.sessionId); } catch (error) { console.warn("Could not release editor preview session", error); } }
    }, reload: () => setRetry((n) => n + 1) };
}

function useSourceAvailability(session:EditorPreviewSession|null,setOffline:(value:boolean)=>void,onRecovered:()=>void){
  useEffect(()=>{
    if(!session)return;let live=true;let timer:ReturnType<typeof setTimeout>;
    const wasOffline={value:session.sourceAvailable===false};
    async function check(){try{const available=await editPreviewSourceAvailable(session!.sessionId);if(!live)return;setOffline(!available);if(available&&wasOffline.value){onRecovered();return;}wasOffline.value=!available;}catch{/* Next metadata poll can recover a transient error. */}if(live)timer=setTimeout(()=>void check(),5000);}
    timer=setTimeout(()=>void check(),5000);return()=>{live=false;clearTimeout(timer);};
  },[session?.sessionId]);
}
