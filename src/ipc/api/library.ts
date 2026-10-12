import { ipc } from "../index";
import { ipcList } from "../read";
import {
  type LibraryScanStatus,
  type PhotoLibrary,
  type PhotoLibraryCreateResult,
  type PhotoLibraryRemoveResult,
  type SidebarCounts,
} from "./types";
import { INVOKE_UNAVAILABLE_PATTERN } from "./errors";

// --- 照片库登记表（photos_libraries，2026-10-09 单库多照片库定案） --------------------
// 后端 P0 骨架已注册命令（返回空数据/未实现错误），M1/M2 后端线填实；
// 前端线按本封装开发 M3「存储」页等 UI。

/** 照片库列表（photo_library_list）；命令失败/非数组回退 []——UI 自然降级空态 */
export async function photoLibraryList(): Promise<PhotoLibrary[]> {
  return ipcList<PhotoLibrary>("photo_library_list");
}

/** 统一照片库：已有文件原位建立索引，新增文件按库的组织规则存放。 */
export async function photoLibraryCreate(
  name: string,
  rootPath: string,
  organizationTemplate: string,
  existingFolder=false,
  reorganizeExisting=false,
): Promise<PhotoLibraryCreateResult> {
  try {
    const library = await ipc<PhotoLibrary>("photo_library_create", {
      name,
      rootPath,
      organizationTemplate,
      existingFolder,
      reorganizeExisting,
    });
    if (library === null || typeof library !== "object" || typeof library.id !== "string") {
      return { ok: false, error: null };
    }
    return { ok: true, library };
  } catch (err) {
    const message = err instanceof Error ? err.message : typeof err === "string" ? err : "";
    if (!message || INVOKE_UNAVAILABLE_PATTERN.test(message)) {
      return { ok: false, error: null };
    }
    return { ok: false, error: message };
  }
}

/**
 * 删除照片库（photo_library_remove）：登记与库内资产记录/派生数据一并删；
 * 默认不动磁盘照片文件，deleteFiles=true 连原文件删（确认短语后勾选；引用库
 * 原文件不由应用管理，后端拒绝代删）。不 catch：失败文案透传给调用方提示。
 */
export async function photoLibraryRemove(
  id: string,
  deleteFiles: boolean,
): Promise<PhotoLibraryRemoveResult> {
  return ipc<PhotoLibraryRemoveResult>("photo_library_remove", { id, deleteFiles });
}

/** 各照片库扫描任务状态（photo_library_scan_status）；失败/非数组回退 [] */
export async function photoLibraryScanStatus(): Promise<LibraryScanStatus[]> {
  return ipcList<LibraryScanStatus>("photo_library_scan_status");
}

/** 取消某库在跑的扫描任务（photo_library_scan_cancel；软信号，当前目录登记完即停）。
 *  命令失败静默 */
export async function photoLibraryScanCancel(libraryId: string): Promise<void> {
  try {
    await ipc<void>("photo_library_scan_cancel", { libraryId });
  } catch {
    // 静默
  }
}

// --- 侧栏计数 ------------------------------------------------------------------------

export async function sidebarCounts(): Promise<SidebarCounts | null> {
  try {
    const counts = await ipc<SidebarCounts | null>("sidebar_counts");
    if (counts === null || typeof counts !== "object") return null;
    const c = counts as Partial<Record<keyof SidebarCounts, unknown>>;
    const numOf = (v: unknown): number =>
      typeof v === "number" && Number.isFinite(v) ? v : 0;
    return {
      assets: numOf(c.assets),
      recentViewed: numOf(c.recentViewed),
      onThisDay: numOf(c.onThisDay),
      tags: numOf(c.tags),
      albums: numOf(c.albums),
    };
  } catch {
    return null;
  }
}

export async function photoLibraryUpdateTemplate(id:string,organizationTemplate:string):Promise<PhotoLibraryCreateResult>{
 try{const library=await ipc<PhotoLibrary>("photo_library_set_organization_template",{id,organizationTemplate});return {ok:true,library};}catch(error){return {ok:false,error:String(error)};}
}
export interface PhotoLibraryOrganizationProgress {libraryId:string;phase:"scan"|"move"|"done";completed:number;total:number}
export function photoLibraryReorganize(id:string){return ipc<PhotoLibraryOrganizationProgress>("photo_library_reorganize",{id});}
export function photoLibraryStopReorganization(id:string){return ipc<void>("photo_library_stop_reorganization",{id});}
