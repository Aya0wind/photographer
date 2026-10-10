import type { RegionCacheRow } from "@/ipc/api/map";

/**
 * 地区树路径工具（regions-cache.json 扁平 id/parent 结构的读侧）：
 * 筛选面板级联选择器与 /map/photos 面包屑共用——按 parent 链上溯拼
 * 「中国 / 浙江省 / 杭州市」完整路径；缓存未就绪（null）由调用方回退只显 id。
 */

/** 按 parent 链上溯拼完整路径（" / " 分隔）；id 不在树内 / 树异常返回 null */
export function regionPathOf(rows: RegionCacheRow[], id: number): string | null {
  const byId = new Map(rows.map((r) => [r.id, r]));
  const start = byId.get(id);
  if (!start) return null;
  const names: string[] = [start.name];
  const seen = new Set<number>([id]);
  let current = start;
  // 防御：脏缓存出现 parent 环时按已访问截断，不死循环
  while (current.parent !== null) {
    const parent = byId.get(current.parent);
    if (!parent || seen.has(parent.id)) break;
    names.unshift(parent.name);
    seen.add(parent.id);
    current = parent;
  }
  return names.join(" / ");
}
