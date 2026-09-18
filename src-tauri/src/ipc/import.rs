//! import 命令：启动/暂停/恢复/取消、任务与日志分页、失败重试。
//! 命令层是薄包装，业务在 super 的核心函数（可测）。

use tauri::State;

use super::{
    cancel_import, jobs_page, logs_page, resume_import, retry_failed, set_import_paused,
    start_import, AppState,
};
use crate::db::{JobRow, LogRow};
use crate::import::engine::ImportPlan;

/// 启动导入，返回 job_id。同时只允许一个活跃导入（Busy 错误）。
#[tauri::command]
pub fn import_start(state: State<AppState>, plan: ImportPlan) -> Result<i64, String> {
    start_import(&state, plan)
}

/// 暂停活跃导入（软暂停：当前文件完成后停）。
#[tauri::command]
pub fn import_pause(state: State<AppState>, job_id: i64) -> Result<(), String> {
    set_import_paused(&state, job_id, true)
}

/// 恢复导入（活跃任务清暂停标志；失联暂停的任务从 journal 重建）。
#[tauri::command]
pub fn import_resume(state: State<AppState>, job_id: i64) -> Result<(), String> {
    resume_import(&state, job_id)
}

/// 取消导入（软取消）。
#[tauri::command]
pub fn import_cancel(state: State<AppState>, job_id: i64) -> Result<(), String> {
    cancel_import(&state, job_id)
}

/// 任务列表（keyset 分页：id 严格大于 after，升序）。
#[tauri::command]
pub fn import_jobs_page(
    state: State<AppState>,
    after_id: i64,
    limit: u32,
) -> Result<Vec<JobRow>, String> {
    jobs_page(&state, after_id, limit)
}

/// 任务日志（游标分页：id 严格大于 after_id，升序）。
#[tauri::command]
pub fn import_logs_page(
    state: State<AppState>,
    job_id: i64,
    after_id: i64,
    limit: u32,
) -> Result<Vec<LogRow>, String> {
    logs_page(&state, job_id, after_id, limit)
}

/// 失败重试：失败行复制为 pending 新任务并立即执行，返回新 job_id。
#[tauri::command]
pub fn import_retry_failed(state: State<AppState>, job_id: i64) -> Result<i64, String> {
    retry_failed(&state, job_id)
}
