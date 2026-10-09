import { useEffect, useRef, useState } from "react";
import { editPreviewClose, editPreviewOpen, editPreviewRender, type EditRecipe, type EditorPreviewSession } from "@/ipc/api";

/** 快速预览和完整显影分别限制一个在途请求，完整显影不阻塞拖动。 */
export function useEditorPreview(assetId: number, recipe: EditRecipe, libraryId: string) {
  const [retry, setRetry] = useState(0);
  const [session, setSession] = useState<EditorPreviewSession | null>(null);
  const [url, setUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const latest = useRef(recipe);
  latest.current = recipe;
  const signature = JSON.stringify(latest.current);
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
    const schedulerHandle = startScheduler(session.sessionId, () => latest.current, { setUrl, setError, setPending });
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

function startScheduler(sessionId: string, getRecipe: () => EditRecipe, callbacks: {
  setUrl: (url: string) => void; setError: (error: string | null) => void; setPending: (value: boolean) => void;
}) {
  const { setUrl, setError, setPending } = callbacks;
    let disposed = false, running = false, refining = false, dirty = false;
    let revision = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const urls: string[] = [];
    const display = (next: string) => {
      urls.push(next); setUrl(next); setError(null);
      if (urls.length > 3) URL.revokeObjectURL(urls.shift()!);
    };
    const refine = async () => {
      if (disposed || refining || running || dirty) return;
      refining = true;
      const version = revision;
      try {
        const next = await editPreviewRender(sessionId, getRecipe(), false);
        if (disposed || version !== revision) URL.revokeObjectURL(next);
        else { display(next); setPending(false); }
      } catch (e: unknown) { if (!disposed && version === revision) setError(String(e)); }
      finally {
        refining = false;
        if (!disposed && version !== revision) timer = setTimeout(() => void refine(), 180);
      }
    };
    const pump = async () => {
      if (disposed || running || !dirty) return;
      dirty = false; running = true;
      const version = revision;
      setPending(true);
      try {
        const next = await editPreviewRender(sessionId, getRecipe(), true);
        if (disposed) URL.revokeObjectURL(next);
        else display(next);
      } catch (e: unknown) { if (!disposed && version === revision) setError(String(e)); }
      finally {
        running = false;
        if (!disposed) {
          if (version !== revision) dirty = true;
          clearTimeout(timer);
          timer = setTimeout(() => void (dirty ? pump() : refine()), dirty ? 16 : 180);
        }
      }
    };
    const wake = () => {
      revision++; dirty = true; clearTimeout(timer);
      if (!running) void pump();
    };
    wake();
    const close = () => {
      disposed = true; clearTimeout(timer);
      for (const value of urls) URL.revokeObjectURL(value);
    };
    return { wake, close };
}
