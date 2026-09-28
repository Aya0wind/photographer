//! 内嵌迁移 SQL：`PRAGMA user_version` 驱动，只加不改（spec §5.4）。
//!
//! 新迁移 = 追加一个 const 并挂到 [`MIGRATIONS`] 末尾，永不修改历史条目。
//! 每条迁移在独立事务中执行（DDL 与 user_version 推进原子提交）。

/// 迁移列表：索引 i 的 SQL 把库从 user_version = i 升到 i + 1。
pub(crate) const MIGRATIONS: &[&str] = &[
    MIGRATION_0001_INIT,
    MIGRATION_0002_PLAN_AND_LOOSE,
    MIGRATION_0003_ORIGIN_AND_DST2,
    MIGRATION_0004_SHOOTING_PARAMS,
    MIGRATION_0005_SEMANTIC_LEDGER,
    MIGRATION_0006_FACES_AND_PEOPLE,
    MIGRATION_0007_UNIQUE_INDEX_TASKS,
    MIGRATION_0008_DEEP_EXIF,
    MIGRATION_0009_RATING_AND_FLAG,
    MIGRATION_0010_VIEW_HISTORY,
    MIGRATION_0011_DROP_SHA256,
    MIGRATION_0012_BURSTS,
    MIGRATION_0013_SIMILAR_BUCKET,
    MIGRATION_0014_HASH_CHANNEL,
    MIGRATION_0015_ALBUMS,
    MIGRATION_0016_SELECTION,
    MIGRATION_0017_VERSIONS,
    MIGRATION_0018_ALBUM_DIRS,
    MIGRATION_0019_ALBUM_SUBGROUPS,
    MIGRATION_0020_DROP_DERIVED_RELATIONS,
    MIGRATION_0021_AI_SELECTION,
    MIGRATION_0022_EDIT_EXPORT,
    MIGRATION_0023_RESERVED,
    MIGRATION_0024_CULLING,
    MIGRATION_0025_EDITABLE_METADATA,
];

/// 0001：初始 schema——assets（查重索引与资产表）、jobs / job_files
/// （断点恢复 journal）、logs（任务日志），外加查询所需的索引。
const MIGRATION_0001_INIT: &str = r#"
CREATE TABLE assets (
    id          INTEGER PRIMARY KEY,
    path        TEXT    NOT NULL UNIQUE,
    filename    TEXT    NOT NULL,
    size        INTEGER NOT NULL,
    mtime       TEXT    NOT NULL,
    xxhash      INTEGER NOT NULL,
    sha256      BLOB    NOT NULL,
    kind        TEXT    NOT NULL CHECK (kind IN ('photo', 'raw', 'video', 'other')),
    captured_at TEXT,
    camera      TEXT,
    source      TEXT    NOT NULL,
    created_at  TEXT    NOT NULL
);

CREATE INDEX idx_assets_sha256      ON assets (sha256);
CREATE INDEX idx_assets_captured_at ON assets (captured_at);
CREATE INDEX idx_assets_xxhash      ON assets (xxhash);

CREATE TABLE jobs (
    id           INTEGER PRIMARY KEY,
    kind         TEXT    NOT NULL,
    device_id    TEXT    NOT NULL,
    device_name  TEXT    NOT NULL,
    status       TEXT    NOT NULL CHECK (status IN ('running', 'paused', 'done', 'cancelled', 'failed')),
    total_files  INTEGER NOT NULL,
    total_bytes  INTEGER NOT NULL,
    stats_json   TEXT,
    started_at   TEXT    NOT NULL,
    finished_at  TEXT
);

CREATE TABLE job_files (
    job_id  INTEGER NOT NULL REFERENCES jobs (id) ON DELETE CASCADE,
    src     TEXT    NOT NULL,
    dst     TEXT    NOT NULL,
    size    INTEGER NOT NULL,
    state   TEXT    NOT NULL CHECK (state IN ('pending', 'copying', 'verified', 'skipped', 'failed')),
    error   TEXT,
    xxhash  INTEGER,
    sha256  BLOB,
    PRIMARY KEY (job_id, src)
);

CREATE INDEX idx_job_files_state ON job_files (job_id, state);

CREATE TABLE logs (
    id      INTEGER PRIMARY KEY,
    ts      TEXT    NOT NULL,
    level   TEXT    NOT NULL,
    job_id  INTEGER,
    message TEXT    NOT NULL
);

