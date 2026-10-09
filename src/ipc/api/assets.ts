import { ipc } from "../index";
import { ipcList } from "../read";
import {
  type AssetCameraCount,
  type AssetDetailDto,
  type AssetDto,
  type AssetFilters,
  type AssetFormatCount,
  type AssetGroupDate,
  type AssetLensCount,
  type BurstStats,
  type DuplicateGroupDto,
  type DuplicateKind,
  type GearFocalBucket,
  type GearLabelBucket,
  type GearNameCount,
  type GearStats,
  type ThumbGetResult,
} from "./types";

/** 取后端缓存缩略图文件路径（JPG/PNG 等可生成；RAW 无预览时返回 null）；命令失败静默 null。
 *  @param size 期望边长（px），如 256；实际以缓存档位就近为准 */
export async function thumbGet(path: string, size: number): Promise<string | null> {
  try {
    return await ipc<string | null>("thumb_get_by_path", { path, size });
  } catch {
    return null;
  }
}

// --- M3 画廊命令封装 --------------------------------------------------------------

/** 画廊/搜索分页（keyset：afterId=上一页最后一条资产 id，首页传 0；capturedAt DESC，
 *  NULL capturedAt 沉底；分页与页面日期分组采用相同顺序）。
 *  filters 平铺为 camelCase 负载字段；失败/非数组回退 []。 */
export async function assetsPage(
  afterId: number,
  limit: number,
  filters?: AssetFilters,
): Promise<AssetDto[]> {
  return ipcList<AssetDto>(
      "assets_page",
      filters && Object.keys(filters).length > 0 ? { afterId, limit, filters } : { afterId, limit },
    );
}

/** 当前条件的真实匹配总数，独立于已加载分页。 */
export async function assetsCount(filters?: AssetFilters): Promise<number | null> {
  try {
    const count = await ipc<number>("assets_count", filters ? { filters } : {});
    return typeof count === "number" && Number.isFinite(count) ? count : null;
  } catch {
    return null;
  }
}

/** 完整日期目录，支持当前筛选与相册范围；未知日期在末尾。 */
export async function assetGroupDates(filters?: AssetFilters): Promise<AssetGroupDate[]> {
  return ipcList<AssetGroupDate>("asset_group_dates", filters ? { filters } : undefined);
}

/** Direct date navigation; before=true returns the nearest newer page in display order. */
export async function assetsSeek(anchorId: number, limit: number, filters?: AssetFilters, before = false): Promise<AssetDto[]> {
  return ipc<AssetDto[]>("assets_seek", { anchorId, limit, filters, before });
}

/** 库内相机型号清单（搜索页相机勾选；按 count 降序）；失败/非数组回退 [] */
export async function cameraList(): Promise<AssetCameraCount[]> {
  return ipcList<AssetCameraCount>("camera_list");
}

/** 库内镜头清单（搜索页镜头勾选；按 count 降序）；失败/非数组回退 [] */
export async function lensList(): Promise<AssetLensCount[]> {
  return ipcList<AssetLensCount>("lens_list");
}

/** 库内文件格式清单（搜索页格式勾选，如 NEF/ARW/JPG）；失败/非数组回退 [] */
export async function formatList(): Promise<AssetFormatCount[]> {
  return ipcList<AssetFormatCount>("format_list");
}

/** 最近添加的资产（recent_assets；created_at DESC keyset：afterId=上一页最后一条 id，
 *  首页传 0）。后端就绪前命令失败/非数组回退 []——UI 自然降级空态。
 *  注：M4.5 改向后「最近浏览」页走 recentViewed；本封装保留（IPC 仍存在）。 */
export async function recentAssets(afterId: number, limit: number): Promise<AssetDto[]> {
  return ipcList<AssetDto>("recent_assets", { afterId, limit });
}

// --- 最近浏览（M4.5：查看器打开/切图打点，最近浏览页数据源） ---------------------------

/** 最近浏览的资产（recent_viewed；按最后浏览时间 DESC，同资产取最新一次，上限 200）。
 *  后端就绪前命令失败/非数组回退 []——UI 自然降级空态。 */
