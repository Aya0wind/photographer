import {useState} from "react";
import {useTranslation} from "react-i18next";
import {appForceExit, editExportCancel} from "@/ipc/api";

const STAGES:Record<string,number>={queued:2,decode:10,develop:25,film:40,gpu:40,cpuFallback:40,resize:55,render:65,encode:80,write:95,register:95};
/** Stage progress: never invent per-pixel percentages during native calls. */
export default function EditorExportProgress({assetId,libraryId,jobId,phase,onError}:{assetId:number;libraryId:string;jobId:number;phase:string;onError:(message:string)=>void}) {
  const {t}=useTranslation();
  const [cancelling,setCancelling]=useState(false);
  const [confirmExit,setConfirmExit]=useState(false);
  async function cancel() {
    setCancelling(true);
    try {await editExportCancel(assetId,libraryId,jobId);} catch(error) {setCancelling(false);onError(String(error));}
  }
  return <section className="shrink-0 border-b border-edge bg-panel px-4 py-3 text-xs" aria-label={t("advancedEditor.exporting")}>
    <div className="flex items-center gap-3"><span role="status">{t(cancelling?"editor.exportCancelling":"advancedEditor.exporting")} · {t(`editor.exportPhase.${phase}`,{defaultValue:phase})}</span>
      <button className="ml-auto rounded border border-edge px-3 py-1" disabled={cancelling||phase==="write"||phase==="register"} onClick={()=>void cancel()}>{t("editor.cancelExport")}</button>
      <button className="rounded border border-edge px-3 py-1 text-danger" onClick={()=>setConfirmExit(true)}>{t("editor.forceExit")}</button>
    </div>
    <progress className="mt-2 h-2 w-full accent-accent" max={100} value={STAGES[phase]??2} aria-label={t("editor.exportStageProgress")}/>
    <p className="mt-1 text-text-muted">{t("editor.exportStageHint")}</p>
    {confirmExit&&<div role="alertdialog" aria-label={t("editor.forceExit")} className="mt-2 flex items-center gap-3"><span>{t("editor.forceExitWarning")}</span><button onClick={()=>setConfirmExit(false)}>{t("common.cancel")}</button><button className="text-danger" onClick={()=>void appForceExit().catch(error=>onError(String(error)))}>{t("editor.forceExitConfirm")}</button></div>}
  </section>;
}