CREATE INDEX idx_logs_job_id ON logs (job_id, id);
"#;

/// 0002：jobs 增加 plan_json（断点恢复/失败重试时重建 ImportPlan）；
/// assets 增加 (size, filename) 复合索引支撑宽松查重键（T7 §查重①）。
const MIGRATION_0002_PLAN_AND_LOOSE: &str = r#"
ALTER TABLE jobs ADD COLUMN plan_json TEXT;

CREATE INDEX idx_assets_size_filename ON assets (size, filename);
"#;

/// 0003（M2）：assets 增加 origin（'imported' 导入入册 / 'external' 原地索引
/// 只读入册，F1 清卡与 UI 据此区分资产来源）；job_files 增加 dst2
/// （F2 双目的地导入 journal 的第二目的地记录）。
const MIGRATION_0003_ORIGIN_AND_DST2: &str = r#"
ALTER TABLE assets ADD COLUMN origin TEXT NOT NULL DEFAULT 'imported';

ALTER TABLE job_files ADD COLUMN dst2 TEXT NOT NULL DEFAULT '';
"#;

/// 0004（M3.5）：assets 增加拍摄参数列（全 nullable：存量资产不回填，
/// 新导入经 EXIF 深提取自动有值）+ pair_asset_id（RAW/JPG 配对——同目录
/// 同 stem 另一格式资产的 id，导入入册时双向写，画廊合并展示）+
/// thumb_state（缩略图状态镜像：0=pending 1=done 2=permanent-none）+
/// index_tasks（索引任务表：导入/索引任务分离，资产级待办持久化，
/// 重启自动恢复；kind 路由通道——thumb/exif 走 CPU 全核通道，
/// ai 预留 GPU（DirectML/ort）通道，见 crate::index 通道说明）。
const MIGRATION_0004_SHOOTING_PARAMS: &str = r#"
ALTER TABLE assets ADD COLUMN width        INTEGER;
ALTER TABLE assets ADD COLUMN height       INTEGER;
ALTER TABLE assets ADD COLUMN iso          INTEGER;
ALTER TABLE assets ADD COLUMN f_number     TEXT;
ALTER TABLE assets ADD COLUMN exposure_time TEXT;
ALTER TABLE assets ADD COLUMN focal_length  TEXT;
ALTER TABLE assets ADD COLUMN lens         TEXT;
ALTER TABLE assets ADD COLUMN pair_asset_id INTEGER;
ALTER TABLE assets ADD COLUMN thumb_state INTEGER NOT NULL DEFAULT 0;

CREATE TABLE index_tasks (
    id         INTEGER PRIMARY KEY,
    kind       TEXT    NOT NULL CHECK (kind IN ('thumb', 'exif', 'ai')),
    asset_id   INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    state      TEXT    NOT NULL CHECK (state IN ('pending', 'running', 'done', 'failed')),
    attempts   INTEGER NOT NULL DEFAULT 0,
    created_at TEXT    NOT NULL,
    updated_at TEXT    NOT NULL
);

CREATE INDEX idx_index_tasks_state ON index_tasks (state, id);
"#;

/// 0005（M4）：assets.ai_indexed_at——语义嵌入记账（usearch 为向量真值，
/// 本列只是时间账：NULL = 未索引，语义回填据此建任务）。
const MIGRATION_0005_SEMANTIC_LEDGER: &str = r#"
ALTER TABLE assets ADD COLUMN ai_indexed_at TEXT;
"#;

/// 0006（M4 人脸全链路）：faces（人脸实例：SCRFD 框 + ArcFace 512 维特征 +
/// 人物簇归属，资产删除级联清脸）+ people（人物簇：名称可空 = 未命名，封面
/// 人脸；删簇 SET NULL 只解除归属不删脸数据）。assets.face_indexed_at 为
/// 人脸处理时间账（NULL = 未处理，人脸回填据此建任务；ai_face_data_clear
/// 连同 faces/people 一并复位）。
///
/// index_tasks.kind 的 CHECK 扩展 'face' 通道：SQLite 不支持 ALTER CHECK，
/// 采用整表重建（列定义与 0004 完全一致，仅 kind 集合扩展 + 数据原样
/// 搬迁——历史迁移条目不改动，数据零丢失）。
const MIGRATION_0006_FACES_AND_PEOPLE: &str = r#"
ALTER TABLE assets ADD COLUMN face_indexed_at TEXT;

