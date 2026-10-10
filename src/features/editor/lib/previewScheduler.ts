import { editPreviewRender, editPreviewStats, type EditRecipe } from "@/ipc/api";
import { preparePreviewImage, releasePreviewImage } from "./previewImages";
import {developmentActive} from "./advancedRecipe";

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

/** The opened source already represents this recipe; no GPU/JPEG work is needed. */
function neutralTone(recipe:Pick<EditRecipe,"adjustments"|"advanced"|"legacyAdjustments">) {
  const a=recipe.advanced;
  return (!recipe.legacyAdjustments || Object.values(recipe.legacyAdjustments).every(v=>v===0))
    && (!recipe.adjustments || Object.values(recipe.adjustments).every(v=>v===0))
    && (!a || (!(a.exposure || a.temperature || a.tint || a.vibrance) && !a.curves?.length
      && (!a.lookup || !a.lookup.enabled || a.lookup.amount===0) && !a.levels && !Object.keys(a.hsl??{}).length
      && !developmentActive(a.development) && !a.blackWhite?.enabled && !Object.values(a.selectiveColor?.ranges??{}).some(row=>row?.some(v=>v!==0))
      && (!a.colorBalance||![...a.colorBalance.shadows,...a.colorBalance.midtones,...a.colorBalance.highlights].some(v=>v!==0))
      && Object.values(a.channelCurves??{}).every(points=>!points.length)));
}
function isNeutral(recipe:EditRecipe) {
 return !recipe.rotateQuarter && !(recipe.geometry?.angle||recipe.geometry?.flipHorizontal||recipe.geometry?.flipVertical) && !recipe.crop && !recipe.textLayers.length && !recipe.brushStrokes.length
  && neutralTone(recipe) && !(recipe.masks??[]).some(m=>m.enabled&&m.density>0&&!neutralTone(m));
}

/** One fixed-resolution, latest-only stream. Keep the displayed frame until the
 * replacement is decoded; dragging never swaps between low/high resolution. */
export function startPreviewScheduler(sessionId: string, getRecipe: () => EditRecipe, callbacks: Callbacks, sourceUrl?:string) {
  let disposed=false, running=false, dirty=false, revision=0, changedAt=0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const urls:string[]=[];
  async function pump() {
    timer=undefined;
    if (disposed || running || !dirty) return;
    dirty=false; running=true;
    const version=revision, requestedAt=changedAt;
    const recipe=getRecipe();
    if(sourceUrl && isNeutral(recipe)) {
      running=false;callbacks.setUrl(sourceUrl);callbacks.setError(null);callbacks.setPending(false);return;
    }
    callbacks.setPending(true);
    try {
      const next=await editPreviewRender(sessionId,recipe,true,version);
      await preparePreviewImage(next);
      // Display each completed frame while a newer gesture is pending. Dropping
      // every in-flight result starves the preview during continuous dragging.
      if (disposed || (version!==revision && sourceUrl && isNeutral(getRecipe()))) { releasePreviewImage(next); return; }
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
        if(dirty) timer=setTimeout(() => void pump(),0);
        else callbacks.setPending(false);
      }
    }
  }
  function wake() {
    revision++;changedAt=performance.now();dirty=true;
    if(!running && timer===undefined) timer=setTimeout(() => void pump(),16);
  }
  function close() {
    disposed=true;clearTimeout(timer);
    for(const url of urls) releasePreviewImage(url);
  }
  wake();
  return {wake,close};
}
