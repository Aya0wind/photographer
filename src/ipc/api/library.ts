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

/**
 * 新建照片库（photo_library_create）：reference=false 登记空文件夹（之后
 * copy/move 导入落盘目标 = 该库 root）；reference=true 从已有文件夹建立
 * （只登记不搬文件，后端触发递归扫描批量登记，进度走
 * libraryScanProgress/libraryScanFinished 事件）。业务错误（路径非法/与其他
 * 库或数据库目录重叠等）透传原始 Err 文案；invoke 不可用 error=null。
 */
export async function photoLibraryCreate(
  name: string,
  rootPath: string,
  reference: boolean,
): Promise<PhotoLibraryCreateResult> {
  try {
    const library = await ipc<PhotoLibrary>("photo_library_create", {
      name,
      rootPath,
      reference,
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
 * 移除登记（photo_library_remove）：永不删照片文件（用户红线）；deleteRecords=true
 * 连库内资产记录一并删（对话框问过用户后）。不 catch：失败文案透传给调用方提示。
 */
export async function photoLibraryRemove(
  id: string,
  deleteRecords: boolean,
): Promise<PhotoLibraryRemoveResult> {
  return ipc<PhotoLibraryRemoveResult>("photo_library_remove", { id, deleteRecords });
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
