import { useEffect, useMemo, useRef, useState } from "react";
import { editPreviewClose, editPreviewOpen, type EditRecipe, type EditorPreviewSession } from "@/ipc/api";

import {useSmartPreview} from "./useSmartPreview";
import { startPreviewScheduler } from "./previewScheduler";

/** Edit a stable cached proxy; original-file development is not part of opening the editor. */
export function useEditorPreview(assetId: number, recipe: EditRecipe, libraryId: string, nativeCanvas = false) {
  const [retry, setRetry] = useState(0);
  const [session, setSession] = useState<EditorPreviewSession | null>(null);
  const [url, setUrl] = useState<string | null>(null);
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
  const openedMode=useRef<boolean|null>(null);
  const [prepareError,setPrepareError]=useState<string|null>(null);

  useEffect(() => {
    if (!enabled) { setSession(null); setUrl(null); return; }
    let cancelled = false;
    let id: string | null = null;
    setSession(null); setUrl(null); setError(null);setPrepareError(null);
    openedMode.current=null;
    const opening = editPreviewOpen(assetId, libraryId,rawlab);
    openedRequest.current = opening;
    void opening.then((value) => {
      id = value.sessionId;
      if (cancelled) void editPreviewClose(id).catch(() => {});
      else {
        openedMode.current=rawlab;setSession(value);

      }
    }).catch((e: unknown) => { if (!cancelled) setError(String(e)); });
    return () => { cancelled = true; if (openedRequest.current === opening) openedRequest.current = null; if (id) void editPreviewClose(id).catch(() => {}); };
  }, [assetId, libraryId, retry, enabled,rawlab]);

  useSmartPreview({session,enabled:rawlab&&openedMode.current===rawlab,latest,onPrepared:value=>{closeScheduler.current?.();setUrl(value.frame);setSession(value.session);},onError:setPrepareError});

  useEffect(() => {
    if (!session || nativeCanvas) { setPending(false); return; }
    const schedulerHandle = startPreviewScheduler(session.sessionId, () => latest.current, { setUrl, setError, setPending },session.sourceUrl);
    scheduler.current = schedulerHandle.wake;closeScheduler.current=schedulerHandle.close;
    return () => { scheduler.current = null; closeScheduler.current=null;schedulerHandle.close(); };
  }, [session, nativeCanvas]);
  useEffect(() => { scheduler.current?.(); }, [signature]);
  return { enabled, session, url: url ?? session?.sourceUrl ?? null, pending,
    loading: enabled && !session && !error, error, warning:prepareError, close: async () => {
      const request = openedRequest.current;
      if (request) { try { const value = await request; await editPreviewClose(value.sessionId); } catch (error) { console.warn("Could not release editor preview session", error); } }
    }, reload: () => setRetry((n) => n + 1) };
}
