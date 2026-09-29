import { ipc } from "../index";
import { ipcList } from "../read";
import {
  type AlbumCreateResult,
  type AlbumDto,
  type AlbumOpResult,
  type AlbumSubgroupDto,
  type AssetDto,
  type AssetFilters,
  type LrStagingResult,
} from "./types";
import { INVOKE_UNAVAILABLE_PATTERN } from "./errors";

/** 相册清单（album_list）；命令失败/非数组回退 []——UI 自然降级空态 */
export async function albumList(): Promise<AlbumDto[]> {
  try {
    const list = await ipc<AlbumDto[] | null>("album_list");
    if (!Array.isArray(list)) return [];
    return list.filter(
      (a): a is AlbumDto =>
        typeof a?.id === "number" && Number.isFinite(a.id) && typeof a?.name === "string",
    );
  } catch {
    return [];
  }
}

/** 新建相册（重名等业务错误由后端透传，调用方行内提示） */
export async function albumCreate(name: string): Promise<AlbumCreateResult> {
  try {
    const album = await ipc<AlbumDto>("album_create", { name });
    if (album === null || typeof album !== "object" || typeof album.id !== "number") {
      return { ok: false, error: null };
    }
    return { ok: true, album };
  } catch (err) {
    const message = err instanceof Error ? err.message : typeof err === "string" ? err : "";
    if (!message || INVOKE_UNAVAILABLE_PATTERN.test(message)) return { ok: false, error: null };
    return { ok: false, error: message };
  }
}

/** 重命名相册（重名等业务错误透传） */
export async function albumRename(id: number, name: string): Promise<AlbumOpResult> {
  try {
    await ipc<void>("album_rename", { id, name });
    return { ok: true };
  } catch (err) {
    const message = err instanceof Error ? err.message : typeof err === "string" ? err : "";
    if (!message || INVOKE_UNAVAILABLE_PATTERN.test(message)) return { ok: false, error: null };
    return { ok: false, error: message };
  }
}

/** 删除相册（仅删引用组，照片保留在图库）；失败 false（调用方按需提示） */
export async function albumDelete(id: number): Promise<boolean> {
  try {
    await ipc<void>("album_delete", { id });
    return true;
  } catch {
    return false;
  }
}

/** 设置相册封面（assetId=null 清除封面回退首张）；失败 false */
export async function albumCoverSet(id: number, assetId: number | null): Promise<boolean> {
  try {
    await ipc<void>("album_cover_set", { id, assetId });
    return true;
  } catch {
    return false;
  }
}

/** 批量加入相册，返回实际新增数（已引用幂等跳过）；失败 null（调用方提示加入失败） */
export async function albumAddAssets(
  id: number,
  assetIds: number[],
  subgroup?: string,
): Promise<number | null> {
  try {
    const payload: Record<string, unknown> = { id, assetIds };
    const trimmed = subgroup?.trim();
    if (trimmed) payload.subgroup = trimmed;
    const added = await ipc<number>("album_add_assets", payload);
    return typeof added === "number" && Number.isFinite(added) ? added : null;
  } catch {
    return null;
  }
}

/** 从相册移除引用（仅删引用，照片保留在图库）；失败 false */
export async function albumRemoveAssets(id: number, assetIds: number[]): Promise<boolean> {
  try {
    await ipc<void>("album_remove_assets", { id, assetIds });
    return true;
  } catch {
    return false;
  }
}

/** 更改相册文件夹名（album_dir_rename，B1 追加包契约）：只改磁盘相册主目录名，
 *  显示名不动。重名/非法名等业务错误透传，调用方行内提示。 */
export async function albumDirRename(id: number, dirName: string): Promise<AlbumOpResult> {
  try {
    await ipc<void>("album_dir_rename", { id, dirName });
    return { ok: true };
  } catch (err) {
    const message = err instanceof Error ? err.message : typeof err === "string" ? err : "";
    if (!message || INVOKE_UNAVAILABLE_PATTERN.test(message)) return { ok: false, error: null };
    return { ok: false, error: message };
  }
}