CREATE TABLE faces (
    id          INTEGER PRIMARY KEY,
    asset_id    INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    box_x       REAL    NOT NULL,
    box_y       REAL    NOT NULL,
    box_w       REAL    NOT NULL,
    box_h       REAL    NOT NULL,
    embedding   BLOB    NOT NULL,
    cluster_id  INTEGER REFERENCES people (id) ON DELETE SET NULL,
    created_at  TEXT    NOT NULL
);

CREATE TABLE people (
    id            INTEGER PRIMARY KEY,
    name          TEXT,
    cover_face_id INTEGER REFERENCES faces (id) ON DELETE SET NULL,
    created_at    TEXT    NOT NULL
);

CREATE INDEX idx_faces_asset   ON faces (asset_id);
CREATE INDEX idx_faces_cluster ON faces (cluster_id);

CREATE TABLE index_tasks_new (
    id         INTEGER PRIMARY KEY,
    kind       TEXT    NOT NULL CHECK (kind IN ('thumb', 'exif', 'ai', 'face')),
    asset_id   INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    state      TEXT    NOT NULL CHECK (state IN ('pending', 'running', 'done', 'failed')),
    attempts   INTEGER NOT NULL DEFAULT 0,
    created_at TEXT    NOT NULL,
    updated_at TEXT    NOT NULL
);

INSERT INTO index_tasks_new (id, kind, asset_id, state, attempts, created_at, updated_at)
    SELECT id, kind, asset_id, state, attempts, created_at, updated_at FROM index_tasks;

DROP TABLE index_tasks;
ALTER TABLE index_tasks_new RENAME TO index_tasks;
CREATE INDEX idx_index_tasks_state ON index_tasks (state, id);
"#;

/// 0007：每个资产的每种索引任务只能有一条。旧实现仅排除 pending/running，
/// failed 后再次点击“立即索引”会重复 INSERT，导致 119 张照片累计出 1700+
/// 条失败记录。迁移优先保留已完成任务，其次保留仍可执行的任务，最后才保留
/// 最新失败任务，再建立唯一约束；手动重试通过 UPDATE 原任务完成。
const MIGRATION_0007_UNIQUE_INDEX_TASKS: &str = r#"
DELETE FROM index_tasks
WHERE id NOT IN (
    SELECT id FROM (
        SELECT id,
               ROW_NUMBER() OVER (
                   PARTITION BY kind, asset_id
                   ORDER BY CASE state
                       WHEN 'done' THEN 0
                       WHEN 'running' THEN 1
                       WHEN 'pending' THEN 2
                       ELSE 3
                   END,
                   updated_at DESC,
                   id DESC
               ) AS row_num
        FROM index_tasks
    ) ranked
    WHERE row_num = 1
);

CREATE UNIQUE INDEX idx_index_tasks_kind_asset ON index_tasks (kind, asset_id);
"#;

/// 0008（M5 深提取）：assets 拍摄参数扩展列（全可空，存量资产经
/// exif-gen-2 代际回填补齐——见 index::refresh_exif_for_generation）。
/// orientation = EXIF 1-8；flash/metering_mode/white_balance/exposure_program
/// 为规范化 token（见 metadata::exif_lite 映射表）；gps_lat/gps_lon 为十进
/// 制度（南纬/西经为负）。数值范围筛选（focal/f_number/exposure_time 等）
/// 对 NULL 行不匹配——「未知」不冒充任何区间。
const MIGRATION_0008_DEEP_EXIF: &str = r#"
ALTER TABLE assets ADD COLUMN orientation INTEGER;
ALTER TABLE assets ADD COLUMN flash TEXT;
ALTER TABLE assets ADD COLUMN metering_mode TEXT;
ALTER TABLE assets ADD COLUMN white_balance TEXT;
ALTER TABLE assets ADD COLUMN exposure_program TEXT;
ALTER TABLE assets ADD COLUMN software TEXT;
ALTER TABLE assets ADD COLUMN artist TEXT;
ALTER TABLE assets ADD COLUMN gps_lat REAL;
ALTER TABLE assets ADD COLUMN gps_lon REAL;
"#;

