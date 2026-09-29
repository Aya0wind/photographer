/** IPC 数据契约，跨命令域共享；命令实现位于各自模块。 */

// --- 契约类型 ----------------------------------------------------------------

export type DeviceKind = "volume" | "mtp" | "folder";
export type FileKind = "photo" | "raw" | "other";

/** 原生平台已实现的能力；false 与「支持但当前未发现设备」分开。 */
export interface PlatformCapabilities {
  filesystemRoots: boolean;
  volumeDevices: boolean;
  portableDevices: boolean;
  hotplug: boolean;
  systemOpen: boolean;
  fileClipboard: boolean;
  fileReveal: boolean;
  documentUris: boolean;
}

export interface DeviceSnapshot {
  /** 空读卡器槽位可见，但不能扫描或导入。旧事件省略时按已插卡处理。 */
  mediaPresent?: boolean;
  scanStatus?: "scanning" | "ready" | "failed";
  scanError?: string | null;
  id: string;
  name: string;
  kind: DeviceKind;
  filesByKind: Record<FileKind, number>;
  bytesTotal: number;
  newFiles: number;
}

export type DuplicatePolicy = "skip" | "rename" | "ask";

/** 导入模式：copy=保留原文件（复制），move=入库后删除源（纳管已有照片） */
export type ImportMode = "copy" | "move";

/**
 * plan.dirTemplate 的固定占位值（dirTemplate 配置退役，2026-09-28 定案）：
 * 目录布局写死为时间/相册+平铺，不再可配置。Rust ImportPlan.dir_template 过渡期
 * 仍为必填（无 serde default），前端固定发送此占位；相册导入下后端 begin 阶段会把
 * dir_template 整体覆写为 `{相册创建YYYY}/{MM}/{dir_name}`，secondTarget 随之同公式。
 */
export const FIXED_PLAN_DIR_TEMPLATE = "{YYYY}/{MM-DD}";

export interface ImportPlan {
  sourceId: string;
  targetRoot: string;
  /** 过渡期兼容占位：恒为 FIXED_PLAN_DIR_TEMPLATE（相册导入下后端整体覆写，值不影响落位） */
  dirTemplate: string;
  nameTemplate: string;
  duplicatePolicy: DuplicatePolicy;
  skipImported: boolean;
  streams: number;
  /** 缺省 copy（Rust 侧默认）；move 时入库后删除源文件 */
  mode: ImportMode;
  /** 双目的地（可选）：一次读取同时复制到第二位置；落位与主目的地相同
   *  （第二根目录 + 同一时间/相册公式；dirTemplate 过渡期必填，恒为
   *  FIXED_PLAN_DIR_TEMPLATE，后端 engine 覆写后两路一致）。
   *  后端约束：move + secondTarget 会被拒绝（前端互斥保证不发出）。 */
  secondTarget?: { targetRoot: string; dirTemplate: string };
  /** 本次导入的文件清单（rel_path 列表）——向导勾选结果，引擎只导入集合内的文件；
   *  省略 = 全部（历史计划兼容）。 */
  include?: string[];
  /** 导入完成后把新入库照片加入该相册（向导「添加到相册」步骤；省略 = 不加入） */
  albumId?: number;
  /** 相册子分组名（B4 子分组模型；省略 = 相册根；后端按名幂等建层） */
  albumSubgroup?: string;
}

export type JobStatus = "running" | "paused" | "done" | "cancelled" | "failed";
export type LogLevel = "error" | "warn" | "info";

export interface JobRow {
  id: number;
  kind: string;
  deviceId: string;
  deviceName: string;
  status: JobStatus;
  totalFiles: number;
  totalBytes: number;
  statsJson: string;
  startedAt: number;
  finishedAt: number | null;
}

export interface LogRow {
  id: number;
  ts: number;
  level: LogLevel;
  jobId: number;
  message: string;
}

export interface ImportStats {
  totalFiles: number;
  doneFiles: number;
  skippedDuplicates: number;
  failedFiles: number;
  totalBytes: number;
  doneBytes: number;
  elapsedMs: number;
  bytesPerSec: number;
  /** 移动模式：成功移动（=复制后删除源）的文件数；copy 任务缺失（契约扩展中） */
  moved?: number;
  /** 移动模式：源文件删除失败数；copy 任务缺失（契约扩展中） */
  sourceDeleteFailed?: number;
}

