import { ipc } from "../index";
import { ipcList } from "../read";
import { type AiModelStatus, type AssetDto, type PersonCluster, type SemanticHit } from "./types";

/** 模型清单（ai_models_status 失败/非数组回退 []——UI 显示后端未连接态） */
export async function aiModelsStatus(): Promise<AiModelStatus[]> {
  return ipcList<AiModelStatus>("ai_models_status");
}

/** 开始下载模型；命令失败静默（UI 状态以 status 轮询/事件为准） */
export async function aiModelDownload(id: string, strict = false): Promise<void> {
  try {
    await ipc<void>("ai_model_download", { id });
  } catch (error) {
    if (strict) throw error;
  }
}

/** 取消下载；命令失败静默 */
export async function aiModelCancel(id: string): Promise<void> {
  try {
    await ipc<void>("ai_model_cancel", { id });
  } catch {
  }
}

/** 删除已安装模型释放磁盘；命令失败静默 */
export async function aiModelDelete(id: string): Promise<void> {
  try {
    await ipc<void>("ai_model_delete", { id });
  } catch {
  }
}

/** 一键清除人脸数据（聚类结果+特征向量，红色强确认后调用）；失败返回 false */
export async function aiFaceDataClear(): Promise<boolean> {
  try {
    const ok = await ipc<boolean | null>("ai_face_data_clear");
    return ok === true;
  } catch {
    return false;
  }
}

/** 后端 PersonRow 的公开载荷。历史前端曾把 `id` 误写成 `clusterId`，
 * 这里在 IPC 边界统一归一，避免 UI 再出现「人物 NaN」和 NaN 操作参数。 */
function normalizePersonCluster(value: unknown): PersonCluster | null {
  if (value === null || typeof value !== "object") return null;
  const row = value as Record<string, unknown>;
  const clusterId = typeof row.clusterId === "number" ? row.clusterId : row.id;
  const faceCount = row.faceCount;
  const coverAssetId = row.coverAssetId;
  if (
    typeof clusterId !== "number" || !Number.isFinite(clusterId) ||
    typeof faceCount !== "number" || !Number.isFinite(faceCount) ||
    typeof coverAssetId !== "number" || !Number.isFinite(coverAssetId)
  ) return null;
  return {
    clusterId,
    name: typeof row.name === "string" ? row.name : null,
    faceCount,
    coverAssetId,
  };
}

/**
 * 语义搜索（SIGLIP2 向量检索）；模型未就绪时后端返回明确错误字符串，
 * 本封装将其抛给调用方（区别于传输失败——用 isIpcAvailable 区分不了，故显式透传）。
 * @param query 自然语言描述（"海边日落"）；limit 默认 100；minScore 可选阈值
 */
export async function searchSemantic(
  query: string,
  limit: number,
  minScore?: number,
): Promise<SemanticHit[]> {
  const payload: Record<string, unknown> = { query, limit };
  if (minScore !== undefined) payload.minScore = minScore;
  const hits = await ipc<SemanticHit[]>("search_semantic", payload);
  return Array.isArray(hits) ? hits : [];
}

// --- M4 人物命令封装 ----------------------------------------------------------------

/** 人物清单（人脸聚类结果；失败/非数组回退 []——后端未就绪即空态兜底） */
export async function peopleList(): Promise<PersonCluster[]> {
  try {
    const list = await ipc<unknown>("people_list");
    if (!Array.isArray(list)) return [];
    return list
      .map(normalizePersonCluster)
      .filter((person): person is PersonCluster => person !== null);
  } catch {
    return [];
  }
}

/** 某人物聚类内的照片（序由后端保证）；失败回退 [] */
export async function peopleAssets(clusterId: number, limit: number): Promise<AssetDto[]> {
  return ipcList<AssetDto>("people_assets", { clusterId, limit });
}

/** 重命名人物（clusterId 聚类；空名由调用方拦下）；命令失败返回 false */
export async function personRename(clusterId: number, name: string): Promise<boolean> {
  try {
    await ipc<unknown>("person_rename", { clusterId, name });
    return true;
  } catch {
    return false;
  }
}

/** 删除人物聚类（仅拆聚类，照片不受影响）；命令失败返回 false */
export async function personDelete(clusterId: number): Promise<boolean> {
  try {
    await ipc<unknown>("person_delete", { clusterId });
    return true;
  } catch {
    return false;
  }
}