/**
 * 归入相册（album_claim_assets，B1 追加包契约）：把日期根下未归册的照片物理
 * 挪入相册主目录（相册名/YYYY/MM-DD/）。已在别册主目录的资产后端整批报错——
 * 那部分只能引用加入；Err 原文透传给调用方展示。返回实际归入数；命令失败 null。
 */
export async function albumClaimAssets(
  id: number,
  assetIds: number[],
  subgroup?: string,
): Promise<number | null> {
  try {
    const payload: Record<string, unknown> = { id, assetIds };
    const trimmed = subgroup?.trim();
    if (trimmed) payload.subgroup = trimmed;
    const moved = await ipc<number>("album_claim_assets", payload);
    return typeof moved === "number" && Number.isFinite(moved) ? moved : assetIds.length;
  } catch (err) {
    const message = err instanceof Error ? err.message : typeof err === "string" ? err : "";
    if (!message || INVOKE_UNAVAILABLE_PATTERN.test(message)) return null;
    // 业务错误（如「已在其他相册主目录」）以 Err 原文抛出，调用方行内/浮层提示
    throw new Error(message);
  }
}

/** 生成 LR 暂存夹（lr_staging_create；name 省略由后端按时间戳命名）。
 *  命令失败/负载异常返回 null（调用方提示失败）。 */
export async function lrStagingCreate(assetIds: number[], name?: string): Promise<LrStagingResult | null> {
  try {
    const payload: Record<string, unknown> = { assetIds };
    if (name !== undefined && name.trim() !== "") payload.name = name.trim();
    const raw = await ipc<unknown>("lr_staging_create", payload);
    if (raw === null || typeof raw !== "object") return null;
    const r = raw as Partial<LrStagingResult>;
    if (typeof r.dir !== "string" || r.dir === "") return null;
    const numOf = (v: unknown): number => (typeof v === "number" && Number.isFinite(v) ? v : 0);
    return { dir: r.dir, created: numOf(r.created), hardlinked: numOf(r.hardlinked), copied: numOf(r.copied) };
  } catch {
    return null;
  }
}

/** 相册内照片分页（album_assets_page；keyset 与 assets_page 同风格：afterId=上一页
 *  末条 id、首页 0；按拍摄时间排序由后端保证；filters 透传可选筛选）。
 *  失败/非数组回退 []。 */
export async function albumAssetsPage(
  id: number,
  afterId: number,
  limit: number,
  filters?: AssetFilters,
): Promise<AssetDto[]> {
  const payload: Record<string, unknown> = { id, afterId, limit };
  if (filters && Object.keys(filters).length > 0) payload.filters = filters;
  return ipcList<AssetDto>("album_assets_page", payload);
}

/** 相册子分组清单（album_subgroups；命令参数名是 id——与 album_assets_page 同款）；
 *  失败/非数组/形状异常回退 [] */
export async function albumSubgroups(albumId: number): Promise<AlbumSubgroupDto[]> {
  try {
    const list = await ipc<unknown>("album_subgroups", { id: albumId });
    if (!Array.isArray(list)) return [];
    return list.filter(
      (g): g is AlbumSubgroupDto =>
        g !== null && typeof g === "object" && typeof (g as AlbumSubgroupDto).name === "string",
    );
  } catch {
    return [];
  }
}

/** 相册内挪子分组（album_item_move_subgroup；命令参数名是 id）：纯引用移动，
 *  subgroup=null 移回根。失败 false（调用方提示）。 */
export async function albumItemMoveSubgroup(
  albumId: number,
  assetIds: number[],
  subgroup: string | null,
): Promise<boolean> {
  try {
    await ipc<void>("album_item_move_subgroup", { id: albumId, assetIds, subgroup });
    return true;
  } catch {
    return false;
  }
}

/** 资产所属相册反查（asset_albums；查看器详情「所属相册」行）。
 *  失败/非数组回退 []——行级降级不阻塞详情面板 */
export async function assetAlbums(assetId: number): Promise<AlbumDto[]> {
  try {
    const list = await ipc<AlbumDto[] | null>("asset_albums", { assetId });
    return Array.isArray(list)
      ? list.filter((a): a is AlbumDto => typeof a?.id === "number" && typeof a?.name === "string")
      : [];
  } catch {
    return [];
  }
}