/** 唯一事件通道 `app://event` 的 payload：以 type（camelCase）辨识的联合 */
export type AppEvent =
  | { type: "deviceFilesProgress"; id: string; files: FileEntryDto[] }
  | { type: "deviceScanFailed"; id: string; message: string }
  | { type: "deviceArrived"; id: string; kind: DeviceKind; name: string }
  | { type: "deviceRemoved"; id: string }
  | {
      type: "deviceScanned";
      id: string;
      name: string;
      kind: DeviceKind;
      snapshot: DeviceSnapshot;
    }
  | { type: "importSessionStarted"; jobId: number; totalFiles: number; totalBytes: number }
  | {
      type: "importFileProgress";
      jobId: number;
      doneFiles: number;
      doneBytes: number;
      /** 进度条口径：完成+跳过+失败的已结算字节（skip 不推进 doneBytes） */
      settledBytes: number;
      currentFile: string;
      bytesPerSec: number;
    }
  | { type: "importPaused"; jobId: number }
  | { type: "importResumed"; jobId: number }
  | { type: "importCancelled"; jobId: number }
  | { type: "importSessionFinished"; jobId: number; stats: ImportStats }
  | { type: "importFileCompleted"; jobId: number; src: string; dst: string; state: string }
  | { type: "cleanStarted"; jobId: number; count: number; bytes: number }
  | { type: "cleanFinished"; jobId: number; stats: CleanResultDto }
  | { type: "thumbnailReady"; assetId: number; size: number; path: string }
  /** 索引任务启动恢复（库级后台：缩略图三档/EXIF 深提取/未来 AI）；pending=剩余项数 */
  | { type: "indexTaskResumed"; pending: number }
  /** 索引任务进度（kind="ai" 语义索引 / 其余为缩略图等）；节流由后端负责 */
  | { type: "indexTaskProgress"; kind: string; done: number; total: number }
  /** AI 模型下载进度（单模型，节流 1s） */
  | { type: "aiModelDownloadProgress"; id: string; doneBytes: number; totalBytes: number }
  /** AI 模型下载结束（ok=false 时 error 为原因文案） */
  | { type: "aiModelDownloadFinished"; id: string; ok: boolean; error?: string | null }
  /** 导出任务阶段推进（阶段 D；phase = render|encode|write|register） */
  | { type: "exportTaskProgress"; jobId: number; assetId: number; phase: string }
  /** 导出收尾（阶段 D；folder 模式带 outputPath，album 模式附带新资产 id） */
  | {
      type: "exportTaskFinished";
      jobId: number;
      assetId: number;
      ok: boolean;
      outputPath?: string | null;
      newAssetId?: number | null;
      error?: string | null;
    }
  /** 联拍收片（阶段 E-1；拍摄后新对象落卡即广播，UI 据此刷新设备文件列表） */
  | { type: "tetheringObjectAdded"; pnpId: string; objectName: string; objectSize: number | null }
  | { type: "tetheringPhotoAdded"; sessionId: string; libraryId: string; albumId: number; assetId: number; name: string }
  | { type: "tetheringStatus"; sessionId: string; connected: boolean; error: string | null }
  | { type: "mapGeoProgress"; stage: string; done: number; total: number; message: string | null }
  | { type: "mapRegionsUpdated" }
  | { type: "appError"; level: string; message: string; recoverable: boolean };

// --- M3 画廊/搜索/查看器契约 ------------------------------------------------------

/** 库内资产大类（与导入侧 FileKind 对齐，不含 other——入库文件必属其一） */
export type AssetKind = "photo" | "raw";

/** 库内资产（assets_page 返回；按 capturedAt DESC 排列，capturedAt 为 NULL 的排最前） */
export interface AssetDto {
  id: number;
  /** 库内绝对路径 */
  path: string;
  name: string;
  kind: AssetKind;
  /** EXIF 拍摄时间（ISO 8601）；EXIF 缺失为 null——前端归「未知日期」组（组序最前） */
  capturedAt: string | null;
  camera: string | null;
  sizeBytes: number;
  /** RAW+JPG 展示组 id（两侧资产 ID 的较小值，同一拍摄的两格式同值） */
  pairId?: number | null;
  /** 像素宽（EXIF 深提取回填，~95% 资产有值）；缺失时前端 justify 网格按 4:3 兜底 */
  width?: number | null;
  /** 像素高（同上） */
  height?: number | null;
  /** 入库时间（ISO 8601；recent_assets 用于「最近添加」范围过滤，assets_page 不带） */
  createdAt?: string | null;
  /** 连拍组 id（M6：时间链×pHash 聚类；同组连续资产在画廊折叠为堆叠卡） */
  burstId?: number | null;
  /** 连拍组员数（照片数，含封面；仅 ≥2 时携带） */
  burstCount?: number | null;
  /** 收藏星标 */
  flagged?: boolean;
  /** 五星代表收藏，与批量收藏操作一致 */
  rating?: number;
  /** 颜色标签（LR 五色：red/yellow/green/blue/purple）；null=未设置。后端契约扩展中 */
  colorLabel?: string | null;
  /** 拒绝旗标（与星级分层的独立标记；筛选/回收站前弱化展示）。后端契约扩展中 */
  rejected?: boolean;
  /** 是否在回收站（trash_list 返回 true；常规查询不携带/为 false） */
  inTrash?: boolean;
}