/// 0009（M5 评分与 LR 互通）：rating 0-5（0 = 未评）；flagged 布尔语义
/// （0/1，收藏旗标）。两列 NOT NULL DEFAULT 0——无 NULL 态，筛选语义
/// 简单。评分写入侧自动同步 XMP 边车（crate::metadata::xmp），LR 存量
/// 边车的 xmp:Rating 经 exif 通道回填进库。
const MIGRATION_0009_RATING_AND_FLAG: &str = r#"
ALTER TABLE assets ADD COLUMN rating INTEGER NOT NULL DEFAULT 0;
ALTER TABLE assets ADD COLUMN flagged INTEGER NOT NULL DEFAULT 0;
"#;

/// 0010（M5 最近浏览）：view_history 每资产一行（PK = asset_id，upsert
/// 天然去重，查询按 viewed_at DESC 即「最近浏览」序）；资产删除级联清
/// 历史。用户定案 2026-09-20：「最近添加」改「最近浏览」。
const MIGRATION_0010_VIEW_HISTORY: &str = r#"
CREATE TABLE view_history (
    asset_id  INTEGER PRIMARY KEY REFERENCES assets (id) ON DELETE CASCADE,
    viewed_at TEXT NOT NULL
);
"#;

/// 0011（M5）：资产/journal 的 SHA256 指纹退役（用户定案 2026-09-20）：
/// 完全重复级判据 = (size, xxhash) 复合键——64 位 xxhash 在 20 万规模
/// 碰撞 ~1e-9，且查重/删除均有人工确认环节；复制流少一次哈希更新。
/// 注意：AI 模型下载校验的 sha256（ai/mod.rs）是另一回事，不受影响。
const MIGRATION_0011_DROP_SHA256: &str = r#"
DROP INDEX idx_assets_sha256;
ALTER TABLE assets DROP COLUMN sha256;
ALTER TABLE job_files DROP COLUMN sha256;
"#;

/// 0012（M6 连拍分组）：pHash 感知指纹（u64 按 i64 位型存，NULL=未算）
/// + bursts 连拍组表 + assets.burst_id 归属（组重算时整体重写）。
///
/// 分组双因子 = 时间链（captured_at 间隔）× 场景链（pHash 汉明距离），
/// 参数见 settings.ai.burst_*。
const MIGRATION_0012_BURSTS: &str = r#"
ALTER TABLE assets ADD COLUMN phash INTEGER;

CREATE TABLE bursts (
    id          INTEGER PRIMARY KEY,
    asset_count INTEGER NOT NULL,
    started_at  TEXT,
    ended_at    TEXT
);

ALTER TABLE assets ADD COLUMN burst_id INTEGER REFERENCES bursts (id) ON DELETE SET NULL;
CREATE INDEX idx_assets_burst ON assets (burst_id);

-- kind 扩 'phash' 通道（整表重建，同 0006 手法；0007 唯一索引随表重建）
CREATE TABLE index_tasks_new (
    id         INTEGER PRIMARY KEY,
    kind       TEXT    NOT NULL CHECK (kind IN ('thumb', 'exif', 'ai', 'face', 'phash')),
    asset_id   INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    state      TEXT    NOT NULL CHECK (state IN ('pending', 'running', 'done', 'failed')),
    attempts   INTEGER NOT NULL DEFAULT 0,
    created_at TEXT    NOT NULL,
    updated_at TEXT    NOT NULL
);
INSERT INTO index_tasks_new (id, kind, asset_id, state, attempts, created_at, updated_at)
    SELECT id, kind, asset_id, state, attempts, created_at, updated_at FROM index_tasks;
DROP TABLE index_tasks;
ALTER TABLE index_tasks_new RENAME TO index_tasks;
CREATE UNIQUE INDEX idx_index_tasks_kind_asset ON index_tasks (kind, asset_id);
CREATE INDEX idx_index_tasks_state ON index_tasks (state, id);
"#;

