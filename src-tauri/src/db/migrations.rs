//! 内嵌迁移 SQL：`PRAGMA user_version` 驱动，只加不改（spec §5.4）。
//!
//! 新迁移 = 追加一个 const 并挂到 [`MIGRATIONS`] 末尾，永不修改历史条目。
//! 每条迁移在独立事务中执行（DDL 与 user_version 推进原子提交）。

/// 迁移列表：索引 i 的 SQL 把库从 user_version = i 升到 i + 1。
pub(crate) const MIGRATIONS: &[&str] = &[MIGRATION_0001_INIT];

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
