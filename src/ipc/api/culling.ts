import { ipc } from "../index";
import {
  type CullAiRules,
  type CullDecisionPatch,
  type CullDecisionValue,
  type CullFinishApply,
  type CullFinishResult,
  type CullItemState,
  type CullPrescanDto,
  type CullScope,
  type CullSessionCreateResult,
  type CullSessionDto,
  type CullSessionOpenResult,
} from "./types";
import { INVOKE_UNAVAILABLE_PATTERN } from "./errors";

/** 脏数据容错：后端载荷 → CullSessionDto 归一（形状异常剔除/回退） */
function normalizeCullSession(value: unknown): CullSessionDto | null {
  if (value === null || typeof value !== "object") return null;
  const r = value as Record<string, unknown>;
  const numOf = (v: unknown): number =>
    typeof v === "number" && Number.isFinite(v) ? v : 0;
  if (typeof r.id !== "number" || !Number.isFinite(r.id) || typeof r.name !== "string") {
    return null;
  }
  // scope 归一：album 必须有 albumId；query 必须有 assetIds 数组；其余按 query 空集兜底
  const rawScope = r.scope !== null && typeof r.scope === "object"
    ? (r.scope as Record<string, unknown>)
    : {};
  let scope: CullScope;
  if (rawScope.kind === "album" && typeof rawScope.albumId === "number") {
    scope = {
      kind: "album",
      albumId: rawScope.albumId,
      subgroup: typeof rawScope.subgroup === "string" && rawScope.subgroup !== "" ? rawScope.subgroup : null,
    };
  } else {
    scope = {
      kind: "query",
      assetIds: Array.isArray(rawScope.assetIds)
        ? rawScope.assetIds.filter((v): v is number => typeof v === "number" && Number.isFinite(v))
        : [],
    };
  }
  return {
    id: r.id,
    name: r.name,
    scope,
    total: numOf(r.total),
    accepted: numOf(r.accepted),
    rejected: numOf(r.rejected),
    undecided: numOf(r.undecided),
    createdAt: typeof r.createdAt === "string" ? r.createdAt : "",
    updatedAt: typeof r.updatedAt === "string" ? r.updatedAt : "",
    finishedAt: typeof r.finishedAt === "string" ? r.finishedAt : null,
  };
}

/** 新建选片会话（cull_session_create；空 scope/后端业务错误透传原始 Err 文案） */
export async function cullSessionCreate(scope: CullScope): Promise<CullSessionCreateResult> {
  try {
    const raw = await ipc<unknown>("cull_session_create", { scope });
    const session = normalizeCullSession(raw);
    if (session === null) return { ok: false, error: null };
    return { ok: true, session };
  } catch (err) {
    const message = err instanceof Error ? err.message : typeof err === "string" ? err : "";
    if (!message || INVOKE_UNAVAILABLE_PATTERN.test(message)) return { ok: false, error: null };
    return { ok: false, error: message };
  }
}

/** 会话清单（cull_session_list；失败/非数组/形状异常回退 []） */
export async function cullSessionList(): Promise<CullSessionDto[]> {
  try {
    const list = await ipc<unknown>("cull_session_list");
    if (!Array.isArray(list)) return [];
    return list
      .map(normalizeCullSession)
      .filter((s): s is CullSessionDto => s !== null);
  } catch {
    return [];
  }
}

/** 打开会话续选（cull_session_open：会话 + 决定表）；失败/形状异常返回 null */
export async function cullSessionOpen(id: number): Promise<CullSessionOpenResult | null> {
  try {
    const raw = await ipc<unknown>("cull_session_open", { sessionId: id });
    if (raw === null || typeof raw !== "object") return null;
    const r = raw as Record<string, unknown>;
    const session = normalizeCullSession(r.session);
    if (session === null) return null;
    const items: CullItemState[] = Array.isArray(r.items)
      ? r.items
          .filter(
            (i): i is Record<string, unknown> =>
              i !== null && typeof i === "object" && typeof (i as Record<string, unknown>).assetId === "number",
          )
          .map((i) => ({
            assetId: i.assetId as number,
            decision:
              i.decision === "accepted" || i.decision === "rejected" ? (i.decision as CullDecisionValue) : null,
            origin: i.origin === "ai" ? "ai" : "manual",
            // burst 脏值容错：burstId 非有限数回 null；burstSize 回退 1（无组）
            burstId:
              typeof i.burstId === "number" && Number.isFinite(i.burstId) ? i.burstId : null,
            burstSize:
              typeof i.burstSize === "number" && Number.isFinite(i.burstSize) && i.burstSize >= 1
                ? Math.floor(i.burstSize)
                : 1,
          }))
      : [];
    return { session, items };
  } catch {
    return null;
  }
}

