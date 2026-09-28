/**
 * 首次引导的本机默认值（达芬奇式库模型，spec §5.11）：
 * 引导创建首个库——名称"主库"、数据库目录 I:\SmartPhoto\主库（自包含可迁移）、
 * 照片存储目录 Y:\照片。三者均可在向导中修改。
 */
export const SUGGESTED_LIBRARY_NAME = "主库";
export const SUGGESTED_DB_DIR = "I:\\SmartPhoto\\主库";
export const SUGGESTED_PHOTO_ROOT = "Y:\\照片";

/**
 * 固定目录布局公式（dirTemplate/importSubdir 配置退役，2026-09-28 定案）：
 * 照片根/{相册创建YYYY}/{相册创建MM}/{相册目录名}/，相册内平铺不按日期分层
 * （应用内按拍摄日分组）。导入目标根 = photoRoot 本身（不再有收纳子目录）。
 * 用于向导/引导的只读展示，落位由后端固定执行。
 */
export const FIXED_ALBUM_LAYOUT = "{相册创建年}\\{相册创建月}\\{相册目录}";

/** 导入目标根 = photoRoot（尾部多余分隔符裁剪；布局固定后无子目录前缀）。 */
export function importRootOf(photoRoot: string): string {
  return photoRoot.replace(/[\\/]+$/, "");
}
