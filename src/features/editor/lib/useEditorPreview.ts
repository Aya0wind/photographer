import { useEffect, useMemo, useRef, useState } from "react";
import { editPreviewClose, editPreviewOpen, type EditRecipe, type EditorPreviewSession } from "@/ipc/api";

import { startPreviewScheduler } from "./previewScheduler";

/** 快速预览和完整显影分别限制一个在途请求，完整显影不阻塞拖动。 */
export function useEditorPreview(assetId: number, recipe: EditRecipe, libraryId: string) {
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
  const enabled = recipe.renderer === "photocraft";

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    let id: string | null = null;
    setSession(null); setUrl(null); setError(null);
    const opening = editPreviewOpen(assetId, libraryId);
    openedRequest.current = opening;
    void opening.then((value) => {
      id = value.sessionId;
      if (cancelled) void editPreviewClose(id).catch(() => {});
      else setSession(value);
    }).catch((e: unknown) => { if (!cancelled) setError(String(e)); });
    return () => { cancelled = true; if (id) void editPreviewClose(id).catch(() => {}); };
  }, [assetId, libraryId, retry, enabled]);

  useEffect(() => {
    if (!session) return;
    const schedulerHandle = startPreviewScheduler(session.sessionId, () => latest.current, { setUrl, setError, setPending });
    scheduler.current = schedulerHandle.wake;
    return () => { scheduler.current = null; schedulerHandle.close(); };
  }, [session]);
  useEffect(() => { scheduler.current?.(); }, [signature]);
  return { enabled, session, url: url ?? session?.sourceUrl ?? null, pending,
    loading: enabled && !session && !error, error, close: async () => {
      const request = openedRequest.current;
      if (request) { try { const value = await request; await editPreviewClose(value.sessionId); } catch (error) { console.warn("Could not release editor preview session", error); } }
    }, reload: () => setRetry((n) => n + 1) };
}
