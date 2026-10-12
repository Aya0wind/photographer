import { useState,useRef,useEffect } from "react";
import { useNavigate } from "react-router";
import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import OrganizationTemplateField from "./OrganizationTemplateField";
import { photoLibraryCreate,photoLibraryList,photoLibraryUpdateTemplate,photoLibraryStopReorganization, type PhotoLibrary,type PhotoLibraryOrganizationProgress } from "@/ipc/api";
import {organizeLibrary} from "../lib/reorganization";
import ErrorModal from "@/shared/components/ErrorModal";

/** 统一照片库：已有文件原位建立索引，新增文件按库的组织规则存放。 */

const FIELD_CLASS =
  "w-full rounded-md border border-edge bg-bg px-2.5 py-1.5 font-mono text-xs text-text-primary outline-none transition-colors focus:border-accent";

/** 从绝对路径取末段作为缺省库名（Windows/Unix 分隔符都认；取不到回退全路径） */
function baseName(path: string): string {
  const parts = path.split(/[\\/]+/).filter((s) => s.length > 0);
  return parts.length > 0 ? parts[parts.length - 1] : path;
}

export default function CreateLibraryDialog({
  onClose,
  onCreated,
  library,
  initialExistingFolder = false,
  initialRootPath = "",
  lockMode = false,
}: {
  onClose: () => void;
  library?:PhotoLibrary;
  initialExistingFolder?: boolean;
  initialRootPath?: string;
  lockMode?: boolean;
  /** 登记成功回调（mode = 实际使用的模式；存储页据此区分 toast 文案） */
  onCreated: (library: PhotoLibrary) => void;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const modalRoot = useRef<HTMLDivElement>(null);
  const organizationDetails = useRef<HTMLDetailsElement>(null);
  const [existingFolder,setExistingFolder]=useState(initialExistingFolder);
  const [libraries, setLibraries] = useState<PhotoLibrary[]>([]);
  useEffect(() => {
    if (!existingFolder || library) return;
    let cancelled = false;
    void photoLibraryList().then(rows => { if (!cancelled) setLibraries(rows); }).catch(() => {});
    return () => { cancelled = true; };
  }, [existingFolder, library]);
  const [organizationTemplate,setOrganizationTemplate]=useState(library?.organizationTemplate??"{year}/{month}/{day}/{filename}");
  const [reorganizeExisting,setReorganizeExisting]=useState(false);
  const createdLibrary=useRef<PhotoLibrary|null>(library?.organizationPending?library:null);
  const [progress,setProgress]=useState<PhotoLibraryOrganizationProgress|null>(null);
  const [name, setName] = useState(library?.name??(initialRootPath ? baseName(initialRootPath) : ""));
  const [rootPath, setRootPath] = useState(library?.rootPath??initialRootPath);
  /** 名称未被手改过时，选完文件夹自动带出末段作缺省名 */
  const [nameTouched, setNameTouched] = useState(false);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  function pathKey(path: string) {
    const normalized = path.trim().replace(/\\/g, "/").replace(/\/+$/, "");
    return /^[a-z]:|^\/\//i.test(normalized) ? normalized.toLowerCase() : normalized;
  }
  const matchedLibrary = !library && existingFolder ? libraries.find(item => {
    const root = pathKey(item.rootPath), selected = pathKey(rootPath);
    if (selected.split("/").some(part => part === "." || part === "..")) return false;
    return selected === root || selected.startsWith(`${root}/`);
  }) : undefined;
  const ready = (Boolean(matchedLibrary) || name.trim().length > 0) && rootPath.trim().length > 0 && !creating;

  async function pickDirectory(): Promise<void> {
    try {
      const dir = await openDialog({
        directory: true,
        defaultPath: rootPath.trim() || undefined,
      });
      if (typeof dir === "string" && dir.length > 0) {
        setRootPath(dir);
        if (!nameTouched) setName(baseName(dir));
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function create(): Promise<void> {
    if (!ready) return;
    setCreating(true);
    setError(null);
    try{
    const result = createdLibrary.current ? {ok:true as const,library:createdLibrary.current} : library ? await photoLibraryUpdateTemplate(library.id,organizationTemplate) : await photoLibraryCreate(
      matchedLibrary?.name ?? name.trim(),
      rootPath.trim(),
      organizationTemplate,
      existingFolder,
      matchedLibrary ? false : reorganizeExisting,
    );
    setCreating(false);
    if (result.ok) {
      if(result.library.organizationPending||(!library&&!matchedLibrary&&reorganizeExisting)){
        createdLibrary.current=result.library;setCreating(true);
        await organizeLibrary(result.library.id,setProgress);
      }
      onCreated({...result.library,organizationPending:false});
      return;
    }
    // null=invoke 不可用（后端未连接）；字符串=后端业务 Err 文案透传
    setError(
      result.error === null ? t("storage.createDialog.unavailable") : result.error,
    );
    }catch(error){setError(String(error).replace(/^Error:\s*/,""));}finally{setCreating(false);}
  }

  return (
    <div
      ref={modalRoot}
      className="fixed inset-0 z-[70] flex items-center justify-center bg-black/60"
      onClick={()=>{if(!creating)onClose();}}
      role="dialog"
      aria-modal="true"
      data-testid="storage-create-dialog"
    >
      <div
        className="sp-scroll max-h-[90vh] w-[min(520px,calc(100vw-32px))] overflow-y-auto rounded-2xl border border-edge bg-surface p-6"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="text-sm font-semibold text-text-primary">
          {t(library?.organizationPending?"storage.organization.resume":library?"storage.organization.label":existingFolder?"storage.addExisting":"storage.create")}
        </h2>

        <p className="mt-2 text-xs leading-relaxed text-text-secondary">{t(existingFolder?"storage.organization.existingHint":"addPhotos.emptyLibraryHint")}</p>
        {!library&&!lockMode&&<div className="mt-2 grid grid-cols-2 gap-2">{[false,true].map(existing=><button key={String(existing)} type="button" aria-pressed={existingFolder===existing} disabled={creating||!!createdLibrary.current} onClick={()=>{if(existingFolder!==existing){setExistingFolder(existing);setReorganizeExisting(false);setRootPath("");}}} className="rounded-lg border border-edge p-2 text-xs aria-pressed:border-accent">{t(existing?"storage.organization.useExisting":"storage.organization.newFolder")}</button>)}</div>}
        {matchedLibrary && <div className="mt-4 rounded-xl border border-accent/30 bg-accent/5 p-4 text-sm" data-testid="storage-existing-library-match"><p>{t("addPhotos.belongsTo", { name: matchedLibrary.name })}</p><p className="mt-2 break-all text-xs text-text-secondary">{matchedLibrary.rootPath}</p><button type="button" disabled={creating} className="mt-3 text-xs text-accent hover:underline" onClick={() => { onClose(); navigate(`/storage#library-${matchedLibrary.id}`); }}>{t("addPhotos.viewLibrary", { name: matchedLibrary.name })}</button></div>}
        {creating&&progress&&<p role="status" className="mt-3 text-xs text-text-secondary">{progress.phase==="scan"?t("storage.organization.scanning"):t("storage.organization.moving",{completed:progress.completed,total:progress.total})}</p>}
        {!creating&&createdLibrary.current?.organizationPending&&<button type="button" className="mt-3 text-left text-xs text-text-secondary underline" onClick={()=>{setCreating(true);void photoLibraryStopReorganization(createdLibrary.current!.id).then(()=>onCreated({...createdLibrary.current!,organizationPending:false})).catch(error=>setError(String(error))).finally(()=>setCreating(false));}}>{t("storage.organization.stopContinue")}</button>}
        {!matchedLibrary && <label className="mt-3 flex flex-col gap-1 text-xs text-text-secondary">
          {t("storage.createDialog.name")}
          <input
            type="text"
          disabled={creating||!!library||!!createdLibrary.current}
            value={name}
            onChange={(e) => {
              setName(e.target.value);
              setNameTouched(true);
            }}
            className={FIELD_CLASS}
            autoFocus
            data-testid="storage-create-name"
          />
        </label>}

        <label className="mt-3 flex flex-col gap-1 text-xs text-text-secondary">
          {t("storage.createDialog.root")}
          <div className="flex gap-2">
            <input
              type="text"
              disabled={creating||!!library||!!createdLibrary.current}
              value={rootPath}
              onChange={(e) => setRootPath(e.target.value)}
              className={FIELD_CLASS}
              data-testid="storage-create-root"
            />
            <button
              type="button"
              onClick={() => void pickDirectory()}
              disabled={creating||!!library||!!createdLibrary.current}
              className="shrink-0 rounded-md border border-edge px-3 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
              data-testid="storage-create-browse"
            >
              {t("storage.createDialog.browse")}
            </button>
          </div>
        </label>

        {!matchedLibrary && (library ? <fieldset disabled={creating||!!createdLibrary.current}><OrganizationTemplateField value={organizationTemplate} onChange={setOrganizationTemplate}/></fieldset> : <details ref={organizationDetails} className="mt-4 rounded-xl border border-edge p-3" data-testid="storage-organization-options"><summary className="cursor-pointer text-xs font-medium text-text-secondary">{t("addPhotos.organizationOptions")}</summary><fieldset className="mt-3" disabled={creating||!!createdLibrary.current}><OrganizationTemplateField value={organizationTemplate} onChange={setOrganizationTemplate}/></fieldset></details>)}
        {!library&&existingFolder&&!matchedLibrary&&<div className="mt-3 space-y-2 rounded-xl border border-edge p-3"><label className="flex items-center gap-2 text-xs"><input type="checkbox" checked={reorganizeExisting} disabled={creating||!!createdLibrary.current} onChange={e=>{setReorganizeExisting(e.target.checked);if(e.target.checked&&organizationDetails.current)organizationDetails.current.open=true;}}/>{t("storage.organization.reorganize")}</label>{reorganizeExisting&&<p className="text-[11px] leading-relaxed text-amber-400">{t("storage.organization.reorganizeNotice")}</p>}</div>}

        <ErrorModal message={error} onClose={()=>setError(null)} portalTarget={modalRoot.current}/>

        <div className="mt-4 flex justify-end gap-2">
          <button
            type="button"
            onClick={onClose}
            disabled={creating}
            className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
            data-testid="storage-create-cancel"
          >
            {t("common.cancel")}
          </button>
          <button
            type="button"
            disabled={!ready}
            onClick={() => void create()}
            className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="storage-create-confirm"
          >
            {creating ? t("storage.createDialog.creating") : matchedLibrary ? t("addPhotos.scanLibrary", { name: matchedLibrary.name }) : library ? t("storage.createDialog.confirm") : existingFolder ? t(reorganizeExisting?"addPhotos.organizeAndAdd":"addPhotos.addAndScan") : t("storage.create")}
          </button>
        </div>
      </div>
    </div>
  );
}
