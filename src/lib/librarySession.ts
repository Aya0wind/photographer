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

/**
 * 库内容缓存失效（photoLibrariesChanged 事件用，2026-10-09 删库联动）：
 * 删除照片库只发该事件（不换库），但库内资产已整体消失，且 SQLite 无
 * AUTOINCREMENT——之后新导入会复用旧 assetId，按 id 键的三张内存表
 * （画廊快照/缩略图缓存/浏览打点）不整表清=张冠李戴的残留。比
 * resetLibrarySession 轻：不动索引状态与导入任务（同库历史仍有效）。
 * thumbPipeline 自身的事件分发会再清一次缩略图缓存（重复清无害）。
 */
export function invalidateLibraryContentCaches(): void {
  clearGallerySnapshot();
  resetThumbPipeline();
  resetViewMark();
}
