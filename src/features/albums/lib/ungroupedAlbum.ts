import type { AlbumDto } from "@/ipc/api";

/**
 * 系统保底相册「未分组」（规格修订，按名幂等由后端自动创建）：
 * - 每张照片都有主相册；「未分组」= 未真正归类的弱语义
 * - 后端规则：禁删（album_delete 报错）、禁改名（album_rename 报错）、
 *   允许改相册文件夹名（album_dir_rename）
 * - 前端以 name 精确匹配识别（后端按名幂等保证唯一）
 */

export const UNGROUPED_ALBUM_NAME = "未分组";

export function isUngroupedAlbum(album: Pick<AlbumDto, "name" | "dirName">): boolean {
  return album.name === UNGROUPED_ALBUM_NAME;
}
