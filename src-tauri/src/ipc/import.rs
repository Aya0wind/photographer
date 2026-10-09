//! import 命令：启动/暂停/恢复/取消、任务与日志分页、失败重试、安全清卡。
//!
//! 铁律（2026-09-18）：涉及磁盘 IO / WPD COM / 网络(NAS) / DB 查询的命令
//! 一律 async + spawn_blocking 后台执行；pause/cancel 仅置原子标志 +
//! 总线发布（纯内存），保留同步。命令层是薄包装，业务在 super 的核心
//! 函数（可测，签名不变）。

use tauri::State;

use super::{
    apply_clean, cancel_import, jobs_page, list_clean_candidates, logs_page, resume_import,
    retry_failed, run_blocking, set_import_paused, start_import, SharedState,
};
use crate::db::{JobRow, LogRow};
use crate::import::clean::{CleanCandidateDto, CleanResultDto};
use crate::import::engine::ImportPlan;

/// 启动导入，返回 job_id。同时只允许一个活跃导入（Busy 错误）。
/// begin 阶段同步做源枚举（MTP 秒级/NAS 目录遍历）+ journal 写入 →
/// 后台线程执行；engine.run 本就在独立线程。
#[tauri::command]
pub async fn import_start(state: State<'_, SharedState>, plan: ImportPlan) -> Result<i64, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| start_import(state, plan)).await
}

/// 暂停活跃导入（软暂停：当前文件完成后停）。纯原子操作，保留同步。
#[tauri::command]
pub fn import_pause(state: State<SharedState>, job_id: i64) -> Result<(), String> {
    set_import_paused(&state, job_id, true)
}

/// 恢复导入（活跃任务清暂停标志；失联暂停的任务从 journal 重建）。
/// journal 全量读 + FOLDER 源 canonicalize（NAS 网络往返）→ 后台线程。
#[tauri::command]
pub async fn import_resume(state: State<'_, SharedState>, job_id: i64) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| resume_import(state, job_id)).await
}

/// 取消导入（软取消）。纯原子操作，保留同步。
#[tauri::command]
pub fn import_cancel(state: State<SharedState>, job_id: i64) -> Result<(), String> {
    cancel_import(&state, job_id)
}

/// 任务列表（keyset 分页：id 严格大于 after，升序）。
/// DB 查询（busy_timeout 最长 5s）→ 后台线程。
#[tauri::command]
pub async fn import_jobs_page(
    state: State<'_, SharedState>,
    after_id: i64,
    limit: u32,
) -> Result<Vec<JobRow>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| jobs_page(state, after_id, limit)).await
}

/// 任务日志（游标分页：id 严格大于 after_id，升序）。DB 查询 → 后台线程。
#[tauri::command]
pub async fn import_logs_page(
    state: State<'_, SharedState>,
    job_id: i64,
    after_id: i64,
    limit: u32,
) -> Result<Vec<LogRow>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        logs_page(state, job_id, after_id, limit)
    })
    .await
}

/// 失败重试：失败行复制为 pending 新任务并立即执行，返回新 job_id。
/// journal 读写 + 源枚举 → 后台线程。
#[tauri::command]
pub async fn import_retry_failed(
    state: State<'_, SharedState>,
    job_id: i64,
) -> Result<i64, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| retry_failed(state, job_id)).await
}

/// M2 F1 安全清卡：列出可清理候选（设备全量 list + DB）→ 后台线程。
#[tauri::command]
pub async fn clean_candidates(
    state: State<'_, SharedState>,
    job_id: i64,
) -> Result<Vec<CleanCandidateDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| list_clean_candidates(state, job_id)).await
}

/// M2 F1 安全清卡：逐文件复验（重读源全流算 size+xxh64）一致才删。
/// 全量读盘 + 删除 → 后台线程。
#[tauri::command]
pub async fn clean_apply(
    state: State<'_, SharedState>,
    job_id: i64,
) -> Result<CleanResultDto, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| apply_clean(state, job_id)).await
}

/// 删除任务历史核：终态才可删（进行中明确拒绝）；jobs/job_files/logs 三清
/// （job_files 经 FK 级联，logs 显式删——它不属于 journal 无 FK）。
pub fn fetch_import_job_delete(state: &super::AppState, job_id: i64) -> Result<(), String> {
    let db = super::app_database_db(state)?;
    match db.delete_job_history(job_id).map_err(|e| e.to_string())? {
        Ok(_) => Ok(()),
        Err(message) => Err(message),
    }
}

/// 删除任务历史（仅终态；进行中返回 Err）。DB 写 → 后台线程。
#[tauri::command]
pub async fn import_job_delete(state: State<'_, SharedState>, job_id: i64) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| fetch_import_job_delete(state, job_id)).await
}