/// 0013（M7 F8 近重复分桶）：pHash 64-bit 切 4 段 16-bit 入桶（多探针：
/// 汉明 ≤6 时 4 段差的总和 ≤6 < 4×2，必有一段差 0——段相等必命中，零漏检；
/// 命中对再精确汉明过滤）。表随 phash 任务增量插入；行数 ≠ 4×phash 数时
/// 纯 SQL 全量重建（懒校验）；资产删除级联清行。
const MIGRATION_0013_SIMILAR_BUCKET: &str = r#"
CREATE TABLE similar_bucket (
    segment  INTEGER NOT NULL,
    seg_val  INTEGER NOT NULL,
    asset_id INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    PRIMARY KEY (segment, seg_val, asset_id)
);
"#;

/// 0014（M8-② 后台哈希通道）：同卷 rename 快道跳过流式复制，无内联
/// xxhash → assets.xxhash 写 0 哨兵（=待补算），index_tasks 加 kind='hash'
/// 通道由 CPU worker 补算（重放安全：整表重建从当前数据复制）。精确查重
/// 层 (size, xxhash) 对哨兵行跳过，补算后自动就位。
const MIGRATION_0014_HASH_CHANNEL: &str = r#"
CREATE TABLE index_tasks_new (
    id         INTEGER PRIMARY KEY,
    kind       TEXT    NOT NULL CHECK (kind IN ('thumb', 'exif', 'ai', 'face', 'phash', 'hash')),
    asset_id   INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    state      TEXT    NOT NULL CHECK (state IN ('pending', 'running', 'done', 'failed')),
    attempts   INTEGER NOT NULL DEFAULT 0,
    created_at TEXT    NOT NULL,
    updated_at TEXT    NOT NULL
);
INSERT INTO index_tasks_new (id, kind, asset_id, state, attempts, created_at, updated_at)
    SELECT id, kind, asset_id, state, attempts, created_at, updated_at FROM index_tasks;
DROP TABLE index_tasks;
ALTER TABLE index_tasks_new RENAME TO index_tasks;
CREATE UNIQUE INDEX idx_index_tasks_kind_asset ON index_tasks (kind, asset_id);
CREATE INDEX idx_index_tasks_state ON index_tasks (state, id);
"#;

/// 0015（M9 相册）：纯引用照片组——album（相册元数据 + 封面引用）+
/// album_item（相册×资产多对多引用）。相册只引用全局图库资产，绝不持有
/// 物理文件：删除相册仅级联清 album_item 引用（ON DELETE CASCADE）；
/// 资产永久删除（duplicate_delete 等）时引用随资产级联消失、封面经
/// ON DELETE SET NULL 解除。AUTOINCREMENT 保证相册 id 删除后不复用
/// （前端路由/事件携带的旧 id 不致指向新相册）。FK 级联依赖连接级
/// `PRAGMA foreign_keys=ON`（[`Db::open`]，与 faces/view_history 同构）。
const MIGRATION_0015_ALBUMS: &str = r#"
CREATE TABLE album (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    name           TEXT    NOT NULL UNIQUE,
    cover_asset_id INTEGER REFERENCES assets (id) ON DELETE SET NULL,
    created_at     TEXT    NOT NULL
);

CREATE TABLE album_item (
    album_id  INTEGER NOT NULL REFERENCES album (id) ON DELETE CASCADE,
    asset_id  INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    added_at  TEXT    NOT NULL,
    PRIMARY KEY (album_id, asset_id)
);

CREATE INDEX idx_album_item_asset       ON album_item (asset_id);
CREATE INDEX idx_album_item_album_added ON album_item (album_id, added_at);
"#;

/// 0016（阶段 B1 选片补全）：
/// - `color_label`：LR 标准色名小写 token（red/yellow/green/blue/purple，
///   NULL = 无标签）。写回侧同步 `xmp:Label`（LR 原生就是标准色名，无映射
///   配置成本）；非法 token 由 IPC 层拒绝，列不设 CHECK（SQLite ALTER 加不了）。
/// - `rejected`：接受/拒绝状态（布尔 0/1），与星级分层的应用内选片状态
///   （roadmap §3：拒绝不写 XMP，避免与星级/旗标混淆）。默认查询**不排除**
///   已拒绝——只是可筛选项，区别于回收站。
/// - `in_trash` / `trashed_at`：应用内回收站（软删标记）。所有常规查询
///   默认排除 in_trash=1；物理删除走 trash_purge（显式动作）。
/// - `smart_view`：智能视图 = 前端 AssetFilters 序列化的命名存取（后端
///   不解释只存取，roadmap §3「搜索结果能保存为智能视图」）。AUTOINCREMENT
///   保证 id 删除后不复用（与 album 同款约定）。
/// - 索引 idx_assets_trash 支撑回收站列表 trashed_at DESC keyset。
const MIGRATION_0016_SELECTION: &str = r#"
ALTER TABLE assets ADD COLUMN color_label TEXT;
ALTER TABLE assets ADD COLUMN rejected INTEGER NOT NULL DEFAULT 0;
ALTER TABLE assets ADD COLUMN in_trash INTEGER NOT NULL DEFAULT 0;
ALTER TABLE assets ADD COLUMN trashed_at TEXT;

