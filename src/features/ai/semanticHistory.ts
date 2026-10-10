/**
 * 语义查询历史（搜索页语义模式）：localStorage 持久化，最近 5 条。
 * - 记录：run 时调用 recordSemanticQuery——去重（已有同词移到最前）、截断上限、落盘
 * - 点击历史条目重搜（SearchPage chips）；存储损坏/隐私模式静默降级
 * 简化项：不走 settingsStore（纯 UI 偏好，localStorage 即可）。
 */

export const SEMANTIC_HISTORY_KEY = "photographer.semantic.history";
export const SEMANTIC_HISTORY_LIMIT = 5;

type StorageLike = Pick<Storage, "getItem" | "setItem" | "removeItem">;

function parseHistory(raw: string | null): string[] {
  if (!raw) return [];
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((q): q is string => typeof q === "string" && q.trim() !== "")
      .slice(0, SEMANTIC_HISTORY_LIMIT);
  } catch {
    return [];
  }
}

/** 读取历史（新→旧序）；缺失/损坏/越界均静默回退 [] */
export function loadSemanticHistory(storage: StorageLike = localStorage): string[] {
  try {
    return parseHistory(storage.getItem(SEMANTIC_HISTORY_KEY));
  } catch {
    return [];
  }
}

/** 记录一次查询：空白忽略；去重置顶 + 截断上限 + 落盘；返回新历史（新→旧） */
export function recordSemanticQuery(
  query: string,
  storage: StorageLike = localStorage,
): string[] {
  const trimmed = query.trim();
  if (trimmed === "") return loadSemanticHistory(storage);
  const prev = loadSemanticHistory(storage);
  const next = [trimmed, ...prev.filter((q) => q !== trimmed)].slice(0, SEMANTIC_HISTORY_LIMIT);
  try {
    storage.setItem(SEMANTIC_HISTORY_KEY, JSON.stringify(next));
  } catch {
    // 隐私模式/配额满：历史仅本次会话内有效
  }
  return next;
}

/** 清空历史（设置里暂无入口，供后续/测试用） */
export function clearSemanticHistory(storage: StorageLike = localStorage): void {
  try {
    storage.removeItem(SEMANTIC_HISTORY_KEY);
  } catch {
    // ignore
  }
}
