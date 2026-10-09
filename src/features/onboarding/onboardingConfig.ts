/**
 * 首次引导默认值与落盘布局公式（2026-10-09 单库多照片库定案）。
 */
import i18n from "@/i18n";
import { directoryPreview, trimDirectoryEnd } from "@/lib/filesystemPaths";
export const SUGGESTED_LIBRARY_NAME = "主库";
export function suggestedLibraryName(): string {
  return i18n.t("library.defaultName", { defaultValue: SUGGESTED_LIBRARY_NAME });
}

/**
 * 固定落盘布局公式（纯时间，物理层无相册/子组维度）：
 * `{照片库}/{拍摄年}/{拍摄月}/{原文件名}`（EXIF 时间，缺失回退文件时间）；
 * 重名走现有 rename 策略，边车与本体同名跟随。用于向导只读展示，
 * 落位由后端固定执行。
 */
export const FIXED_TIME_LAYOUT = "{拍摄年}\\{拍摄月}\\{原文件名}";

/** 导入目标根 = 照片库 root（尾部多余分隔符裁剪；布局固定后无子目录前缀）。 */
export function importRootOf(rootPath: string): string {
  return trimDirectoryEnd(rootPath);
}

/** 纯时间布局的目标路径预览（向导/双目的地第二路共用同一公式）。 */
export function timeLayoutPreview(rootPath: string): string {
  return directoryPreview(importRootOf(rootPath), "{拍摄年}", "{拍摄月}", "{原文件名}");
}