CREATE TABLE smart_view (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    name         TEXT    NOT NULL UNIQUE,
    filters_json TEXT    NOT NULL,
    created_at   TEXT    NOT NULL
);

CREATE INDEX idx_assets_trash ON assets (in_trash, trashed_at);
"#;

/// 0017（阶段 B2 原片-成片版本关系，roadmap §5）：
/// - `photo_group`：一次快门的逻辑组（raw=RAW 原片 / sooc=机内 JPEG /
///   derived=成片派生件）。组行只承载 id（成员在 group_asset），
///   AUTOINCREMENT 防删除后 id 复用（与 album/smart_view 同款约定）。
/// - `group_asset`：组×资产多对多（PK(group_id, asset_id)，一资产至多
///   属一组——应用逻辑保证，membership 行随资产/组删除双向级联）。
///   孤儿单资产不强制入组（无孪生的 v1 不建组，查询按需兼容）。
///   组形成：导入引擎 pair 配对（raw+photo）升级为同组建组；存量 pair
///   数据跑 scripts/backfill_photo_groups.py 幂等回填。
/// - `asset_relation`：成片→原片的 `derived_from` 显式关系（source 记
///   lr_export/app_edit；match_basis 记 filename/exif_time/phash_score
///   组合 JSON；confirmed=用户已确认的强关联）。
/// - 资产删除级联：group_asset/asset_relation 均随 assets 行消失；
///   空组行由删除路径清理（assets_delete_rows）。
const MIGRATION_0017_VERSIONS: &str = r#"
CREATE TABLE photo_group (
    id INTEGER PRIMARY KEY AUTOINCREMENT
);

CREATE TABLE group_asset (
    group_id INTEGER NOT NULL REFERENCES photo_group (id) ON DELETE CASCADE,
    asset_id INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    role     TEXT    NOT NULL CHECK (role IN ('raw', 'sooc', 'derived')),
    PRIMARY KEY (group_id, asset_id)
);

CREATE INDEX idx_group_asset_asset ON group_asset (asset_id);

CREATE TABLE asset_relation (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    asset_id         INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    related_asset_id INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    kind             TEXT    NOT NULL CHECK (kind IN ('derived_from')),
    source           TEXT,
    match_basis      TEXT,
    confirmed        INTEGER NOT NULL DEFAULT 0,
    created_at       TEXT    NOT NULL
);

CREATE INDEX idx_asset_relation_asset   ON asset_relation (asset_id);
CREATE INDEX idx_asset_relation_related ON asset_relation (related_asset_id);
"#;

/// 0018（阶段 B3 相册物理目录化，用户定案 2026-09-27；布局改版 2026-09-28）：
/// - `album.dir_name`：相册物理主目录名（布局 `photoRoot/{创建YYYY}/
///   {创建MM}/{dir_name}/`——外层=相册创建时间年月（UTC 口径）、相册内
///   平铺；公式统一在 [`crate::db::album_home_rel_parts`]，一次拍摄任务
///   一册）。创建时由显示名净化生成
///   （[`crate::db::sanitize_dir_name`]），显示名改名不动它；受控改目录走
///   album_dir_rename（物理 rename + DB 路径批量更新）。NOT NULL UNIQUE 经
///   唯一索引实现（SQLite ALTER 加不了 UNIQUE 列）。
/// - 存量行回填 `album-{id}`（确定性唯一；用户定案：存量测试数据自行重导，
///   无迁移负担，不做按名净化回填）。
const MIGRATION_0018_ALBUM_DIRS: &str = r#"
ALTER TABLE album ADD COLUMN dir_name TEXT NOT NULL DEFAULT '';
UPDATE album SET dir_name = 'album-' || id WHERE dir_name = '';
CREATE UNIQUE INDEX idx_album_dir_name ON album (dir_name);
"#;

