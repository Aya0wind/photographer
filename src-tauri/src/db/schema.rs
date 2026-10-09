//! 单版本建表脚本（2026-10-09 单数据库多照片库定案，计划 §二）。
//!
//! 版本未发布：**不做兼容、不做迁移、不带历史包袱**（用户红线）——本文件
//! 就是 schema 的唯一版本，历史迁移条目（0001..0027）已折叠进各表的最终
//! 形态，不写 migrate、不使用 `PRAGMA user_version`。改表 = 直接改本文件。
//!
//! 全部对象 `IF NOT EXISTS`：[`Db::open`](super::Db::open) 每次打开幂等执行；
//! 多连接并发首开时 DDL 逐语句原子提交 + `busy_timeout` 忙等收敛，无需
//! 迁移锁（旧「新库并发首开迁移锁」随 migrate 体系一并删除）。

/// 建表脚本（唯一版本；`Db::open` 幂等执行）。
pub(crate) const SCHEMA: &str = r#"
-- ===========================================================================
-- photos_libraries（§二）：照片库 = 一个文件夹的登记项（纯物理概念）。
-- 应用级唯一数据库里登记 N 个照片库；相册/子组是全局逻辑概念，不分库。
-- asset_count/size_bytes 为统计缓存（登记/删除后由刷新方法重算），
-- root_path 为规范化绝对路径（登记闸门保证），UNIQUE 仅兜底。
-- status：online=根在盘 | offline=整库离线（reconcile 按库粒度判定）。
-- ===========================================================================
CREATE TABLE IF NOT EXISTS photos_libraries (
    id          TEXT PRIMARY KEY,
    name        TEXT    NOT NULL,
    root_path   TEXT    NOT NULL UNIQUE,
    created_at  TEXT    NOT NULL,
    status      TEXT    NOT NULL DEFAULT 'online' CHECK (status IN ('online', 'offline')),
    asset_count INTEGER NOT NULL DEFAULT 0,
    size_bytes  INTEGER NOT NULL DEFAULT 0
);

-- ===========================================================================
-- assets：全局资产表（查重索引 + 元数据）。library_id 为归属照片库的
-- 静态属性（§一：仅整库重定位改 root，归属不变；无外键——移除登记是否
-- 连记录删由应用层决定，见 §七）；missing=单文件缺失标记（§五，整库
-- 离线走 photos_libraries.status）；xmp_dirty=离线期间元数据改动待补写
-- 边车（库恢复在线后 reconcile 补写清标志）。volume_serial+file_id 为
-- 登记指纹（§三 增量扫描两级识别：同卷同 id = 硬链接直跳，exFAT/FAT
-- 无 file id 时两者为 NULL 走哈希路径）。
-- ===========================================================================
CREATE TABLE IF NOT EXISTS assets (
    id          INTEGER PRIMARY KEY,
    path        TEXT    NOT NULL UNIQUE,
    filename    TEXT    NOT NULL,
    size        INTEGER NOT NULL,
    mtime       TEXT    NOT NULL,
    xxhash      INTEGER NOT NULL,
    kind        TEXT    NOT NULL CHECK (kind IN ('photo', 'raw', 'video', 'other')),
    captured_at TEXT,
    camera      TEXT,
    source      TEXT    NOT NULL,
    created_at  TEXT    NOT NULL,
    origin      TEXT    NOT NULL DEFAULT 'imported',
    width       INTEGER,
    height      INTEGER,
    iso         INTEGER,
    f_number    TEXT,
    exposure_time TEXT,
    focal_length  TEXT,
    lens        TEXT,
    pair_asset_id INTEGER,
    thumb_state INTEGER NOT NULL DEFAULT 0,
    ai_indexed_at TEXT,
    face_indexed_at TEXT,
    orientation INTEGER,
    flash       TEXT,
    metering_mode TEXT,
    white_balance TEXT,
    exposure_program TEXT,
    software    TEXT,
    artist      TEXT,
    gps_lat     REAL,
    gps_lon     REAL,
    rating      INTEGER NOT NULL DEFAULT 0,
    flagged     INTEGER NOT NULL DEFAULT 0,
    phash       INTEGER,
    burst_id    INTEGER REFERENCES bursts (id) ON DELETE SET NULL,
    color_label TEXT,
    rejected    INTEGER NOT NULL DEFAULT 0,
    in_trash    INTEGER NOT NULL DEFAULT 0,
    trashed_at  TEXT,
    library_id  TEXT,
    missing     INTEGER NOT NULL DEFAULT 0,
    xmp_dirty   INTEGER NOT NULL DEFAULT 0,
    volume_serial INTEGER,
    file_id     TEXT
);

