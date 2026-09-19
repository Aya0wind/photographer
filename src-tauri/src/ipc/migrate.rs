//! migrate 命令（M3，spec §5.11）：dbDir 两阶段迁移 / photoRoot 切换。
//!
//! 快验证（库存在/路径合法/守卫登记）在后台线程完成后立即返回；复制/移动
//! 主体由 TaskSupervisor 派发，进度与结果经 migration* 事件回报——磁盘
//! 大 IO 命令铁律：不阻塞调用线程，更不上主线程。

use tauri::State;

use super::{run_blocking, SharedState};
use crate::migrate;

/// dbDir 迁移（两阶段：复制+逐文件校验 journal 断点可恢复 → 原子改注册表
/// + 旧目录 `.migrated-bak` 标记）。同参重调 = 断点恢复。迁移期间该库拒绝
/// 新导入。完成/失败经 migrationFinished 事件回报。
#[tauri::command]
pub async fn db_dir_migrate(
    state: State<'_, SharedState>,
    library_id: String,
    new_dir: String,
) -> Result<(), String> {
    let shared = state.inner().clone();
    let for_task = std::sync::Arc::clone(&shared);
    run_blocking(shared, move |_| {
        migrate::db_dir_migrate(&for_task, &library_id, &new_dir)
    })
    .await
}

/// photoRoot 切换。mode=switch 仅改配置（旧照片转 external 只读语义，
/// 零物理变化）；mode=migrate 批量移动 + 资产路径更新（journal 断点恢复，
/// 同参重调续跑）。进度经 migration* 事件回报。
#[tauri::command]
pub async fn photo_root_switch(
    state: State<'_, SharedState>,
    library_id: String,
    new_root: String,
    mode: String,
) -> Result<(), String> {
    let shared = state.inner().clone();
    let for_task = std::sync::Arc::clone(&shared);
    run_blocking(shared, move |_| {
        migrate::photo_root_switch(&for_task, &library_id, &new_root, &mode)
    })
    .await
}