export async function recentViewed(limit: number): Promise<AssetDto[]> {
  return ipcList<AssetDto>("recent_viewed", { limit });
}

/** 连拍分组统计快照；命令失败/负载异常静默 null */
export async function burstStats(): Promise<BurstStats | null> {
  try {
    const stats = await ipc<BurstStats | null>("burst_stats");
    if (stats === null || typeof stats !== "object") return null;
    const s = stats as Partial<BurstStats>;
    return typeof s.groups === "number" && typeof s.photosInBursts === "number"
      ? (stats as BurstStats)
      : null;
  } catch {
    return null;
  }
}

// --- 那年今天 / 器材统计（M7） ---------------------------------------------------------

/** 那年今天：历年同月日资产（on_this_day；今天无历史为 []）。
 *  前端按 capturedAt 年份归块（降序）；失败/非数组回退 []——UI 自然降级空态。 */
export async function onThisDay(): Promise<AssetDto[]> {
  return ipcList<AssetDto>("on_this_day");
}

/** 负载形状校验：六个字段必须是数组（桶项内容浅校验 count 为数） */
function isGearStats(value: unknown): value is GearStats {
  if (typeof value !== "object" || value === null) return false;
  const s = value as Partial<Record<keyof GearStats, unknown>>;
  const arrays: Array<[unknown, (item: unknown) => boolean]> = [
    [s.cameras, (i) => typeof (i as GearNameCount)?.name === "string" && typeof (i as GearNameCount)?.count === "number"],
    [s.lenses, (i) => typeof (i as GearNameCount)?.name === "string" && typeof (i as GearNameCount)?.count === "number"],
    [s.focalBuckets, (i) => typeof (i as GearFocalBucket)?.label === "string" && typeof (i as GearFocalBucket)?.count === "number"],
    [s.isoBuckets, (i) => typeof (i as GearLabelBucket)?.label === "string" && typeof (i as GearLabelBucket)?.count === "number"],
    [s.apertureBuckets, (i) => typeof (i as GearLabelBucket)?.label === "string" && typeof (i as GearLabelBucket)?.count === "number"],
    [s.shutterBuckets, (i) => typeof (i as GearLabelBucket)?.label === "string" && typeof (i as GearLabelBucket)?.count === "number"],
  ];
  return arrays.every(([field, itemOk]) => Array.isArray(field) && field.every(itemOk));
}

/** 器材统计快照；命令失败/负载形状异常静默 null */
export async function gearStats(): Promise<GearStats | null> {
  try {
    const stats = await ipc<GearStats | null>("gear_stats");
    return isGearStats(stats) ? stats : null;
  } catch {
    return null;
  }
}

/**
 * 重复组列表（duplicates_list）。游标语义（对齐 src-tauri duplicates.rs）：
 * after = 上一页末组**序号**（0 基组偏移，skip 计数——不是组 id），首页传
 * 0/省略；limit 限组数（后端默认 50、上限 100）。组序：组大小降序。
 * 失败/非数组回退 []，组项形状异常剔除。
 */
export async function duplicatesList(
  kind: DuplicateKind,
  after = 0,
  limit = 20,
): Promise<DuplicateGroupDto[]> {
  try {
    const list = await ipc<DuplicateGroupDto[] | null>("duplicates_list", { kind, after, limit });
    if (!Array.isArray(list)) return [];
    return list.filter(
      (g): g is DuplicateGroupDto =>
        typeof g?.kind === "string" && (g.kind === "exact" || g.kind === "similar") && Array.isArray(g.assets),
    );
  } catch {
    return [];
  }
}


/** 标记资产被浏览（asset_view_mark；查看器打开/切图时调用，fire-and-forget）。
 *  命令失败静默——浏览打点不阻塞查看。 */
export async function assetViewMark(assetId: number): Promise<void> {
  try {
    await ipc<void>("asset_view_mark", { assetId });
  } catch {
    // 静默
  }
}

/** 按 id 批量取资产（语义/智能相册结果回填 AssetDto 用）；失败/非数组回退 []。
 *  契约补充项（assets_by_ids，需后端 lane 实现；keyset 之外唯一的随机访问口）。 */
