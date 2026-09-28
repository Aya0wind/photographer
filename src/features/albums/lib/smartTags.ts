import { searchSemantic, type SemanticHit } from "@/ipc/api";
import i18n from "@/i18n";
import { useSettingsStore } from "@/stores/settingsStore";
import { loadHiddenTags } from "./hiddenTags";

export const DEFAULT_SMART_TAGS = [
  "人像", "风景", "夜景", "美食", "建筑", "街拍", "动物", "花卉", "雪", "日落", "黑白",
  "天空云彩", "公园", "山", "湖泊", "海洋", "海滩", "森林", "桥", "河流", "日出日落",
  "广场", "街道", "花", "烟花", "猫", "狗", "鸟", "合影", "儿童", "城市", "乡村",
  "道路", "车", "自行车", "飞机", "火车", "船", "雨", "雾",
] as const;

/** 只翻译内置标签的显示名，检索词和缓存键保持稳定，用户自定义名称原样显示。 */
export function smartTagLabel(tag: string): string {
  const index = (DEFAULT_SMART_TAGS as readonly string[]).indexOf(tag);
  return index < 0 ? tag : i18n.t(`albums.smartTag.${index}`, { defaultValue: tag });
}

const TAGS_KEY = "smartphoto.albums.tags.v2";
const INDEX_KEY = "smartphoto.albums.tagIndex.v2";

type TagRecord = { cover: number | null; hits?: SemanticHit[] };
type TagIndex = Record<string, TagRecord>;

function libraryKey(): string {
  return useSettingsStore.getState().settings.activeLibraryId ?? "default";
}

function loadIndex(): Record<string, TagIndex> {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(INDEX_KEY) ?? "{}");
    return parsed && typeof parsed === "object" && !Array.isArray(parsed)
      ? parsed as Record<string, TagIndex> : {};
  } catch {
    return {};
  }
}

export function loadSmartTags(): string[] {
  try {
    const raw = localStorage.getItem(TAGS_KEY);
    if (raw !== null) {
      const parsed: unknown = JSON.parse(raw);
      if (Array.isArray(parsed)) return [...new Set(parsed.filter((tag): tag is string => typeof tag === "string" && tag.trim().length > 0))];
    }
  } catch { /* malformed storage: use defaults */ }
  const hidden = loadHiddenTags();
  return DEFAULT_SMART_TAGS.filter((tag) => !hidden.includes(tag));
}

export function saveSmartTags(tags: string[]): void {
  localStorage.setItem(TAGS_KEY, JSON.stringify(tags));
  window.dispatchEvent(new Event("smartphoto:tags-changed"));
}

export function indexedTagCover(tag: string): number | null | undefined {
  return loadIndex()[libraryKey()]?.[tag]?.cover;
}

export function rememberTagCover(tag: string, assetId: number | null): void {
  const scope = libraryKey();
  const index = loadIndex();
  index[scope] = { ...index[scope], [tag]: { ...index[scope]?.[tag], cover: assetId } };
  localStorage.setItem(INDEX_KEY, JSON.stringify(index));
}

export function unindexedTags(tags: readonly string[]): string[] {
  const index = loadIndex()[libraryKey()] ?? {};
  return tags.filter((tag) => !Array.isArray(index[tag]?.hits));
}

/** Only new tags run a semantic query. The photo embedding index is shared by all tags. */
export async function indexNewTags(tags: readonly string[], onProgress?: (done: number, total: number) => void): Promise<void> {
  const pending = unindexedTags(tags);
  let done = 0;
  for (const tag of pending) {
    const hits = await searchSemantic(tag, 100);
    const scope = libraryKey();
    const index = loadIndex();
    index[scope] = { ...index[scope], [tag]: { cover: hits[0]?.assetId ?? null, hits } };
    localStorage.setItem(INDEX_KEY, JSON.stringify(index));
    onProgress?.(++done, pending.length);
  }
  window.dispatchEvent(new Event("smartphoto:tags-indexed"));
}
