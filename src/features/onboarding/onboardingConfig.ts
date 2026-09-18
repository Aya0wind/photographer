/**
 * 首次引导的本机默认值（达芬奇式库模型，spec §5.11）：
 * 引导创建首个库——名称"主库"、数据库目录 I:\SmartPhoto\主库（自包含可迁移）、
 * 照片存储目录 Y:\照片。三者均可在向导中修改。
 */
export const SUGGESTED_LIBRARY_NAME = "主库";
export const SUGGESTED_DB_DIR = "I:\\SmartPhoto\\主库";
export const SUGGESTED_PHOTO_ROOT = "Y:\\照片";

/** 模板令牌白名单（与 Rust 侧 import/templates.rs 保持一致，M1 落地） */
export const TEMPLATE_TOKENS = [
  "YYYY", "YY", "MM", "DD", "MM-DD", "HH", "mm", "ss",
  "原文件名", "原目录", "相机", "镜头",
] as const;

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
