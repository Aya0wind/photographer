import { useEffect, useRef, useState } from "react";
import { editPreviewClose, editPreviewOpen, editPreviewRender, type EditRecipe, type EditorPreviewSession } from "@/ipc/api";

/** 单个在途请求，滑动期间连续处理最新值，静止 180ms 后补高质量预览。 */
export function useEditorPreview(assetId: number, recipe: EditRecipe, libraryId: string) {
  const [retry, setRetry] = useState(0);
  const [session, setSession] = useState<EditorPreviewSession | null>(null);
  const [url, setUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const latest = useRef({ adjustments: recipe.adjustments, advanced: recipe.advanced });
  latest.current = { adjustments: recipe.adjustments, advanced: recipe.advanced };
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
    let disposed = false, running = false, dirty = false;
    let revision = 0, changedAt = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const urls: string[] = [];
    const pump = async () => {
      if (disposed || running || !dirty) return;
      dirty = false; running = true;
      const version = revision;
      const interactive = performance.now() - changedAt < 180;
      const values = latest.current;
      setPending(true);
      try {
        const next = await editPreviewRender(session.sessionId, {
          version: 1, renderer: "photocraft", rotateQuarter: 0, crop: null, ...values,
          textLayers: [], brushStrokes: [], output: { longEdge: null, quality: 90 },
        }, interactive);
        // 持续拖动时展示刚完成的帧，最新参数继续排在下一帧；不会回滚到旧会话。
        if (disposed) URL.revokeObjectURL(next);
        else {
          urls.push(next); setUrl(next); setError(null);
          // 保留最近三帧，让正在加载上一帧的 HTMLImageElement 有时间完成解码。
          if (urls.length > 3) URL.revokeObjectURL(urls.shift()!);
        }
      } catch (e: unknown) { if (!disposed && version === revision) setError(String(e)); }
      finally {
        running = false;
        if (!disposed) {
          if (version !== revision) dirty = true;
          if (dirty) { timer = setTimeout(() => void pump(), 16); }
          else if (interactive) { timer = setTimeout(() => { dirty = true; void pump(); }, Math.max(0, 180 - (performance.now() - changedAt))); }
          else setPending(false);
        }
      }
    };
    scheduler.current = () => {
      changedAt = performance.now(); revision++; dirty = true;
      clearTimeout(timer);
      if (!running) void pump();
    };
    scheduler.current();
    return () => {
      disposed = true; scheduler.current = null; clearTimeout(timer);
      for (const value of urls) URL.revokeObjectURL(value);
    };
  }, [session]);
  useEffect(() => { scheduler.current?.(); }, [signature]);
  return { enabled, session, url: url ?? session?.sourceUrl ?? null, pending,
    loading: enabled && !session && !error, error, close: async () => {
      const request = openedRequest.current;
      if (request) { try { const value = await request; await editPreviewClose(value.sessionId); } catch (error) { console.warn("Could not release editor preview session", error); } }
    }, reload: () => setRetry((n) => n + 1) };
}