/** 相机型号计数（cameras_list 返回，搜索页相机勾选数据源；按 count 降序） */
export interface AssetCameraCount {
  camera: string;
  count: number;
}

/** 镜头型号计数（lens_list 返回，搜索页镜头勾选数据源；按 count 降序） */
export interface AssetLensCount {
  lens: string;
  count: number;
}

/** 文件格式计数（format_list 返回，搜索页格式勾选数据源；如 NEF/ARW/JPG） */
export interface AssetFormatCount {
  format: string;
  count: number;
}

/** 搜索/过滤条件（camelCase 平铺进 assets_page 负载；全字段可省略） */
export interface AssetFilters {
  /** 类型集合（照片=photo+raw，RAW=raw）；省略=全部。
   *  M3 二轮起用 kinds 数组（后端 lane 同步加 Vec<AssetKind> 参数），替代单值 kind */
  kinds?: AssetKind[];
  /** RFC3339（后端按绝对时间归一比较）；前端由 "YYYY-MM-DD" 本地日界转 UTC */
  capturedAfter?: string;
  capturedBefore?: string;
  /** 相机型号多选：所选值精确匹配（OR）；无相机信息的资产不会命中 */
  cameras?: string[];
  // --- M4 扩展筛选（后端 lane 契约扩展中；全 Optional，后端未实现时不传） ---
  /** 镜头多选 OR */
  lenses?: string[];
  /** 文件格式多选 OR（如 NEF/ARW/JPG） */
  formats?: string[];
  /** 焦距区间（mm，含端点） */
  focalMin?: number;
  focalMax?: number;
  /** ISO 区间（含端点） */
  isoMin?: number;
  isoMax?: number;
  /** 光圈 f 值区间（如 2.8-5.6，含端点） */
  apertureMin?: number;
  apertureMax?: number;
  /** 快门区间（秒，如 0.004=1/250s，含端点） */
  shutterMin?: number;
  shutterMax?: number;
  /** 闪光灯是否闪光（true=开/false=关；省略=不过滤） */
  /** 闪光灯三态："on"=闪光 | "off"=已知未闪光 | "unknown"=无信息 */
  flash?: "on" | "off" | "unknown";
  /** 拍摄方向（按 EXIF orientation 归类）；省略=不过滤 */
  orientation?: "landscape" | "portrait";
  /** 是否有 GPS 坐标（true=有/false=无；省略=不过滤） */
  hasGps?: boolean;
  /** 文件大小区间（字节，含端点） */
  sizeMin?: number;
  sizeMax?: number;
  /** 所属相册（手工相册引用维度；省略 = 不过滤）。相册详情页内不重复携带 */
  albumId?: number;
  flagged?: boolean;
  ratingMin?: number;
  /** 颜色标签（LR 五色之一）；省略 = 不过滤。后端契约扩展中 */
  colorLabel?: string;
  /** 拒绝旗标三态过滤（true=仅拒绝 / false=仅未拒绝；省略 = 不限）。后端契约扩展中 */
  rejected?: boolean;
  /** 闭眼风险（C 阶段 AI 选片）：closed=有人闭眼 | maybe=可能闭眼；省略 = 不过滤。
   *  单值语义：closed/maybe 互斥（UI chips 二选一）。后端在途契约 */
  eyes?: "closed" | "maybe";
  /** 疑似失焦：soft=软片；省略 = 不过滤（无模型依赖，算法内置）。后端在途契约 */
  blur?: "soft";
  /** 相册子分组精确名（仅 album_assets_page 消费；省略 = 不限层） */
  subgroup?: string;
  /** true = 只看相册根散照片（album_item.subgroup 为 NULL；仅 album_assets_page 消费） */
  subgroupIsNull?: boolean;
}

/** 日期分组统计（asset_group_dates 返回，chips 条数据源；未知日期组 date=null 排最前） */
export interface AssetGroupDate {
  date: string | null;
  count: number;
  coverAssetId: number;
}

/**
 * 单资产全量元数据（asset_detail 返回，查看器 EXIF 面板）。
 * 字段名与后端实测对齐（src-tauri assets.rs：{id, flatten(AssetRow), duplicate_count}），
 * 由 assetDetail() 归一为 camelCase：后端 flatten 的 AssetRow 是 filename/size/createdAt，
 * 顶层 duplicate_count 未做 camelCase 重命名；EXIF 扩展字段（宽高/ISO/光圈/快门/焦距）
 * 后端暂未返回（契约扩展中），存在即透出、缺失为 null/undefined。
 */
