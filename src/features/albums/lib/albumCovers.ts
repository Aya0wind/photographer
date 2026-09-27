import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";

import { albumAssetsPage, assetThumbGet, searchSemantic, type AlbumDto } from "@/ipc/api";

/**
 * 智能相册标签封面（/albums）：自动用「该标签语义搜索第一条命中」的缩略图做封面。
 * 进页面后台批量预取：固定并发池（≤3，避免与画廊缩略图管线抢 IPC）；
 * 无命中/命令失败/转换失败一律静默 null——调用方渲染占位。
 */

/** 封面缩略图名义边长（与画廊网格同 240 → 后端 snap 256 档，复用缓存） */
export const ALBUM_COVER_THUMB_SIZE = 240;
/** 封面预取并发上限 */
export const ALBUM_COVER_CONCURRENCY = 3;

/** 单标签封面 asset URL；任何一步失败/无命中 → null（占位） */
export async function fetchAlbumCover(tag: string): Promise<string | null> {
  try {
    const hits = await searchSemantic(tag, 1);
    const hit = hits[0];
    if (!hit) return null;
    const result = await assetThumbGet(hit.assetId, ALBUM_COVER_THUMB_SIZE);
    if (result.status !== "ready") return null; // pending 也当无封面（下轮事件自愈）
    return convertFileSrc(result.path) || null;
  } catch {
    return null;
  }
}

/**
 * 固定并发任务池：最多 limit 个任务同时执行，完成一个补位下一个（保持提交序）。
 * 单任务异常不会中断池（调用方约定任务自捕获；此处仍兜底继续补位）。
 */
export async function runTaskPool(
  tasks: Array<() => Promise<void>>,
  limit: number,
): Promise<void> {
  let next = 0;
  const workerCount = Math.max(1, Math.min(limit, tasks.length));
  const workers = Array.from({ length: workerCount }, async () => {
    while (next < tasks.length) {
      const task = tasks[next++];
      try {
        await task();
      } catch {
        // 任务自负责降级；兜底继续
      }
    }
  });
  await Promise.all(workers);
}

/** tag → 封面 asset URL；缺失键 = 尚未结算（占位），null = 已结算但无图（占位） */
export function useAlbumCovers(tags: readonly string[]): Record<string, string | null> {
  const [covers, setCovers] = useState<Record<string, string | null>>({});

  useEffect(() => {
    let cancelled = false;
    void runTaskPool(
      tags.map((tag) => async () => {
        const url = await fetchAlbumCover(tag);
        if (cancelled) return;
        setCovers((prev) => ({ ...prev, [tag]: url }));
      }),
      ALBUM_COVER_CONCURRENCY,
    );
    return () => {
      cancelled = true;
    };
  }, [tags]);

  return covers;
}

/**
 * 手工相册封面（/albums 手工相册区）：coverAssetId 有值 → 该资产缩略图；
 * 未指定 → 回退相册第一张（album_assets_page 首条）；空相册/失败 → null（占位图形）。
 * 与标签封面同一并发池语义（≤3），批量预取、静默降级。
 * @returns albumId → 封面 asset URL（缺失键 = 尚未结算）
 */
export function useManualAlbumCovers(albums: readonly AlbumDto[]): Record<number, string | null> {
  const [covers, setCovers] = useState<Record<number, string | null>>({});

  useEffect(() => {
    let cancelled = false;
    void runTaskPool(
      albums.map((album) => async () => {
        let assetId = album.coverAssetId;
        if (assetId === null && album.itemCount > 0) {
          const first = await albumAssetsPage(album.id, 0, 1);
          assetId = first[0]?.id ?? null;
        }
        if (assetId === null) {
          if (!cancelled) setCovers((prev) => ({ ...prev, [album.id]: null }));
          return;
        }
        const result = await assetThumbGet(assetId, ALBUM_COVER_THUMB_SIZE);
        const url =
          result.status === "ready" ? convertFileSrc(result.path) || null : null;
        if (!cancelled) setCovers((prev) => ({ ...prev, [album.id]: url }));
      }),
      ALBUM_COVER_CONCURRENCY,
    );
    return () => {
      cancelled = true;
    };
    // albums 引用每次渲染都可能变化（albumList 结果 state）；以 id+cover 串签名稳定化
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [albums.map((a) => `${a.id}:${a.coverAssetId ?? ""}:${a.itemCount}`).join(",")]);

  return covers;
}
