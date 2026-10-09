import { ipc } from "../index";
import { ipcList } from "../read";
import { type AssetDto, type TrashPurgeResult } from "./types";

/** 批量设置颜色标签（asset_label_set；label=null 清除）。命令失败静默（乐观 UI 由调用方回滚/重拉） */
export async function assetLabelSet(assetIds: number[], label: string | null): Promise<void> {
  try {
    await ipc<void>("asset_label_set", { assetIds, label });
  } catch {
    // 静默
  }
}

/** 批量设置拒绝旗标（asset_reject_set；与星级分层的独立标记）。命令失败静默 */
export async function assetRejectSet(assetIds: number[], rejected: boolean): Promise<void> {
  try {
    await ipc<void>("asset_reject_set", { assetIds, rejected });
  } catch {
    // 静默
  }
}

/** 移入回收站（asset_trash_move：软删，常规查询后端自动排除）。命令失败静默 */
export async function assetTrashMove(assetIds: number[]): Promise<void> {
  try {
    await ipc<void>("asset_trash_move", { assetIds });
  } catch {
    // 静默
  }
}

/** 移入回收站（可抛错版：相似照片页等需要错误透传给用户的调用方）。 */
export async function assetTrashMoveChecked(assetIds: number[]): Promise<void> {
  await ipc<void>("asset_trash_move", { assetIds });
}

/** 回收站清单（trash_list；trashedAt DESC keyset：afterId=上一页末条 id，首页传 0）。
 *  失败/非数组回退 []——UI 自然降级空态。 */
export async function trashList(afterId: number, limit: number): Promise<AssetDto[]> {
  return ipcList<AssetDto>("trash_list", { afterId, limit });
}

/** 从回收站恢复（trash_restore：常规查询重新可见）。失败 false（调用方提示） */
export async function trashRestore(assetIds: number[]): Promise<boolean> {
  try {
    await ipc<void>("trash_restore", { assetIds });
    return true;
  } catch {
    return false;
  }
}

/** 彻底删除（trash_purge，2026-10-09 §五 定案语义）：在线库真删本体
 *  （deleteFiles=true 且非外部引用）；离线库回收站项原样保留（不删文件
 *  不删记录）；缺失项仅删记录。返回总结（offlineKept/offlineLibraries
 *  供「N 项因照片库离线保留」提示）；业务错误原样抛给调用方展示。 */
export async function trashPurge(
  assetIds: number[],
  deleteFiles: boolean,
): Promise<TrashPurgeResult> {
  return ipc<TrashPurgeResult>("trash_purge", { assetIds, deleteFiles });
}

/** 清理所有源缺失照片（assets_purge_missing）：扫描库内活跃资产，源文件
 *  不存在的行永久删除（级联清引用）。返回删除数；业务错误抛给调用方。 */
export async function assetsPurgeMissing(): Promise<number> {
  const purged = await ipc<number>("assets_purge_missing");
  return typeof purged === "number" && Number.isFinite(purged) ? purged : 0;
}