export interface AssetDetailDto {
  id: number;
  path: string;
  /** 文件名（后端 AssetRow.filename） */
  filename: string;
  /** 文件大小（字节；后端 AssetRow.size） */
  size: number;
  kind: AssetKind;
  capturedAt: string | null;
  camera: string | null;
  /** 镜头 */
  lens?: string | null;
  /** 入库时间（后端 AssetRow.createdAt） */
  createdAt: string | null;
  /** 库内同指纹重复数（不含自身；后端 duplicate_count 归一） */
  dupCount: number;
  // --- EXIF 扩展（M4 契约扩展；未返回时缺省，面板按缺失隐藏行/组） ---
  width?: number | null;
  height?: number | null;
  /** 总像素（百万，如 24.3） */
  megapixels?: number | null;
  /** 长宽比（如 "3:2"） */
  aspect?: string | null;
  /** EXIF 方向 1-8（5-8=竖拍；1-4=横拍，含镜像） */
  orientation?: number | null;
  /** 快门（如 "1/250"；面板追加 s 展示） */
  shutter?: string | null;
  /** 光圈 f 值 */
  /** 光圈/焦距为展示态字符串（"2.8"/"59"），查看器直接拼 f//mm */
  aperture?: string | null;
  focalLength?: string | null;
  iso?: number | null;
  /** 闪光灯（后端翻译后的文案，如「闪光」/「未闪光」） */
  flash?: string | null;
  meteringMode?: string | null;
  whiteBalance?: string | null;
  exposureProgram?: string | null;
  software?: string | null;
  artist?: string | null;
  gpsLat?: number | null;
  gpsLon?: number | null;
  /** 文件格式（扩展名大写，如 "NEF"） */
  format?: string | null;
  /** 评分 0-5（0=未评；查看器星标条） */
  rating?: number | null;
  /** 旗标（待整理标记） */
  flagged?: boolean | null;
  /** AI 选片分析（C 阶段；null=未分析）。
   *  eyes.value：closed=有人闭眼 | maybe=可能闭眼 | no_face=未检出人脸（后端
   *  归一 token，未知值原样透传）；blur.value：soft=疑似软片。
   *  score 均为 0-100 置信分。 */
  aiAnalysis: {
    eyes?: { value: string; score: number };
    blur?: { value: string; score: number };
  } | null;
}

// --- 安全清卡（M2）：候选预览 → 强确认 → 后端逐文件指纹复验后删除 ---------------

/** 可清理源文件（clean_candidates 返回；已入库且指纹匹配的源文件） */
export interface CleanCandidateDto {
  /** 源文件绝对路径 */
  src: string;
  /** 库内相对路径 */
  relPath: string;
  size: number;
  assetId: string | null;
}

/** 清卡结果（clean_apply 返回 / cleanFinished 事件） */
export interface CleanResultDto {
  deleted: number;
  failed: number;
  freedBytes: number;
  errors: string[];
}

// --- 命令封装 ----------------------------------------------------------------

/** 设备文件条目 DTO（device_files 返回；relPath 为设备内相对路径，"/" 分隔） */
export interface FileEntryDto {
  id: string;
  relPath: string;
  size: number;
  mtime: string;
}

/** 文件系统目录树的单个节点（hasSubdirs=false 时无子目录、不显示展开箭头） */
export interface FsDirEntry {
  name: string;
  path: string;
  hasSubdirs: boolean;
}

/** import_start 结果：ok=false 时 error 为后端 Err 文案；error=null 表示 invoke 本身不可用（调用方显示通用文案） */
export type ImportStartResult = { ok: true; jobId: number } | { ok: false; error: string | null };

// --- 连拍分组（M6） -------------------------------------------------------------------

/** 连拍统计（burst_stats 返回；后端不可用/失败为 null——UI 隐藏展示行） */
export interface BurstStats {
  groups: number;
  photosInBursts: number;
}

/** 机身/镜头型号计数（gear_stats TOP 榜；按 count 降序由后端排） */
export interface GearNameCount {
  /** 型号名（EXIF Model/LensModel，后端已做空值清洗，如 "Canon EOS R5"） */
  name: string;
  count: number;
}

/** 焦段分桶（24/50/85/135/200+mm）：max=null 表示 200+ 开放桶 */
export interface GearFocalBucket {
  /** 桶文案（如 "24mm"、"200+mm"） */
  label: string;
  /** 桶下界（含）；200+ 桶 min=200 */
  min: number;
  /** 桶上界（不含）；null=开放桶 */
  max: number | null;
  count: number;
}

/** 标签计数桶（ISO/光圈/快门分布共用；label 由后端生成，如 "ISO 400"、"f/2.8"、"1/250s"） */
export interface GearLabelBucket {
  label: string;
  count: number;
}

/** 器材与拍摄参数分布快照（gear_stats 返回；无 EXIF 数据/后端未就绪为 null） */
export interface GearStats {
  /** 机身 TOP（型号 × 快门数） */
  cameras: GearNameCount[];
  /** 镜头 TOP */
  lenses: GearNameCount[];
  /** 焦段分布（24/50/85/135/200+mm 分桶） */
  focalBuckets: GearFocalBucket[];
  /** ISO 分布（100/200/400/800/1600/3200+） */
  isoBuckets: GearLabelBucket[];
  /** 光圈分布（f/1.x/2.x/4/5.6/8/11+） */
  apertureBuckets: GearLabelBucket[];
  /** 快门分布（>1s/1s/1/2…1/1000+ 归档） */
  shutterBuckets: GearLabelBucket[];
}

