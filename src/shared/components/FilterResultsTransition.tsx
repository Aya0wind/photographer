import { useLayoutEffect, useRef, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { useMotionOn } from "@/lib/motion";

/** Keep the mounted grid while fetching; dim it, then fade in the replacement.
 * No key/remount, so loading never resets the thumbnail cache or virtualizer. */
export default function FilterResultsTransition({ busy, revision, children, loadingLabel }: {
  busy: boolean; revision: number; children: ReactNode; loadingLabel?: string;
}) {
  const {t}=useTranslation();
  const motionOn=useMotionOn();
  const content=useRef<HTMLDivElement>(null);
  useLayoutEffect(()=>{
    if(!motionOn||revision===0||busy)return;
    const animation=content.current?.animate?.([{opacity:0.25},{opacity:1}],{duration:220,easing:"ease-out"});
    return()=>animation?.cancel();
  },[revision,motionOn,busy]);
  return <div className="relative h-full min-h-0" aria-busy={busy} data-testid="filter-results-transition">
    <div ref={content} className="h-full min-h-0" inert={busy}
      style={{opacity:busy?0.25:1,transition:motionOn?"opacity 160ms ease-out":"none"}}>{children}</div>
    {busy&&<div className="absolute inset-0 z-20 flex items-center justify-center bg-bg/15" role="status" data-testid="filter-results-loading">
      <span className="ui-glass flex items-center gap-2 rounded-full border border-edge px-4 py-2 text-xs text-text-primary shadow-lg">
        <span className={`h-4 w-4 rounded-full border-2 border-edge border-t-accent ${motionOn?"animate-spin":""}`} aria-hidden="true"/>
        {loadingLabel ?? t("search.updatingResults")}
      </span>
    </div>}
  </div>;
}
