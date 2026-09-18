//! SQLite 基础 + journal（spec §5.4）：M1 T2 由核心 lane 实现。
//! WAL、迁移（user_version 驱动，只加不改）、jobs/job_files/assets/logs 仓储。