export async function assetsByIds(ids: number[]): Promise<AssetDto[]> {
  return ipcList<AssetDto>("assets_by_ids", { ids });
}

/** AI 选片分析负载归一（脏数据容错；非对象/字段缺失 → null/剔除） */
function normalizeAiAnalysis(value: unknown): AssetDetailDto["aiAnalysis"] {
  if (value === null || typeof value !== "object") return null;
  const r = value as Record<string, unknown>;
  const chOf = (v: unknown): NonNullable<AssetDetailDto["aiAnalysis"]>["eyes"] => {
    if (v === null || typeof v !== "object") return undefined;
    const c = v as Record<string, unknown>;
    if (typeof c.value !== "string" || (c.score != null && (typeof c.score !== "number" || !Number.isFinite(c.score)))) return undefined;
    const channel: NonNullable<NonNullable<AssetDetailDto["aiAnalysis"]>["eyes"]> = {
      value: c.value, score: typeof c.score === "number" ? c.score : null,
      modelVersion: typeof c.modelVersion === "string" ? c.modelVersion : undefined,
    };
    if (c.details && typeof c.details === "object") {
      const d = c.details as Record<string, unknown>;
      const finite = (n: unknown): number | null => typeof n === "number" && Number.isFinite(n) ? n : null;
      channel.details = {
        source: typeof d.source === "string" ? d.source : undefined,
        width: finite(d.width) ?? undefined, height: finite(d.height) ?? undefined,
        calibrated: d.calibrated === true, reason: typeof d.reason === "string" ? d.reason : null,
        regions: Array.isArray(d.regions) ? d.regions.flatMap((v: unknown) => {
          if (!v || typeof v !== "object") return [];
          const e = v as Record<string, unknown>;
          if (typeof e.kind !== "string" || typeof e.state !== "string" || finite(e.person) == null
            || !Array.isArray(e.bounds) || e.bounds.length !== 4 || e.bounds.some(n => finite(n) == null || n < 0 || n > 1)) return [];
          return [{ kind: e.kind, state: e.state, person: e.person as number, bounds: e.bounds as number[],
            side: typeof e.side === "string" ? e.side : null, reason: typeof e.reason === "string" ? e.reason : null,
            rawScore: finite(e.rawScore), auxiliaryEar: finite(e.auxiliaryEar) }];
        }) : [],
      };
    }
    return channel;
  };
  const out: NonNullable<AssetDetailDto["aiAnalysis"]> = {};
  const eyes = chOf(r.eyes), blur = chOf(r.blur);
  if (eyes !== undefined) out.eyes = eyes;
  if (blur !== undefined) out.blur = blur;
  return out;
}

/** 单资产全量元数据（查看器 EXIF 面板）；命令失败/不存在/负载异常返回 null。
 *  后端负载 → AssetDetailDto 归一：filename/size/createdAt + duplicate_count（顶层
 *  snake_case，未做 camelCase 重命名）→ dupCount；字段缺失容错（不透传 undefined），
 *  EXIF 扩展字段存在即带出（aperture/shutter 兼容 fNumber/exposure 别名；
 *  M4 新字段读 camelCase、snake_case 兜底）。 */