CREATE INDEX IF NOT EXISTS idx_assets_captured_at  ON assets (captured_at);
CREATE INDEX IF NOT EXISTS idx_assets_xxhash       ON assets (xxhash);
CREATE INDEX IF NOT EXISTS idx_assets_size_filename ON assets (size, filename);
CREATE INDEX IF NOT EXISTS idx_assets_burst        ON assets (burst_id);
CREATE INDEX IF NOT EXISTS idx_assets_trash        ON assets (in_trash, trashed_at);
-- 画廊「来自哪个库」筛选（§一：库归属可筛选不可操作）
CREATE INDEX IF NOT EXISTS idx_assets_library      ON assets (library_id);
-- 登记指纹卷内索引（§三：新文件先查 file-id，命中 = 硬链接零哈希直跳）
CREATE INDEX IF NOT EXISTS idx_assets_volume_file  ON assets (volume_serial, file_id);

-- ===========================================================================
-- 库扫描状态（§三 增量扫描 / §八 2/3/7）：目录 mtime 剪枝缓存 + missing
-- 延迟确认缺席账 + 晚到边车已读 mtime。全部辅助账本，删库重建无损语义
--（只是重新全量枚举/重读一次边车）。
-- ===========================================================================
-- 目录 mtime 剪枝：dir 的 mtime 只反映直接子项增删/改名，子目录内容变化
-- 不改父 mtime——每目录独立判定（mtime 相同 → 跳过该目录的直接文件处理，
-- 子目录仍递归 stat 判定）。
CREATE TABLE IF NOT EXISTS library_scan_dirs (
    library_id TEXT NOT NULL,
    dir_path   TEXT NOT NULL,
    mtime      TEXT NOT NULL,
    PRIMARY KEY (library_id, dir_path)
);

-- missing 延迟确认（§八-7 连续两轮不在才标）：资产路径缺席第一轮记账，
-- 第二轮仍在缺席账上 → 标 missing 并删行；文件回来即删行。
CREATE TABLE IF NOT EXISTS library_scan_absent (
    asset_id   INTEGER PRIMARY KEY REFERENCES assets (id) ON DELETE CASCADE,
    library_id TEXT NOT NULL,
    noted_at   TEXT NOT NULL
);

-- 晚到边车补读（§八-3 触发一次）：已读入的边车 mtime——同 mtime 下轮不
-- 重读；边车再次更新（LR 改星级）→ mtime 变化 → 再读一次（DB 已有值不
-- 覆盖，应用内值优先，与 exif 通道同语义）。
CREATE TABLE IF NOT EXISTS library_scan_sidecars (
    asset_id INTEGER PRIMARY KEY REFERENCES assets (id) ON DELETE CASCADE,
    mtime    TEXT NOT NULL
);

-- ===========================================================================
-- library_scan_jobs（M4b 从文件夹建立 §三）：批量登记任务账本，一行一库。
-- 「从文件夹建立」落 pending 行，扫描 worker 单管道拾取 running →
-- done/failed；进度随扫描入库（total=预点数，registered/skipped 为跨轮
-- 累计绝对值——进程崩溃/重启后 running 行自然续跑，登记幂等，已登记
-- 文件按路径短路）。cancelling=软取消信号（worker 文件边界响应后删行，
-- 未登记文件由增量扫描常态拾取——单管道哲学 §八-5）；导入在场整轮让路
-- （行保持 pending，状态查询按导入旗投影 paused）。
-- ===========================================================================
CREATE TABLE IF NOT EXISTS library_scan_jobs (
    library_id TEXT PRIMARY KEY REFERENCES photos_libraries (id) ON DELETE CASCADE,
    status     TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'running', 'cancelling', 'done', 'failed')),
    total      INTEGER NOT NULL DEFAULT 0,
    registered INTEGER NOT NULL DEFAULT 0,
    skipped    INTEGER NOT NULL DEFAULT 0,
    error      TEXT,
    updated_at TEXT NOT NULL
);

