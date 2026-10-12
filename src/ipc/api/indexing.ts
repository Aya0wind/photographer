import { ipc } from "../index";
import { type IndexKind, type IndexStatus, type RebuildKind } from "./types";

/** 索引池（index_task_pause/resume 的 pool 参数）："index" = 图片索引池
 *  （thumb/exif/hash/phash），"selection" = 选片索引池（eyes/blur）。 */
export type IndexPool = "index" | "selection";

/** 暂停索引任务（库级后台）；命令失败静默（后端接线前按钮无副作用） */
export async function indexTaskPause(pool: IndexPool = "index"): Promise<void> {
  try {
    await ipc<void>("index_task_pause", { pool });
  } catch {
  }
}

/** 恢复索引任务（池级闸）；导入让路期间后端拒绝（按钮已禁用，防御性静默） */
export async function indexTaskResume(pool: IndexPool = "index"): Promise<void> {
  try {
    await ipc<void>("index_task_resume", { pool });
  } catch {
  }
}

/** 索引状态快照；失败/负载异常返回 null（调用方隐藏/降级区块） */
export async function indexStatus(): Promise<IndexStatus | null> {
  try {
    const status = await ipc<IndexStatus | null>("index_status");
    if (status === null || typeof status !== "object") return null;
    const s = status as Partial<Record<IndexKind, unknown>>;
    const okThumb = s.thumb !== null && typeof s.thumb === "object";
    const okExif = s.exif !== null && typeof s.exif === "object";
    const okAi = s.ai !== null && typeof s.ai === "object";
    return okThumb && okExif && okAi ? (status as IndexStatus) : null;
  } catch {
    return null;
  }
}

/** 立即触发指定索引（幂等）。不 catch：ai 模型未就绪等业务错误（后端 Err 文案，
 *  如「请先在设置中下载模型」）由调用方提示；传输失败经 ipc() 统一置不可用标志 */
export async function indexKickNow(kind: IndexKind): Promise<void> {
  await ipc<void>("index_kick_now", { kind });
}

/** 重建指定索引（index_rebuild：清缓存/任务账重跑）。不 catch：失败文案透传给调用方 */
export async function indexRebuild(kind: RebuildKind): Promise<void> {
  await ipc<void>("index_rebuild", { kind });
}