/// 0019（相册子分组，用户定案 2026-09-27）：相册=容器，可直放散照片；
/// 子分组 = album_item 上的命名层（NULL = 散在相册根）。子分组名无特殊
/// 语义（「成片」「原片」只是约定），同一 (album_id, asset_id) 仍唯一——
/// 一张照片在一个相册里只属于一个子分组或根。索引支撑子分组清单
/// （DISTINCT）与子分组视图查询。
const MIGRATION_0019_ALBUM_SUBGROUPS: &str = r#"
ALTER TABLE album_item ADD COLUMN subgroup TEXT;
CREATE INDEX idx_album_item_subgroup ON album_item (album_id, subgroup);
"#;

/// 0020（移除内嵌原片/成片概念，用户定案 2026-09-27 简化）：asset_relation
/// 表删除——唯一使用者是 lr_export_import（成片导回），该功能已随概念一并
/// 移除（派生件走普通导入 + 相册子分组，无显式派生关系）。
/// photo_group / group_asset **保留**：RAW+机内 JPEG 孪生分组与查看器版本
/// chips 仍依赖，与原成片概念无关。
const MIGRATION_0020_DROP_DERIVED_RELATIONS: &str = r#"
DROP TABLE asset_relation;
"#;

/// 0021（阶段 C AI 辅助选片）：ai_analysis —— 每资产每分析 kind 一行
/// （PK(asset_id, kind) upsert），kind ∈ eyes（闭眼三态 closed/maybe/
/// unknown；无人脸不产生记录）| blur（sharp/soft/unknown + 0-100 清晰度
/// 分）。输出只是可筛选建议，与用户决定分层，绝不自动写 XMP。
/// index_tasks kind 扩 'eyes'/'blur' 通道（整表重建，0012 手法）：
/// - eyes：独立通道（检测当时裁眼区域送分类器；模型未收录→通道空转跳过）
/// - blur：无模型依赖（512 档缩略图拉普拉斯清晰度分，始终可用）
const MIGRATION_0021_AI_SELECTION: &str = r#"
CREATE TABLE ai_analysis (
    asset_id      INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    kind          TEXT    NOT NULL CHECK (kind IN ('eyes', 'blur')),
    value         TEXT,
    score         REAL,
    model_version TEXT,
    analyzed_at   TEXT    NOT NULL,
    PRIMARY KEY (asset_id, kind)
);

CREATE TABLE index_tasks_new (
    id         INTEGER PRIMARY KEY,
    kind       TEXT    NOT NULL CHECK (kind IN ('thumb', 'exif', 'ai', 'face', 'phash', 'hash', 'eyes', 'blur')),
    asset_id   INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    state      TEXT    NOT NULL CHECK (state IN ('pending', 'running', 'done', 'failed')),
    attempts   INTEGER NOT NULL DEFAULT 0,
    created_at TEXT    NOT NULL,
    updated_at TEXT    NOT NULL
);
INSERT INTO index_tasks_new (id, kind, asset_id, state, attempts, created_at, updated_at)
    SELECT id, kind, asset_id, state, attempts, created_at, updated_at FROM index_tasks;
DROP TABLE index_tasks;
ALTER TABLE index_tasks_new RENAME TO index_tasks;
CREATE UNIQUE INDEX idx_index_tasks_kind_asset ON index_tasks (kind, asset_id);
CREATE INDEX idx_index_tasks_state ON index_tasks (state, id);
"#;