// --- 两级去重（M7 F8：完全重复 exact / 近似 similar） -----------------------------------

/** 去重档位：exact = (size, xxhash) 完全相同；similar = pHash 汉明 ≤6 近似
 *  （RAW+JPG 孪生已被后端排除；连拍组内不排除——正是挑片场景） */
export type DuplicateKind = "exact" | "similar";

/** 重复组（duplicates_list 返回）：组内 created_at 升序（首张=最早入库） */
export interface DuplicateGroupDto {
  kind: DuplicateKind;
  assets: AssetDto[];
}

// --- M4 AI：模型管理 / 语义搜索 ----------------------------------------------------

export type AiFeature = "semantic" | "face" | "selection";
export type AiModelState = "idle" | "downloading" | "verifying" | "done" | "failed";
/** AI 三档画质（契约：settings.ai.qualityTier 同域；ModelEntry.tier 归属档位） */
export type AiQualityTier = "fast" | "normal" | "accurate";

/** AI 模型状态（ai_models_status 返回；清单：siglip2-visual/siglip2-text/scrfd/arcface
 *  + 三档画质新件 scrfd-10g / siglip2-vision-fp16 / siglip2-text-fp16） */
export interface AiModelStatus {
  id: string;
  /** 旧字段兼容（=state==="done"） */
  installed: boolean;
  bytesTotal: number;
  downloadedBytes: number;
  version: string | null;
  feature: AiFeature;
  state: AiModelState;
  /** 画质档位归属（三档画质契约）：fast/normal/accurate=该档专用件，
   *  null=各档共用件（如 tokenizer）。旧后端未发此字段时视为 null（共用）。 */
  tier?: AiQualityTier | null;
}

// --- M4 人物（人脸聚类）契约 ------------------------------------------------------

/** 人物聚类条目（people_list 返回；faceCount 降序由后端保证） */
export interface PersonCluster {
  clusterId: number;
  /** 用户命名；未命名为 null（UI 显示「人物 N」） */
  name: string | null;
  /** 聚类内人脸数（徽标） */
  faceCount: number;
  /** 封面资产 id（走 asset_thumb_get 管线取图） */
  coverAssetId: number;
}

export interface SemanticHit {
  assetId: number;
  /** 相似度 0..1 */
  score: number;
}

/** asset_thumb_get 四态结果（「排队中≠永久失败」的关键契约） */
export type ThumbGetResult =
  | { status: "ready"; path: string }
  | { status: "pending" }
  | { status: "unavailable" }
  /** 源文件已不在磁盘（被移动/删除，终态）。cachedPath 非空=找到历史缓存缩略图（尽力展示） */
  | { status: "missing"; cachedPath: string | null };

// --- 索引任务（缩略图/EXIF/语义）：状态与手动触发 -------------------------------------

export type IndexKind = "thumb" | "exif" | "ai" | "face";

/**
 * 单通道任务计数（index_status，对应后端 IndexKindStatus）。与后端持久化
 * 任务账（index_tasks 表）同构：pending/running 是「进行中」按钮态的派生
 * 真值；total=可索引资产数（N/M 口径）；failed 仅展示用途。
 */
export interface IndexCounters {
  pending: number;
  running: number;
  done: number;
  failed: number;
  total: number;
}

/** 索引状态（index_status 返回；thumb/exif/ai 三通道计数） */
export interface IndexStatus {
  thumb: IndexCounters;
  exif: IndexCounters;
  ai: IndexCounters;
  /** 新后端始终返回；可选仅用于兼容旧版本/测试夹具。 */
  face?: IndexCounters;
}

// --- 索引重建 / 资产标记（M5） --------------------------------------------------------

/** 重建索引的通道（index_rebuild；语义通道在重建命令里叫 semantic，与 IndexKind 的 ai 区分） */
export type RebuildKind = "thumb" | "exif" | "semantic" | "face";

/** 删除库（library_delete）：库数据目录必删；photoRoot 给定时连照片目录
 *  一起删（后端三道闸：library.db 存在性/非活跃库/照片目录非盘根）。
 *  不 catch：失败文案透传给删除对话框。 */
export interface LibraryDeleteResult {
  dbDeleted: boolean;
  photoRootDeleted: boolean;
}

/** 库照片存储目录整体重定位（library_relocate）：改配置 + 重写库内路径
 *  前缀（前提：用户已在文件管理器把整树搬到新根）。apply=false 只预检。
 *  不 catch：失败文案（相对路径拒绝/库不存在）透传给调用方。 */
