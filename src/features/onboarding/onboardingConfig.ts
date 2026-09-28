/**
 * 首次引导的本机默认值（达芬奇式库模型，spec §5.11）：
 * 引导创建首个库——名称"主库"、数据库目录 I:\SmartPhoto\主库（自包含可迁移）、
 * 照片存储目录 Y:\照片。三者均可在向导中修改。
 */
export const SUGGESTED_LIBRARY_NAME = "主库";
export const SUGGESTED_DB_DIR = "I:\\SmartPhoto\\主库";
export const SUGGESTED_PHOTO_ROOT = "Y:\\照片";

/**
 * 卡/相机导入的专用子目录名（spec §5.11 应用写入区）。
 * photoRoot 归用户管理（可预存内容）；应用只写入 photoRoot\SmartPhoto，
 * 用户也可手动把照片移入该区后触发重建索引。与 Rust 侧 ImportSettings.import_subdir 默认值一致。
 */
export const DEFAULT_IMPORT_SUBDIR = "SmartPhoto";

/**
 * 固定目录布局公式（dirTemplate 配置退役，2026-09-28 定案）：
 * 照片根/{相册创建YYYY}/{相册创建MM}/{相册目录名}/，相册内平铺不按日期分层
 * （应用内按拍摄日分组）。用于向导/引导的只读展示，落位由后端固定执行。
 */
export const FIXED_ALBUM_LAYOUT = "{相册创建年}\\{相册创建月}\\{相册目录}";

/**
 * 导入目标根 = photoRoot + 导入子目录（分隔符统一 Windows 风格）。
 * subdir 为空或仅分隔符时退回 photoRoot 本身；两侧多余分隔符会被裁剪。
 */
export function importRootOf(photoRoot: string, subdir: string = DEFAULT_IMPORT_SUBDIR): string {
  const root = photoRoot.replace(/[\\/]+$/, "");
  const sub = subdir.replace(/^[\\/]+|[\\/]+$/g, "");
  if (!sub) return root;
  return `${root}\\${sub}`;
}
