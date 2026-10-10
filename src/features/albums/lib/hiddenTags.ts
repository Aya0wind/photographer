/**
 * 智能相册标签可见性（设置页画廊 tab）：localStorage 简化存储（不走 settings）。
 * 记录「已隐藏标签」数组；智能相册页过滤后渲染。
 */

export const HIDDEN_ALBUM_TAGS_KEY = "photographer.albums.hiddenTags";

type StorageLike = Pick<Storage, "getItem" | "setItem">;

/** 已隐藏标签清单；缺失/损坏静默回退 [] */
export function loadHiddenTags(storage: StorageLike = localStorage): string[] {
  try {
    const raw = storage.getItem(HIDDEN_ALBUM_TAGS_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((t): t is string => typeof t === "string");
  } catch {
    return [];
  }
}

export function saveHiddenTags(tags: string[], storage: StorageLike = localStorage): void {
  try {
    storage.setItem(HIDDEN_ALBUM_TAGS_KEY, JSON.stringify(tags));
  } catch {
    // 隐私模式等：仅本次会话生效
  }
}

/** 切换某标签可见性并落盘；返回新的已隐藏清单 */
export function toggleHiddenTag(tag: string, storage: StorageLike = localStorage): string[] {
  const prev = loadHiddenTags(storage);
  const next = prev.includes(tag) ? prev.filter((t) => t !== tag) : [...prev, tag];
  saveHiddenTags(next, storage);
  return next;
}