export interface LibraryRelocateResult {
  affected: number;
  unaffected: number;
  rootExists: boolean;
}

/** 侧栏导航计数（sidebar_counts：一次性纯 COUNT；后端在途契约——
 *  命令未注册/失败/形状异常静默 null，侧栏不显示徽标） */
export interface SidebarCounts {
  /** 图库资产总数 */
  assets: number;
  /** 最近浏览条数 */
  recentViewed: number;
  /** 那年今天条数 */
  onThisDay: number;
  /** 标签数 */
  tags: number;
  /** 相册数 */
  albums: number;
}

// --- 手工相册（纯引用照片组）契约 -----------------------------------------------------
// 相册 = 照片引用集合：同一照片可入多个相册；删除相册/移出相册只删引用，
// 永不动库内文件（后端 lane 并行实现中，前端按此契约封装）。

/** 手工相册（album_list 返回；createdAt DESC 由后端保证） */
export interface AlbumDto {
  id: number;
  name: string;
  /** 相册物理主目录名（目录化，B1 追加包契约）：显示名改名不动它；缺省回退 name */
  dirName?: string | null;
  /** 封面资产 id（album_cover_set 指定；null=未指定，前端回退相册第一张） */
  coverAssetId: number | null;
  /** 相册内照片数（引用数） */
  itemCount: number;
  /** 创建时间（ISO 8601） */
  createdAt: string;
}

/** 相册写操作结果：ok=false 时 error 为后端 Err 文案（如重名）；null = invoke 不可用 */
export type AlbumOpResult = { ok: true } | { ok: false; error: string | null };
export type AlbumCreateResult = { ok: true; album: AlbumDto } | { ok: false; error: string | null };

// --- LR 暂存夹（B1 追加包契约） --------------------------------------------------------

/** lr_staging_create 结果：dir=生成的暂存目录绝对路径 */
export interface LrStagingResult {
  dir: string;
  created: number;
  /** 硬链接条目数（同盘走硬链接） */
  hardlinked: number;
  /** 复制条目数（跨盘回退复制） */
  copied: number;
}

/** 相册子分组（album_subgroups 返回；B4 子分组模型：相册内任意命名文件夹层） */
export interface AlbumSubgroupDto {
  name: string;
  /** 组内照片数（引用数） */
  itemCount: number;
}

// --- 选片补全（B1）：颜色标签 / 拒绝旗标 / 回收站 / 智能视图 ----------------------------
// 后端 lane 并行实现中：命令未注册时按既有惯例静默降级（写操作 false、列表 []），
// UI 乐观更新不被传输失败阻塞。

/** 颜色标签枚举（LR 五色；与 AssetDto.colorLabel / AssetFilters.colorLabel 同域） */
export type ColorLabelKind = "red" | "yellow" | "green" | "blue" | "purple";

// --- B2：资产版本关系（RAW+机内JPEG 孪生展示；子分组为纯子文件夹，无内建成片语义） ----------------

/** 版本组成员（role: raw=原片 RAW | sooc=机内 JPEG | derived=成片；未入组 null） */
export interface VersionMember {
  assetId: number;
  role: "raw" | "sooc" | "derived" | null;
  name: string;
  thumbReady: boolean;
}

/** 版本查询载荷（groupId=null = 孤片，members 只有自己） */
export interface AssetVersions {
  groupId: number | null;
  members: VersionMember[];
}

/** User metadata stored per photo in the current library. */
export interface EditableMetadata {
  title: string; description: string; author: string; copyright: string; keywords: string[];
  capturedAt: string | null; camera: string; lens: string; gpsLat: number | null; gpsLon: number | null;
}

// --- 阶段 D：非破坏编辑配方 + JPEG 导出 ----------------------------------------------
// 契约（与后端 lane 共同遵守，字段名不得偏移）：
// - rotateQuarter 顺时针 90° 步进，先旋转后裁剪；crop 相对「旋转后图像」归一化；
// - 文字/笔迹坐标相对「裁剪后画布」归一化（画布宽=1；sizeRel=字高/画布宽、
//   widthRel=笔宽/画布宽；文字锚点=文本框左上角左对齐，多行 \n）。
// - 保存配方仅写本应用数据库（非破坏）；导出才生成新 JPEG（album 模式后端自动
//   命名 {stem}_edit.jpg，前端不传文件名）。

