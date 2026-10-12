// --- 侧栏计数本地刷新总线 ------------------------------------------------------------
// 先例：photographer:tags-changed（smartTags.ts → AlbumsPages）。后端无事件的
// 本地变更点（回收站彻底删除/恢复、相册增删改名、人物删除）成功后 dispatch，
// 侧栏监听并入 debounced 重拉 sidebar_counts/people_list——与 app://event
// 事件集共用同一条防抖拉取管道。

/** window CustomEvent 事件名 */
export const COUNTS_REFRESH_EVENT = "photographer:counts-refresh";

/** 操作成功后通知侧栏重拉计数（幂等；侧栏侧 ~400ms 防抖合并连续通知） */
export function notifyCountsRefresh(): void {
  window.dispatchEvent(new Event(COUNTS_REFRESH_EVENT));
}
