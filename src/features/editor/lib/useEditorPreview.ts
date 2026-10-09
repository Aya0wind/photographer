import { useEffect, useRef, useState } from "react";
import {
  editPreviewClose, editPreviewOpen, editPreviewRender,
  type EditRecipe, type EditorPreviewSession,
} from "@/ipc/api";

/** 单个在途任务 + 最新请求优先；不为每次 pointermove 解码源文件。 */
export function useEditorPreview(assetId: number, recipe: EditRecipe, libraryId: string) {
  const enabled = recipe.renderer === "photocraft";
  const [retry, setRetry] = useState(0);
  const [opened, setOpened] = useState<{ assetId: number; retry: number; session: EditorPreviewSession } | null>(null);
  const [result, setResult] = useState<{ sessionId: string; url: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const queue = useRef<Promise<void>>(Promise.resolve());
  const session = enabled && opened?.assetId === assetId && opened.retry === retry ? opened.session : null;
  const adjustments = JSON.stringify({ adjustments: recipe.adjustments, advanced: recipe.advanced });

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    let id: string | null = null;
    setOpened(null);
    setResult(null);
    setError(null);
    void editPreviewOpen(assetId, libraryId).then((value) => {
      id = value.sessionId;
      if (cancelled) void editPreviewClose(id).catch(() => {});
      else setOpened({ assetId, retry, session: value });
    }).catch((e: unknown) => {
      if (!cancelled) setError(String(e));
    });
    return () => {
      cancelled = true;
      if (id) void editPreviewClose(id).catch(() => {});
    };
  }, [assetId, libraryId, enabled, retry]);

  useEffect(() => {
    if (!session) return;
    let cancelled = false;
    const values: Pick<EditRecipe, "adjustments" | "advanced"> = JSON.parse(adjustments);
    const basicZero = !values.adjustments || Object.values(values.adjustments).every((value) => value === 0);
    const advancedZero = !values.advanced || (values.advanced.curves.length === 0 &&
      [values.advanced.exposure, values.advanced.temperature, values.advanced.tint, values.advanced.vibrance].every((value) => value === 0));
    if (basicZero && advancedZero) {
      setResult({ sessionId: session.sessionId, url: session.sourceUrl });
      setPending(false);
      setError(null);
      return;
    }
    setPending(true);
    const timer = window.setTimeout(() => {
      queue.current = queue.current.then(async () => {
        if (cancelled) return;
        try {
          const url = await editPreviewRender(session.sessionId, {
            version: 1, renderer: "photocraft", rotateQuarter: 0, crop: null,
            ...values, textLayers: [], brushStrokes: [], output: { longEdge: null, quality: 90 },
          });
          if (!cancelled) {
            setResult({ sessionId: session.sessionId, url });
            setError(null);
          }
        } catch (e: unknown) {
          if (!cancelled) setError(String(e));
        } finally {
          if (!cancelled) setPending(false);
        }
      });
    }, 120);
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [session, adjustments]);

  return {
    enabled, session,
    url: session ? result?.sessionId === session.sessionId ? result.url : session.sourceUrl : null,
    loading: enabled && !session && !error,
    pending, error: enabled ? error : null,
    reload: () => setRetry((value) => value + 1),
  };
}
