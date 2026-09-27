import { useAiStore } from "@/stores/aiStore";
import { useImportStore } from "@/stores/importStore";
import { clearGallerySnapshot } from "@/features/gallery/lib/galleryCache";
import { resetThumbPipeline } from "@/features/gallery/lib/thumbPipeline";
import { resetViewMark } from "@/features/gallery/lib/viewMark";

/**
 * 库会话重置（打开/切换库时调用）。应用内的内存态几乎都按库私有：
 * - 画廊快照：不区分库，跨库沿用=看到上一个库的照片（真机 2026-09-27 事故）
 * - 缩略图管线缓存：键是 assetId+size，不同库 assetId 撞号=张冠李戴的图
 * - 浏览打点去抖表：同款撞号问题
 * - 索引状态/进度、导入任务/历史/失败清单：按库私有
 * 模型清单与下载进度是应用级（%APPDATA% 全局），不在此清。
 */
export function resetLibrarySession(): void {
  clearGallerySnapshot();
  resetThumbPipeline();
  resetViewMark();
  useAiStore.getState().resetLibrarySession();
  useImportStore.getState().resetLibrarySession();
}
