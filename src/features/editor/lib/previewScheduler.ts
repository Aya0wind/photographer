import { editPreviewRender, editPreviewStats, type EditRecipe } from "@/ipc/api";
import { preparePreviewImage, releasePreviewImage } from "./previewImages";

interface Callbacks {
  setUrl: (url: string) => void;
  setError: (error: string | null) => void;
  setPending: (value: boolean) => void;
}
const SETTLE_MS = 180;

function measure(name: string, start: number, detail?: unknown) {
  // 每类只保留最新样本；浏览器不支持扩展 measure 时不影响编辑。
  try {
    performance.clearMeasures(name);
    performance.measure(name, { start, end: performance.now(), detail });
  } catch { /* 浏览器性能诊断是可选项。 */ }
}

/** 快速帧和精细帧独立限流、独立定时器；后者不会覆盖快速通道的唤醒。 */
export function startPreviewScheduler(sessionId: string, getRecipe: () => EditRecipe, callbacks: Callbacks) {
  let disposed = false, running = false, refining = false, dirty = false;
  let revision = 0, changedAt = 0, displayed = -1, displayedRefined = false, refinedRevision = -1;
  let fastTimer: ReturnType<typeof setTimeout> | undefined;
  let refineTimer: ReturnType<typeof setTimeout> | undefined;
  const urls: string[] = [];

  function scheduleRefinement() {
    clearTimeout(refineTimer);
    if (!disposed && refinedRevision < revision) {
      refineTimer = setTimeout(() => void refine(), Math.max(0, SETTLE_MS - (performance.now() - changedAt)));
    }
  }

  async function publish(next: string, version: number, refined: boolean, requestedAt: number) {
    await preparePreviewImage(next);
    if (disposed || version < displayed || (version === displayed && displayedRefined) || (refined && version !== revision)) {
      releasePreviewImage(next);
      return;
    }
    displayed = version; displayedRefined = refined;
    urls.push(next); callbacks.setUrl(next); callbacks.setError(null);
    if (urls.length > 3) releasePreviewImage(urls.shift()!);
    measure(refined ? "editor.preview.refined.frame-ready" : "editor.preview.interactive.frame-ready", requestedAt);
  }

  async function refine() {
    if (disposed || refining || running || dirty || refinedRevision === revision) return;
    refining = true;
    const version = revision, requestedAt = changedAt;
    try {
      const next = await editPreviewRender(sessionId, getRecipe(), false, version);
      await publish(next, version, true, requestedAt);
      if (!disposed && version === revision) {
        refinedRevision = version; callbacks.setPending(false);
        // 按精细帧异步取分段耗时，不给交互帧增加一次诊断 IPC。
        const diagnosticStarted = performance.now();
        void editPreviewStats(sessionId).then((stats) => {
          if (!disposed && version === revision) measure("editor.preview.diagnostics", diagnosticStarted, stats);
        }).catch((error: unknown) => { if (!disposed) console.debug("Editor preview diagnostics unavailable", error); });
      }
    } catch (e: unknown) { if (!disposed && version === revision) callbacks.setError(String(e)); }
    finally {
      refining = false;
      if (!disposed && version !== revision) scheduleRefinement();
    }
  }

  async function pump() {
    if (disposed || running || !dirty) return;
    dirty = false; running = true;
    const version = revision, requestedAt = changedAt;
    callbacks.setPending(true);
    try {
      const next = await editPreviewRender(sessionId, getRecipe(), true, version);
      await publish(next, version, false, requestedAt);
    } catch (e: unknown) { if (!disposed && version === revision) callbacks.setError(String(e)); }
    finally {
      running = false;
      if (!disposed) {
        dirty ||= version !== revision;
        if (dirty) fastTimer = setTimeout(() => void pump(), 0);
        else scheduleRefinement();
      }
    }
  }

  function wake() {
    revision++; changedAt = performance.now(); dirty = true;
    clearTimeout(fastTimer); clearTimeout(refineTimer);
    if (!running) void pump();
  }
  function close() {
    disposed = true; clearTimeout(fastTimer); clearTimeout(refineTimer);
    for (const url of urls) releasePreviewImage(url);
  }
  wake();
  return { wake, close };
}
