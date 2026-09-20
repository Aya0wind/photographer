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
