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
 * 导入目标根 = photoRoot + 导入子目录（分隔符统一 Windows 风格）。
 * subdir 为空或仅分隔符时退回 photoRoot 本身；两侧多余分隔符会被裁剪。
 */
export function importRootOf(photoRoot: string, subdir: string = DEFAULT_IMPORT_SUBDIR): string {
  const root = photoRoot.replace(/[\\/]+$/, "");
  const sub = subdir.replace(/^[\\/]+|[\\/]+$/g, "");
  if (!sub) return root;
  return `${root}\\${sub}`;
}

/** 模板令牌白名单（与 Rust 侧 import/templates.rs 保持一致，M1 落地） */
export const TEMPLATE_TOKENS = [
  "YYYY", "YY", "MM", "DD", "MM-DD", "HH", "mm", "ss",
  "原文件名", "原目录", "相机", "镜头",
] as const;

/** 目录模板预设（key 供下拉/分段 UI，value 即模板目录段；{原文件名} 由调用方按需拼接） */
export const TEMPLATE_PRESETS = [
  { key: "ymd", value: "{YYYY}/{MM-DD}" },
  { key: "ym", value: "{YYYY}/{MM}" },
  { key: "orig", value: "{原目录}" },
] as const;

/** 模板是否匹配某预设（去掉尾部 /{原文件名} 后比对）；不匹配返回 "custom" */
export function matchTemplatePreset(template: string): string {
  const dirPart = template.replace(/\/?\{原文件名\}\s*$/, "");
  const hit = TEMPLATE_PRESETS.find((p) => p.value === dirPart);
  return hit ? hit.key : "custom";
}

/** 找出模板中的未知令牌（用于向导即时校验） */
export function unknownTokens(template: string): string[] {
  const found = template.match(/\{([^{}]*)\}/g) ?? [];
  const allowed = new Set<string>(TEMPLATE_TOKENS);
  return found.map((t) => t.slice(1, -1)).filter((t) => !allowed.has(t));
}

const SAMPLE_VALUES: Record<string, string> = {
  YYYY: "2026", YY: "26", MM: "09", DD: "18", "MM-DD": "09-18", HH: "14", mm: "30", ss: "05",
  原文件名: "IMG_0001.CR3", 原目录: "DCIM\\100CANON", 相机: "EOS_R5", 镜头: "24-70",
};

/** 用示例值渲染目录模板预览：photoRoot + 模板目录 + 文件名（分隔符统一为 Windows 风格） */
export function previewTemplate(template: string, photoRoot: string): string {
  const dir = template
    .replace(/\{([^{}]*)\}/g, (m, token: string) => SAMPLE_VALUES[token] ?? m)
    .replace(/\//g, "\\");
  const file = /\{原文件名\}/.test(template) ? "" : "\\IMG_0001.CR3";
  const root = photoRoot.replace(/[\\/]+$/, "");
  return `${root}\\${dir}${file}`;
}
