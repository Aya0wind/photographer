import type { AssetDto, AssetGroupDate } from "@/ipc/api";

/**
 * 画廊会话快照（真机修复 2026-09-19）：GalleryPage 每次挂载从零拉数据，
 * 切页回来全体资产回到加载态。把「已加载资产 + 日期分组 + hasMore +
 * 滚动位置」缓存在模块级单例：重挂载立即渲染缓存内容（缩略图本身走
 * 磁盘缓存，毫秒级重现），后台静默 revalidate——首页与缓存前缀一致
 * （库未变）则保留已加载全量，首页变化（新导入/删除）才重置。
 * 会话级缓存：进程重启即失效（重启后首屏本来就该重拉）。
 */

export interface GallerySnapshot {
  assets: AssetDto[];
  dates: AssetGroupDate[];
  /** 补页是否未尽（决定重挂载后哨兵是否继续补页） */
  hasMore: boolean;
  /** 视口滚动位置（重挂载恢复） */
  scrollTop: number;
  savedAt: number;
}

let snapshot: GallerySnapshot | null = null;

export function gallerySnapshot(): GallerySnapshot | null {
  return snapshot;
}

export function saveGallerySnapshot(next: GallerySnapshot): void {
  snapshot = next;
}

/** 清空会话快照（切库时必须调用——快照不区分库，跨库沿用=看到上一个库的照片） */
export function clearGallerySnapshot(): void {
  snapshot = null;
}

/** 仅测试用 */
export function clearGallerySnapshotForTests(): void {
  clearGallerySnapshot();
}
