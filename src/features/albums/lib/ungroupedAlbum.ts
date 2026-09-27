import type { AlbumDto } from "@/ipc/api";

/**
 * 系统保底相册「未分组」（规格修订，按名幂等由后端自动创建）：
 * - 每张照片都有主相册；「未分组」= 未真正归类的弱语义
 * - 后端规则：禁删（album_delete 报错）、禁改名（album_rename 报错）、
 *   允许改相册文件夹名（album_dir_rename）
 * - 前端以 name 精确匹配识别（后端按名幂等保证唯一）
 */

export const UNGROUPED_ALBUM_NAME = "未分组";

/**
 * 子分组默认建议名（用户定案）：子分组是纯子文件夹，无内建语义；「原片」「成片」
 * 只是新建/移组时的常用命名建议——仅出现在 datalist 提示里，与既有名去重。
 */
export const DEFAULT_SUBGROUP_SUGGESTIONS: readonly string[] = ["原片", "成片"];

/** datalist 建议 = 默认建议名（未被占用的前置）+ 该相册现有子组名（保序去重） */
export function subgroupSuggestions(existing: readonly string[]): string[] {
  const names = existing.map((n) => n.trim());
  return [...DEFAULT_SUBGROUP_SUGGESTIONS.filter((s) => !names.includes(s)), ...names];
}

export function isUngroupedAlbum(album: Pick<AlbumDto, "name" | "dirName">): boolean {
  return album.name === UNGROUPED_ALBUM_NAME;
}