export async function assetDetail(id: number): Promise<AssetDetailDto | null> {
  try {
    const raw = await ipc<unknown>("asset_detail", { id });
    if (raw === null || typeof raw !== "object") return null;
    const r = raw as Record<string, unknown>;
    /** 字符串或有限数字 → 展示态字符串（光圈/快门/焦距双形态兼容） */
const strNumOf = (v: unknown): string | null =>
  typeof v === "number" && Number.isFinite(v)
    ? String(v)
    : typeof v === "string" && v.length > 0
      ? v
      : null;

const numOf = (v: unknown): number | null =>
      typeof v === "number" && Number.isFinite(v) ? v : null;
    const strOf = (v: unknown): string | null =>
      typeof v === "string" && v.length > 0 ? v : null;
    const kind = r.kind === "raw" ? "raw" : "photo";
    return {
      id: numOf(r.id) ?? 0,
      path: typeof r.path === "string" ? r.path : "",
      filename:
        typeof r.filename === "string" ? r.filename : typeof r.name === "string" ? r.name : "",
      size: numOf(r.size ?? r.sizeBytes) ?? 0,
      kind,
      capturedAt: strOf(r.capturedAt),
      camera: strOf(r.camera),
      lens: strOf(r.lens),
      createdAt: strOf(r.createdAt),
      dupCount: numOf(r.duplicate_count ?? r.duplicateCount) ?? 0,
      width: numOf(r.width),
      height: numOf(r.height),
      megapixels: numOf(r.megapixels),
      aspect: strOf(r.aspect),
      orientation: numOf(r.orientation),
      iso: numOf(r.iso),
      // 光圈/快门/焦距：展示态字符串（"2.8"/"1/250"/"59"），数字形态兼容转字符串
      aperture: strNumOf(r.aperture ?? r.fNumber),
      shutter: strNumOf(r.shutter ?? r.exposureTime ?? r.exposure),
      focalLength: strNumOf(r.focalLength),
      flash: strOf(r.flash),
      meteringMode: strOf(r.meteringMode ?? r.metering_mode),
      whiteBalance: strOf(r.whiteBalance ?? r.white_balance),
      exposureProgram: strOf(r.exposureProgram ?? r.exposure_program),
      software: strOf(r.software),
      artist: strOf(r.artist),
      gpsLat: numOf(r.gpsLat ?? r.gps_lat),
      gpsLon: numOf(r.gpsLon ?? r.gps_lon),
      format: strOf(r.format),
      rating: numOf(r.rating),
      flagged: typeof r.flagged === "boolean" ? r.flagged : null,
      aiAnalysis: normalizeAiAnalysis(r.aiAnalysis),
    };
  } catch {
    return null;
  }
}

/** 库内资产缩略图（按 assetId 取后端缓存文件绝对路径，调用方自行 convertFileSrc）。
 *  与向导的 thumbGet（按源文件路径，导入前预览用）是两个命令：本命令为 asset_thumb_get。
 *  size 为期望边长（画廊网格 240 / 查看器大图 1280；RAW 传 >2048 = 内嵌全幅直出档）。
 *  三态：ready=缓存命中（附路径）；pending=已入队后台生成（thumbnailReady 事件后
 *  重试即 ready）；unavailable=永久不可用（资产不存在 / thumb_state=2 / 不可解码 /
 *  连续失败 ≥3 次）。 */
export async function assetThumbGet(assetId: number, size: number): Promise<ThumbGetResult> {
  try {
    const raw = await ipc<unknown>("asset_thumb_get", { assetId, size });
    if (raw !== null && typeof raw === "object") {
      const r = raw as { status?: unknown; path?: unknown; cachedPath?: unknown };
      if (r.status === "ready" && typeof r.path === "string") {
        return { status: "ready", path: r.path };
      }
      if (r.status === "missing") {
        // cachedPath 可为 null（连历史缓存都没有）；空串归一为 null
        return {
          status: "missing",
          cachedPath: typeof r.cachedPath === "string" && r.cachedPath !== "" ? r.cachedPath : null,
        };
      }
      if (r.status === "pending" || r.status === "unavailable") {
        return { status: r.status };
      }
    }
    return { status: "unavailable" };
  } catch {
    return { status: "unavailable" };
  }
}

/** 资产评分（asset_rating_set；0=清除，1-5 星）。命令失败静默（乐观 UI 由调用方回滚） */
export async function assetRatingSet(assetId: number, rating: number): Promise<void> {
  try {
    await ipc<void>("asset_rating_set", { assetId, rating });
  } catch {
    // 静默
  }
}

/** 资产旗标（asset_flag_set；红旗标记/待整理）。命令失败静默 */
export async function assetFlagSet(assetId: number, flagged: boolean): Promise<void> {
  try {
    await ipc<void>("asset_flag_set", { assetId, flagged });
  } catch {
    // 静默
  }
}
