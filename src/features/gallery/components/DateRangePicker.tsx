import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";

export default function DateRangePicker({ from, to, onApply, ranges }: {
  from: string; to: string; onApply: (range: { from: string; to: string }) => void;
  ranges: () => Array<{ key: string; label: string; from: string; to: string }>;
}) {
  const { t }=useTranslation();
  const [open,setOpen]=useState(false);
  const [draft,setDraft]=useState({from,to});
  const trigger=useRef<HTMLButtonElement>(null);
  const panel=useRef<HTMLDivElement>(null);
  const close=()=>{setOpen(false);trigger.current?.focus();};
  useEffect(()=>{
    if(!open)return;
    panel.current?.querySelector<HTMLInputElement>("input")?.focus();
    const keydown=(event:KeyboardEvent)=>{
      if(event.key==="Escape"){event.preventDefault();event.stopPropagation();setOpen(false);trigger.current?.focus();}
      if(event.key==="Tab"){
        const fields=panel.current?.querySelectorAll<HTMLElement>("input,button:not([disabled])");
        if(!fields?.length)return;
        if(event.shiftKey&&document.activeElement===fields[0]){event.preventDefault();fields[fields.length-1]?.focus();}
        else if(!event.shiftKey&&document.activeElement===fields[fields.length-1]){event.preventDefault();fields[0]?.focus();}
      }
    };
    document.addEventListener("keydown",keydown,true);
    return()=>document.removeEventListener("keydown",keydown,true);
  },[open]);
  const invalid=!!draft.from&&!!draft.to&&draft.from>draft.to;
  const active=!!from||!!to;
  return <>
    <button ref={trigger} type="button" aria-expanded={open} aria-haspopup="dialog"
      aria-label={t("search.datePicker")} data-testid="search-date-picker"
      onClick={()=>{setDraft({from,to});setOpen(true);}}
      className={`inline-flex h-8 max-w-full items-center gap-2 rounded-md border px-2.5 text-xs transition-colors ${active?"border-accent/50 bg-accent/10 text-accent":"border-edge bg-panel/40 text-text-secondary hover:border-accent"}`}>
      <svg viewBox="0 0 20 20" width="15" height="15" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><rect x="3" y="4" width="14" height="13" rx="2"/><path d="M6 2v4m8-4v4M3 8h14"/></svg>
      <span className="truncate">{active?`${from||t("search.dateAny")} – ${to||t("search.dateAny")}`:t("search.datePicker")}</span>
    </button>
    {open&&createPortal(<div className="fixed inset-0 z-[90] flex items-center justify-center bg-black/35 p-4"
      onPointerDown={e=>{if(e.target===e.currentTarget)close();}}>
      <div ref={panel} role="dialog" aria-modal="true" aria-label={t("search.datePicker")}
        className="ui-glass w-full max-w-sm rounded-2xl border border-edge bg-surface p-5 shadow-2xl" data-testid="search-date-dialog">
        <h2 className="mb-4 text-sm font-semibold text-text-primary">{t("search.datePicker")}</h2>
        <div className="grid grid-cols-2 gap-3">
          <label className="min-w-0 space-y-1.5 text-xs text-text-secondary">{t("search.dateFrom")}
            <input type="date" value={draft.from} onChange={e=>setDraft(d=>({...d,from:e.target.value}))}
              data-testid="search-from" className="block h-9 w-full min-w-0 rounded-md border border-edge bg-bg px-2 text-text-primary"/>
          </label>
          <label className="min-w-0 space-y-1.5 text-xs text-text-secondary">{t("search.dateTo")}
            <input type="date" value={draft.to} onChange={e=>setDraft(d=>({...d,to:e.target.value}))}
              data-testid="search-to" className="block h-9 w-full min-w-0 rounded-md border border-edge bg-bg px-2 text-text-primary"/>
          </label>
        </div>
        <div className="mt-4 flex flex-wrap gap-2">
          {ranges().map(r=><button type="button" key={r.key} data-testid={`search-quick-${r.key}`}
            onClick={()=>setDraft({from:r.from,to:r.to})}
            className="rounded-md border border-edge px-2.5 py-1.5 text-xs text-text-secondary hover:border-accent hover:text-accent">{r.label}</button>)}
          <button type="button" onClick={()=>setDraft({from:"",to:""})} className="rounded-md border border-edge px-2.5 py-1.5 text-xs text-text-secondary">{t("search.dateAny")}</button>
        </div>
        {invalid&&<p role="alert" className="mt-3 text-xs text-amber-400">{t("search.dateInvalid")}</p>}
        <div className="mt-5 flex justify-end gap-2">
          <button type="button" onClick={close} className="rounded-md border border-edge px-4 py-2 text-xs text-text-secondary">{t("common.cancel")}</button>
          <button type="button" disabled={invalid} data-testid="search-date-apply"
            onClick={()=>{onApply(draft);close();}}
            className="rounded-md bg-accent px-4 py-2 text-xs font-medium text-black disabled:opacity-40">{t("common.confirm")}</button>
        </div>
      </div>
    </div>,document.body)}
  </>;
}