-- ===========================================================================
-- jobs / job_files（断点恢复 journal）/ logs（任务日志）
-- ===========================================================================
CREATE TABLE IF NOT EXISTS jobs (
    id           INTEGER PRIMARY KEY,
    kind         TEXT    NOT NULL,
    device_id    TEXT    NOT NULL,
    device_name  TEXT    NOT NULL,
    status       TEXT    NOT NULL CHECK (status IN ('running', 'paused', 'done', 'cancelled', 'failed')),
    total_files  INTEGER NOT NULL,
    total_bytes  INTEGER NOT NULL,
    stats_json   TEXT,
    started_at   TEXT    NOT NULL,
    finished_at  TEXT,
    plan_json    TEXT
);

CREATE TABLE IF NOT EXISTS job_files (
    job_id  INTEGER NOT NULL REFERENCES jobs (id) ON DELETE CASCADE,
    src     TEXT    NOT NULL,
    dst     TEXT    NOT NULL,
    size    INTEGER NOT NULL,
    state   TEXT    NOT NULL CHECK (state IN ('pending', 'copying', 'verified', 'skipped', 'failed')),
    error   TEXT,
    xxhash  INTEGER,
    dst2    TEXT    NOT NULL DEFAULT '',
    PRIMARY KEY (job_id, src)
);

CREATE INDEX IF NOT EXISTS idx_job_files_state ON job_files (job_id, state);