/// 0022（阶段 D 基础编辑与导出，roadmap §8）：
/// - `edit_recipe`：非破坏编辑配方（每资产至多一份，PK=asset_id）。配方为
///   version 化 JSON 文本（校验/夹取在 IPC 层做，库只存归一化后的文本）；
///   updated_at 为 Unix epoch 毫秒（DTO 层渲染 RFC3339）。资产删除级联清配方。
/// - `export_job`：导出任务账（持久化任务铁律——进程重启后 UI 仍可查
///   历史/终态）。status：queued→running→done|error（无暂停态：单文件导出
///   不支持断点续传，进程中断的遗留行由下一次 export_run 收尸为 error）。
///   结果四元组（output_path/width/height/bytes）+ album 模式的新资产 id。
///   行风格对齐 jobs（TEXT RFC3339 时间戳 + status CHECK）。
const MIGRATION_0022_EDIT_EXPORT: &str = r#"
CREATE TABLE edit_recipe (
    asset_id   INTEGER PRIMARY KEY REFERENCES assets (id) ON DELETE CASCADE,
    recipe     TEXT    NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE export_job (
    id           INTEGER PRIMARY KEY,
    asset_id     INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    mode         TEXT    NOT NULL CHECK (mode IN ('folder', 'album')),
    status       TEXT    NOT NULL CHECK (status IN ('queued', 'running', 'done', 'error')),
    output_path  TEXT,
    width        INTEGER,
    height       INTEGER,
    bytes        INTEGER,
    new_asset_id INTEGER,
    error        TEXT,
    created_at   TEXT    NOT NULL,
    finished_at  TEXT
);

CREATE INDEX idx_export_job_asset  ON export_job (asset_id, id);
CREATE INDEX idx_export_job_status ON export_job (status, id);
"#;

/// 0023（空占位，保号）：编号预留给并行 lane（AI/目录系）——本 lane 落地
/// 选片迁移 0024（编号已由用户批准的方案 docs/plans/2026-09-28-culling-
/// proposal.md §1 固定）。空 SQL = 仅推进 user_version 的 no-op，无任何 DDL。
const MIGRATION_0023_RESERVED: &str = "";

/// 0024（选片会话 V1，proposal §1 两表 + 快照表）：
/// - `cull_session`：选片会话一等实体。scope 为创建时来源 JSON（kind=
///   album{albumId,subgroup} | query{assetIds}——query 的 assetIds 是创建时
///   传入序的**来源记录**，当前快照真值在 cull_session_asset，两侧允许随
///   资产删除漂移）；name 不设唯一约束（同来源多轮会话由应用层命名规则
///   区分）。proposal §1 的 order_key/last_asset_id 不落列：快照序由
///   cull_session_asset.seq 承载，断点由 decision 覆盖推导（proposal 原注）。
/// - `cull_session_asset`：快照有序资产 id（seq 从 0 起，PK(session_id, seq)；
///   同一 asset 允许多次出现——应用层去重后写入）。**读路径注意**：V1 open
///   全量返回（万张内可接受），V2 加 keyset 分页时按 (session_id, seq) 游标。
/// - `cull_decision`：每会话每资产至多一条决定（未定 = 无行）。
///   origin manual|ai（AI 只预标记，用户可翻转——V3 写入）。
/// - 级联：会话删除 → 快照/决定行随灭；资产删除 → 两侧 asset_id 行随灭
///   （快照缩水、计数纯派生自然收敛）。asset_id 单列索引支撑级联反查。
const MIGRATION_0024_CULLING: &str = r#"
CREATE TABLE cull_session (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL,
    scope       TEXT    NOT NULL,
    created_at  TEXT    NOT NULL,
    updated_at  TEXT    NOT NULL,
    finished_at TEXT
);

CREATE TABLE cull_session_asset (
    session_id INTEGER NOT NULL REFERENCES cull_session (id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL,
    asset_id   INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    PRIMARY KEY (session_id, seq)
);

CREATE INDEX idx_cull_session_asset_asset ON cull_session_asset (asset_id);

CREATE TABLE cull_decision (
    session_id INTEGER NOT NULL REFERENCES cull_session (id) ON DELETE CASCADE,
    asset_id   INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    decision   TEXT    NOT NULL CHECK (decision IN ('accepted', 'rejected')),
    origin     TEXT    NOT NULL DEFAULT 'manual' CHECK (origin IN ('manual', 'ai')),
    decided_at TEXT    NOT NULL,
    PRIMARY KEY (session_id, asset_id)
);

CREATE INDEX idx_cull_decision_session ON cull_decision (session_id);
CREATE INDEX idx_cull_decision_asset  ON cull_decision (asset_id);
"#;

/// Explicit library metadata survives source EXIF reindexing.
const MIGRATION_0025_EDITABLE_METADATA: &str = r#"
CREATE TABLE asset_metadata (
    asset_id INTEGER PRIMARY KEY REFERENCES assets(id) ON DELETE CASCADE,
    value TEXT NOT NULL
);
"#;
