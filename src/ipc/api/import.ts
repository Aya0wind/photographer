import { ipc } from "../index";
import { ipcList } from "../read";
import {
  type CleanCandidateDto,
  type CleanResultDto,
  type ImportPlan,
  type ImportStartResult,
  type JobRow,
  type LogRow,
} from "./types";
import { INVOKE_UNAVAILABLE_PATTERN } from "./errors";

/** 按方案启动导入会话；后端逻辑错误（如目标目录嵌套守卫）透出原始 Err 文案。
 *  可用性标志由 ipc() 统一维护（自愈式），此处不再手动置位。 */
export async function importStart(plan: ImportPlan): Promise<ImportStartResult> {
  try {
    const jobId = await ipc<number>("import_start", { plan });
    if (typeof jobId !== "number" || !Number.isFinite(jobId)) {
      return { ok: false, error: null };
    }
    return { ok: true, jobId };
  } catch (err) {
    const message = err instanceof Error ? err.message : typeof err === "string" ? err : "";
    if (!message || INVOKE_UNAVAILABLE_PATTERN.test(message)) {
      return { ok: false, error: null };
    }
    return { ok: false, error: message };
  }
}

/** 暂停任务（无返回值；失败静默并标记 IPC 不可用） */
export async function importPause(jobId: number): Promise<void> {
  try {
    await ipc<void>("import_pause", { jobId });
  } catch {
  }
}

export async function importResume(jobId: number): Promise<void> {
  try {
    await ipc<void>("import_resume", { jobId });
  } catch {
  }
}

export async function importCancel(jobId: number): Promise<void> {
  try {
    await ipc<void>("import_cancel", { jobId });
  } catch {
  }
}

/** 历史任务游标分页（afterId 升序取下一页） */
export async function importJobsPage(afterId: number, limit: number): Promise<JobRow[]> {
  try {
    return await ipc<JobRow[]>("import_jobs_page", { afterId, limit });
  } catch {
    return [];
  }
}

/** 单任务日志游标分页 */
export async function importLogsPage(
  jobId: number,
  afterId: number,
  limit: number,
): Promise<LogRow[]> {
  try {
    return await ipc<LogRow[]>("import_logs_page", { jobId, afterId, limit });
  } catch {
    return [];
  }
}

/** 重试任务的全部失败文件，返回新 jobId；失败返回 null */
export async function importRetryFailed(jobId: number): Promise<number | null> {
  try {
    return await ipc<number>("import_retry_failed", { jobId });
  } catch {
    return null;
  }
}

/** 清卡候选预览（该任务已入库且指纹匹配的源文件）；失败/无候选返回 [] */
export async function cleanCandidates(jobId: number): Promise<CleanCandidateDto[]> {
  return ipcList<CleanCandidateDto>("clean_candidates", { jobId });
}

/** 执行清卡（后端删除前逐文件复验指纹）；进行中/完成态由 cleanStarted/cleanFinished 事件驱动。
 *  返回值仅作兜底（命令失败返回 null），UI 状态以事件为准。 */
export async function cleanApply(jobId: number): Promise<CleanResultDto | null> {
  try {
    return await ipc<CleanResultDto>("clean_apply", { jobId });
  } catch {
    return null;
  }
}

/** 删除历史任务记录（import_job_delete；任务抽屉历史区 × 按钮）。不 catch：
 *  失败文案透传给调用方提示 */
export async function importJobDelete(jobId: number): Promise<void> {
  await ipc<void>("import_job_delete", { jobId });
}