CREATE TABLE IF NOT EXISTS logs (
    id      INTEGER PRIMARY KEY,
    ts      TEXT    NOT NULL,
    level   TEXT    NOT NULL,
    job_id  INTEGER,
    message TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_logs_job_id ON logs (job_id, id);

-- ===========================================================================
-- index_tasks：索引任务表（导入/索引任务分离，资产级待办；重启自动恢复）。
-- kind 通道路由：thumb/exif/hash/phash/blur=CPU 通道，ai/face=AI 推理通道，
-- eyes=闭眼检测通道。每资产每 kind 至多一条（唯一索引）。
-- ===========================================================================
CREATE TABLE IF NOT EXISTS index_tasks (
    id         INTEGER PRIMARY KEY,
    kind       TEXT    NOT NULL CHECK (kind IN ('thumb', 'exif', 'ai', 'face', 'phash', 'hash', 'eyes', 'blur')),
    asset_id   INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    state      TEXT    NOT NULL CHECK (state IN ('pending', 'running', 'done', 'failed')),
    attempts   INTEGER NOT NULL DEFAULT 0,
    created_at TEXT    NOT NULL,
    updated_at TEXT    NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_index_tasks_kind_asset ON index_tasks (kind, asset_id);
CREATE INDEX IF NOT EXISTS idx_index_tasks_state ON index_tasks (state, id);

-- ===========================================================================
-- faces / people：人脸实例与人物簇（资产删除级联清脸；删簇只解除归属）
-- ===========================================================================
CREATE TABLE IF NOT EXISTS faces (
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

CREATE INDEX IF NOT EXISTS idx_faces_asset   ON faces (asset_id);
CREATE INDEX IF NOT EXISTS idx_faces_cluster ON faces (cluster_id);

CREATE TABLE IF NOT EXISTS people (
    id            INTEGER PRIMARY KEY,
    name          TEXT,
    cover_face_id INTEGER REFERENCES faces (id) ON DELETE SET NULL,
    created_at    TEXT    NOT NULL
);

-- ===========================================================================
-- view_history：每资产一行（PK=asset_id upsert 去重，「最近浏览」序）
-- ===========================================================================
CREATE TABLE IF NOT EXISTS view_history (
    asset_id  INTEGER PRIMARY KEY REFERENCES assets (id) ON DELETE CASCADE,
    viewed_at TEXT NOT NULL
);

-- ===========================================================================
-- bursts：连拍组（成员数/起止时间；assets.burst_id 归属）
-- ===========================================================================
CREATE TABLE IF NOT EXISTS bursts (
    id          INTEGER PRIMARY KEY,
    asset_count INTEGER NOT NULL,
    started_at  TEXT,
    ended_at    TEXT
);

-- ===========================================================================
-- similar_bucket：pHash 64-bit 切 4 段 16-bit 入桶（多探针近重复分桶）
-- ===========================================================================
CREATE TABLE IF NOT EXISTS similar_bucket (
    segment  INTEGER NOT NULL,
    seg_val  INTEGER NOT NULL,
    asset_id INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    PRIMARY KEY (segment, seg_val, asset_id)
);

-- ===========================================================================
-- album / album_item：相册=纯引用照片组（全局逻辑概念，不分库 §一）。
-- 删除相册仅级联清引用；dir_name 为历史物理目录名（相册物理目录化已随
-- 纯时间布局退役，字段保留兼容既有读写路径，M5 逻辑化收尾时清理）。
-- ===========================================================================
CREATE TABLE IF NOT EXISTS album (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    name           TEXT    NOT NULL UNIQUE,
    cover_asset_id INTEGER REFERENCES assets (id) ON DELETE SET NULL,
    created_at     TEXT    NOT NULL,
    dir_name       TEXT    NOT NULL DEFAULT ''
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_album_dir_name ON album (dir_name);

CREATE TABLE IF NOT EXISTS album_item (
    album_id  INTEGER NOT NULL REFERENCES album (id) ON DELETE CASCADE,
    asset_id  INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    added_at  TEXT    NOT NULL,
    subgroup  TEXT,
    PRIMARY KEY (album_id, asset_id)
);

CREATE INDEX IF NOT EXISTS idx_album_item_asset       ON album_item (asset_id);
CREATE INDEX IF NOT EXISTS idx_album_item_album_added ON album_item (album_id, added_at);
CREATE INDEX IF NOT EXISTS idx_album_item_subgroup    ON album_item (album_id, subgroup);

-- ===========================================================================
-- smart_view：命名筛选存取（前端 AssetFilters 序列化，后端不解释）
-- ===========================================================================
CREATE TABLE IF NOT EXISTS smart_view (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    name         TEXT    NOT NULL UNIQUE,
    filters_json TEXT    NOT NULL,
    created_at   TEXT    NOT NULL
);

-- ===========================================================================
-- photo_group / group_asset：一次快门的逻辑组（raw=RAW 原片 / sooc=机内
-- JPEG / derived=成片派生件）；一资产至多属一组（应用逻辑保证）
-- ===========================================================================
CREATE TABLE IF NOT EXISTS photo_group (
    id INTEGER PRIMARY KEY AUTOINCREMENT
);

CREATE TABLE IF NOT EXISTS group_asset (
    group_id INTEGER NOT NULL REFERENCES photo_group (id) ON DELETE CASCADE,
    asset_id INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    role     TEXT    NOT NULL CHECK (role IN ('raw', 'sooc', 'derived')),
    PRIMARY KEY (group_id, asset_id)
);

CREATE INDEX IF NOT EXISTS idx_group_asset_asset ON group_asset (asset_id);

-- ===========================================================================
-- ai_analysis：AI 辅助选片（每资产每 kind 一行 upsert；eyes=闭眼三态 /
-- blur=清晰度分）。输出只是可筛选建议，与用户决定分层。
-- ===========================================================================
CREATE TABLE IF NOT EXISTS ai_analysis (
    asset_id      INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    kind          TEXT    NOT NULL CHECK (kind IN ('eyes', 'blur')),
    value         TEXT,
    score         REAL,
    model_version TEXT,
    analyzed_at   TEXT    NOT NULL,
    details_json  TEXT,
    PRIMARY KEY (asset_id, kind)
);

-- ===========================================================================
-- edit_recipe / export_job：非破坏编辑配方 + 导出任务账
-- ===========================================================================
CREATE TABLE IF NOT EXISTS edit_recipe (
    asset_id   INTEGER PRIMARY KEY REFERENCES assets (id) ON DELETE CASCADE,
    recipe     TEXT    NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS export_job (
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

CREATE INDEX IF NOT EXISTS idx_export_job_asset  ON export_job (asset_id, id);
CREATE INDEX IF NOT EXISTS idx_export_job_status ON export_job (status, id);

-- ===========================================================================
-- album_export_job：相册/子组「导出为文件夹」任务账（M6，§六 LR 互操作）
-- 同卷硬链接/跨卷拷贝 + 全量 XMP 边车。一行一任务：total=相册内待导出
-- 成员数，done=已导出数（源 missing 跳过不计），linked=其中硬链接数；
-- 遗留 queued/running 行（进程中断）由下一次 run/status 对照内存活跃
-- 集合收尸为 error（无续传语义）。
-- ===========================================================================
CREATE TABLE IF NOT EXISTS album_export_job (
    id          INTEGER PRIMARY KEY,
    album_id    INTEGER NOT NULL REFERENCES album (id) ON DELETE CASCADE,
    subgroup    TEXT,
    output_dir  TEXT    NOT NULL,
    status      TEXT    NOT NULL CHECK (status IN ('queued', 'running', 'done', 'cancelled', 'error')),
    total       INTEGER NOT NULL DEFAULT 0,
    done        INTEGER NOT NULL DEFAULT 0,
    linked      INTEGER NOT NULL DEFAULT 0,
    error       TEXT,
    created_at  TEXT    NOT NULL,
    finished_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_album_export_job_status ON album_export_job (status, id);

-- ===========================================================================
-- asset_metadata：显式库元数据（EXIF 重扫不覆盖的用户编辑真值）
-- ===========================================================================
CREATE TABLE IF NOT EXISTS asset_metadata (
    asset_id INTEGER PRIMARY KEY REFERENCES assets(id) ON DELETE CASCADE,
    value TEXT NOT NULL
);

-- ===========================================================================
-- cull_session / cull_session_asset / cull_decision：选片会话三表
-- ===========================================================================
CREATE TABLE IF NOT EXISTS cull_session (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL,
    scope       TEXT    NOT NULL,
    created_at  TEXT    NOT NULL,
    updated_at  TEXT    NOT NULL,
    finished_at TEXT
);

CREATE TABLE IF NOT EXISTS cull_session_asset (
    session_id INTEGER NOT NULL REFERENCES cull_session (id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL,
    asset_id   INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    PRIMARY KEY (session_id, seq)
);

CREATE INDEX IF NOT EXISTS idx_cull_session_asset_asset ON cull_session_asset (asset_id);

CREATE TABLE IF NOT EXISTS cull_decision (
    session_id INTEGER NOT NULL REFERENCES cull_session (id) ON DELETE CASCADE,
    asset_id   INTEGER NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
    decision   TEXT    NOT NULL CHECK (decision IN ('accepted', 'rejected')),
    origin     TEXT    NOT NULL DEFAULT 'manual' CHECK (origin IN ('manual', 'ai')),
    decided_at TEXT    NOT NULL,
    PRIMARY KEY (session_id, asset_id)
);

CREATE INDEX IF NOT EXISTS idx_cull_decision_session ON cull_decision (session_id);
CREATE INDEX IF NOT EXISTS idx_cull_decision_asset  ON cull_decision (asset_id);

-- ===========================================================================
-- regions / asset_regions：拍摄地图树形地区索引（每资产每层一行挂接）
-- ===========================================================================
CREATE TABLE IF NOT EXISTS regions (
    id        INTEGER PRIMARY KEY,
    parent_id INTEGER REFERENCES regions(id),
    level     INTEGER NOT NULL,
    name      TEXT    NOT NULL,
    code      TEXT,
    lat       REAL    NOT NULL,
    lon       REAL    NOT NULL,
    source    TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_regions_parent ON regions(parent_id, level);
CREATE INDEX IF NOT EXISTS idx_regions_code   ON regions(code);

CREATE TABLE IF NOT EXISTS asset_regions (
    asset_id  INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    region_id INTEGER NOT NULL REFERENCES regions(id),
    level     INTEGER NOT NULL,
    PRIMARY KEY (asset_id, level)
);

CREATE INDEX IF NOT EXISTS idx_asset_regions_region ON asset_regions(region_id, level);
"#;