/** 裁剪矩形（相对旋转后图像归一化，x/y/w/h ∈ [0,1]） */
export interface EditRecipeCrop {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** 文字图层（多行用 \n；sizeRel=字高/画布宽） */
export interface EditRecipeTextLayer {
  id: string;
  x: number;
  y: number;
  text: string;
  sizeRel: number;
  color: string;
}

/** 画笔笔迹（widthRel=笔宽/画布宽；points 为裁剪后画布归一化折线） */
export interface EditRecipeBrushStroke {
  id: string;
  color: string;
  widthRel: number;
  points: { x: number; y: number }[];
}

/** 输出尺寸/质量（配方内嵌；longEdge=null = 原尺寸） */
export interface EditRecipeOutput {
  longEdge: number | null;
  quality: number;
}

/** 非破坏编辑配方（后端 edit_recipe 表存储的同一 JSON） */
export interface EditAdjustments { brightness: number; contrast: number; saturation: number; }

export interface EditRecipe {
  version: 1;
  rotateQuarter: 0 | 1 | 2 | 3;
  adjustments?: EditAdjustments;
  crop: EditRecipeCrop | null;
  textLayers: EditRecipeTextLayer[];
  brushStrokes: EditRecipeBrushStroke[];
  output: EditRecipeOutput;
}

/** editRecipeGet / editRecipeSave 返回：recipe=null 表示尚无配方 */
export interface EditRecipeState {
  recipe: EditRecipe | null;
  updatedAt: string | null;
}

/** 导出选项（folder 模式必填 folder；album 模式必填 album，文件名后端自动生成） */
export interface ExportOptions {
  mode: "folder" | "album";
  folder?: { outputDir: string; fileName: string };
  album?: { albumId: string; subgroup: string | null };
  longEdge?: number | null;
  quality?: number;
  removeGps?: boolean;
  copyright?: string;
  author?: string;
  keywords?: string[];
}

/** 导出产物（ExportTask.result；album 模式附带新资产 id） */
export interface ExportResultDto {
  outputPath: string;
  width: number;
  height: number;
  bytes: number;
  assetId: number | null;
}

/**
 * 导出任务 DTO（export_run 返回 / export_job 行投影；后端 src-tauri/src/edit/export.rs
 * ExportTaskDto 镜像——id 为库内任务号，exportTaskProgress/exportTaskFinished 事件按它关联）。
 */
export interface ExportTask {
  id: number;
  assetId: number;
  mode: string;
  status: "queued" | "running" | "done" | "error";
  result: ExportResultDto | null;
  error: string | null;
}

export type ExportRunResult =
  | { ok: true; task: ExportTask }
  | { ok: false; error: string | null };

// --- 阶段 E-1：WPD 零驱动联拍（相机能力探测 / 触发拍摄） ----------------------------
// 契约（与后端 lane 共同遵守）：pnpId = WPD PnP 设备 id，与设备列表 device.id 同源。
// 注意：清点命令叫 tethering_camera_list——gallery 的 camera_list（库内相机型号
// 计数）已占用无前缀名，前后端两侧都必须避开该撞名。收片事件 tetheringObjectAdded
// 走唯一事件通道 app://event（见 AppEvent 联合）。

/** 联拍能力位（camera_probe / tethering_camera_list 报告；全部按后端探测为准，
 *  UI 永远按报告渲染，不做品牌名承诺） */
export interface CameraInfo {
  /** WPD PnP 设备 id（与设备列表 id 同源） */
  pnpId: string;
  name: string;
  capabilities: {
    /** 文件传输（现有 WPD 导入通道） */
    fileTransfer: boolean;
    /** 标准 PTP InitiateCapture（0x100E）可触发拍摄 */
    standardCapture: boolean;
    /** Nikon 厂商扩展码（0x90C0）可触发拍摄 */
    vendorCaptureNikon: boolean;
    /** OBJECT_ADDED 事件订阅（拍摄后自动收片） */
    objectAddedEvents: boolean;
    liveView: boolean;
  };
}

/** camera_capture 结果：error=null 且 objectName 非 null = 成功触发并等到收片；
 *  error 非 null 为后端业务错误文案（UI 直接展示）；error=null 且 objectName=null
 *  表示 invoke 不可用等传输层失败（UI 用通用失败文案） */
export interface CameraCaptureResult {
  objectName: string | null;
  objectSize: number | null;
  error: string | null;
}

// --- 联拍会话（tethering_*：独立窗口 + 相册锚定 + 免导入任务入册） ---------------------

/** 相机拍摄参数（tethering_settings 报告）：id 为后端单配置名（gphoto
 *  list_config 名，如 shutterspeed/f-number/iso/exposuremetermode）。
 *  kind 决定控件形态；options 为 choice 候选表（value=设置用值，label=展示）。 */
export type TetherSettingKind = "choice" | "toggle" | "range" | "action" | "text";
export interface TetherSettingOption {
  value: string;
  label: string;
}
export interface TetherCameraSetting {
  id: string;
  label: string;
  kind: TetherSettingKind;
  current: string;
  writable: boolean;
  options: TetherSettingOption[];
  min?: number;
  max?: number;
  step?: number;
}

/** 会话内已入册照片（胶片条；后端只留最近 64 张）。 */
export interface TetherPhoto {
  id: number;
  name: string;
  kind: string;
}

/** 联拍会话（tethering_session/start 返回；多相机可并行多会话，同相机互斥）。 */
export interface TetherSessionDto {
  id: string;
  libraryId: string;
  albumId: number;
  albumName: string;
  camera: CameraInfo;
  settings: TetherCameraSetting[];
  photos: TetherPhoto[];
  connected: boolean;
  receiving: boolean;
  error: string | null;
}

export type TetherStartResult =
  | { ok: true; session: TetherSessionDto }
  | { ok: false; error: string | null };

export type TetherCaptureResult = { ok: true } | { ok: false; error: string | null };

// --- 选片会话（Culling V1）契约 ------------------------------------------------------
// 方案：docs/plans/2026-09-28-culling-proposal.md §1/§6。会话是一等持久化实体，
// 进度（已选/已剔除/未定/总数）全部由后端真值派生；决定即时落库（决定即保存）。
// 后端 lane 并行实装中：命令未注册时按既有惯例静默降级（列表 []、写操作
// false/null），前端乐观 UI 不被传输失败阻塞。

/** 决定值（未定 = null，无 cull_decision 行） */
export type CullDecisionValue = "accepted" | "rejected";

/** 会话来源快照：相册（含子组）/ 查询结果（asset id 快照，防新导入扰动） */
export type CullScope =
  | { kind: "album"; albumId: number; subgroup: string | null }
  | { kind: "query"; assetIds: number[] };

/** 选片会话（cull_session_* 返回；计数为后端派生真值） */
export interface CullSessionDto {
  id: number;
  name: string;
  scope: CullScope;
  total: number;
  accepted: number;
  rejected: number;
  undecided: number;
  createdAt: string;
  updatedAt: string;
  /** null = 进行中（会话归档不删，历史可查） */
  finishedAt: string | null;
}

/** 会话内单资产决定状态（cullSessionOpen 返回；decision=null 即未定） */
export interface CullItemState {
  assetId: number;
  decision: CullDecisionValue | null;
  /** manual=用户手标 / ai=AI 预标记（V3；用户可翻转） */
  origin: "manual" | "ai";
  /** 连拍组 id（null=无组；V2 对比视图组员选取/一键留张用） */
  burstId: number | null;
  /** 连拍组大小（快照内同组张数；>=2 即组徽标/对比可用；缺省 1） */
  burstSize: number;
}

/** cullSessionOpen 结果：会话 + 决定表（按会话快照序） */
export interface CullSessionOpenResult {
  session: CullSessionDto;
  items: CullItemState[];
}

/** 单条决定写入（decision=null 清除决定回未定） */
export interface CullDecisionPatch {
  assetId: number;
  decision: CullDecisionValue | null;
}

/** 收尾映射开关（已选→旗标/星级；已剔除→拒绝） */
export interface CullFinishApply {
  acceptedFlag: boolean;
  acceptedRating: number | null;
  rejectRejected: boolean;
}

/** cullSessionFinish 结果：各出口实际作用张数 */
export interface CullFinishResult {
  appliedFlag: number;
  appliedRating: number;
  rejected: number;
}

export type CullSessionCreateResult =
  | { ok: true; session: CullSessionDto }
  | { ok: false; error: string | null };

// --- AI 挑图（Culling V3，方案 §3.3）契约 --------------------------------------------
// AI 只建议不自动决定：预览（apply=false）出摘要，应用（apply=true）写入会话为
// origin='ai' 预标记（仅未定项），用户过片随时翻转。规则映射 ai_analysis 阈值。

/** 检测敏感度三档（weak=少报误报 / normal=默认 / strong=宁可错杀） */
export type CullAiSensitivity = "weak" | "normal" | "strong";

/** AI 挑图规则（cull_ai_prescan 载荷；表单态 → 规整由 cullingCore.buildAiRules） */
export interface CullAiRules {
  eyes: { enabled: boolean; sensitivity: CullAiSensitivity };
  blur: { enabled: boolean; sensitivity: CullAiSensitivity };
  /** 连拍组自动留最锐（组内其余标剔除建议；AfterShoot 式） */
  burstKeepSharpest: boolean;
  /** 豁免：合影人数 > N 不判闭眼等（0=关） */
  groupExemptFaces: number;
  /** 精选张数上限（null=不限；「帮我精选 30 张」） */
  maxAccepted: number | null;
}

/** cull_ai_prescan 结果（预览/应用同形；明细 assetIds 后端可能仅预览返回——可选） */
export interface CullPrescanDto {
  /** 建议保留张数 */
  suggestedAccepted: number;
  /** 建议剔除张数 */
  suggestedRejected: number;
  /** 已手动决定跳过张数 */
  skippedManual: number;
  /** 命中豁免（合影> N 人）张数 */
  exemptedGroup: number;
  /** 建议（剔除）明细资产 id（可选；仅预览可能返回） */
  assetIds?: number[];
}
