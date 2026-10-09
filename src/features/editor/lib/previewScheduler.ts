import { editPreviewRender, editPreviewStats, type EditRecipe } from "@/ipc/api";
import { preparePreviewImage, releasePreviewImage } from "./previewImages";

interface Callbacks {
  setUrl: (url: string) => void;
  setError: (error: string | null) => void;
  setPending: (value: boolean) => void;
}

function measure(name: string, start: number, detail?: unknown) {
  // 每类只保留最新样本；浏览器不支持扩展 measure 时不影响编辑。
  try {
    performance.clearMeasures(name);
    performance.measure(name, { start, end: performance.now(), detail });
  } catch { /* 浏览器性能诊断是可选项。 */ }
}

/** One fixed-resolution, latest-only stream. Keep the displayed frame until the
 * replacement is decoded; dragging never swaps between low/high resolution. */
export function startPreviewScheduler(sessionId: string, getRecipe: () => EditRecipe, callbacks: Callbacks) {
  let disposed=false, running=false, dirty=false, revision=0, changedAt=0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const urls:string[]=[];
  async function pump() {
    if (disposed || running || !dirty) return;
    dirty=false; running=true;
    const version=revision, requestedAt=changedAt;
    callbacks.setPending(true);
    try {
      const next=await editPreviewRender(sessionId,getRecipe(),true,version);
      await preparePreviewImage(next);
      if (disposed || version!==revision) { releasePreviewImage(next); return; }
      urls.push(next); callbacks.setUrl(next); callbacks.setError(null);
      if (urls.length>3) releasePreviewImage(urls.shift()!);
      measure("editor.preview.interactive.frame-ready",requestedAt);
      if (import.meta.env.DEV) void editPreviewStats(sessionId).then(stats => {
        if (!disposed && version===revision) measure("editor.preview.diagnostics",requestedAt,stats);
      }).catch(() => {});
    } catch(e:unknown) { if(!disposed && version===revision) callbacks.setError(String(e)); }
    finally {
      running=false;
      if(!disposed) {
        dirty ||= version!==revision;
        if(dirty) timer=setTimeout(() => void pump(),40);
        else callbacks.setPending(false);
      }
    }
  }
  function wake() {
    revision++;changedAt=performance.now();dirty=true;
    clearTimeout(timer);
    if(!running) timer=setTimeout(() => void pump(),40);
  }
  function close() {
    disposed=true;clearTimeout(timer);
    for(const url of urls) releasePreviewImage(url);
  }
  wake();
  return {wake,close};
}