/** 批量写入决定（cull_decision_apply；单条过片也走此接口）。成功返回最新会话
 *  计数（进度真值），失败/形状异常返回 null（调用方回滚乐观更新） */
export async function cullDecisionApply(
  sessionId: number,
  decisions: CullDecisionPatch[],
): Promise<CullSessionDto | null> {
  try {
    return normalizeCullSession(await ipc<unknown>("cull_decision_apply", { sessionId, decisions }));
  } catch {
    return null;
  }
}

/** 会话改名（cull_session_rename）；命令失败 false */
export async function cullSessionRename(id: number, name: string): Promise<boolean> {
  try {
    await ipc<void>("cull_session_rename", { sessionId: id, name });
    return true;
  } catch {
    return false;
  }
}

/** 丢弃会话（cull_session_discard：仅删会话+决定表，不动库内照片/标记） */
export async function cullSessionDiscard(id: number): Promise<boolean> {
  try {
    await ipc<void>("cull_session_discard", { sessionId: id });
    return true;
  } catch {
    return false;
  }
}

/** 完成会话并应用收尾映射（cull_session_finish）；失败/形状异常返回 null */
export async function cullSessionFinish(
  id: number,
  apply: CullFinishApply,
): Promise<CullFinishResult | null> {
  try {
    const raw = await ipc<unknown>("cull_session_finish", { sessionId: id, apply });
    if (raw === null || typeof raw !== "object") return null;
    const r = raw as Record<string, unknown>;
    const numOf = (v: unknown): number =>
      typeof v === "number" && Number.isFinite(v) ? v : 0;
    return {
      appliedFlag: numOf(r.appliedFlag),
      appliedRating: numOf(r.appliedRating),
      rejected: numOf(r.rejected),
    };
  } catch {
    return null;
  }
}

/** AI 挑图规则跑批（cull_ai_prescan）。apply=false 预览摘要 / true 写入预标记；
 *  失败/形状异常返回 null（调用方提示后端未连接，不改会话状态） */
export async function cullAiPrescan(
  sessionId: number,
  rules: CullAiRules,
  apply: boolean,
): Promise<CullPrescanDto | null> {
  try {
    const raw = await ipc<unknown>("cull_ai_prescan", { sessionId, rules, apply });
    if (raw === null || typeof raw !== "object") return null;
    const r = raw as Record<string, unknown>;
    // 后端每桶为 { count, assetIds } 对象（或历史扁平数字）——两种形状都归一
    const bucketOf = (v: unknown): { count: number; assetIds?: number[] } => {
      if (typeof v === "number" && Number.isFinite(v)) return { count: v };
      if (v !== null && typeof v === "object") {
        const b = v as Record<string, unknown>;
        const count =
          typeof b.count === "number" && Number.isFinite(b.count) ? b.count : 0;
        const ids = Array.isArray(b.assetIds)
          ? b.assetIds.filter(
              (x): x is number => typeof x === "number" && Number.isFinite(x),
            )
          : undefined;
        return { count, ...(ids !== undefined ? { assetIds: ids } : {}) };
      }
      return { count: 0 };
    };
    const accepted = bucketOf(r.suggestedAccepted);
    const rejected = bucketOf(r.suggestedRejected);
    const manual = bucketOf(r.skippedManual);
    const exempt = bucketOf(r.exemptedGroup);
    const assetIds = [...(accepted.assetIds ?? []), ...(rejected.assetIds ?? [])];
    return {
      suggestedAccepted: accepted.count,
      suggestedRejected: rejected.count,
      skippedManual: manual.count,
      exemptedGroup: exempt.count,
      ...(assetIds.length > 0 ? { assetIds } : {}),
    };
  } catch {
    return null;
  }
}
