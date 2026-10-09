//! 照片库命令（photos_libraries 登记表；2026-10-09 单数据库多照片库定案）。
//!
//! photo_library_list / create / remove / relocate 全量落地（db 层
//! [`crate::db::libraries`]：root 互斥校验内置）；M4b 登记管道闭环：
//! create(reference=true) 落批量登记任务行 + kick worker，`run_pending_
//! batch_jobs` 持久化拾取（进度入库/软取消/导入让路/崩溃续跑），scan_
//! status / scan_cancel 返回真值。命令名与负载形状与前端契约
//! `src/ipc/api/types.ts`（PhotoLibrary 等）和封装 `src/ipc/api/library.ts`
//! 一一对应。
//!
//! 铁律：磁盘 IO / DB 批量查询一律 `run_blocking` 后台线程；移除登记
//! 永不删照片文件（用户红线）。

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use super::{run_blocking, AppState, SharedState};
use crate::db::libraries::PhotosLibraryRow;
use crate::events::AppEvent;

/// 照片库登记项 DTO（photo_library_list 返回；photos_libraries 行投影，
/// camelCase 与前端 `PhotoLibrary` 契约对齐）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoLibraryDto {
    /// uuid（登记时生成）。
    pub id: String,
    pub name: String,
    /// 库根目录绝对路径（规范化形态）。
    pub root_path: String,
    /// 登记时间（RFC3339）。
    pub created_at: String,
    /// online=根在盘 | offline=整库离线（reconcile 按照片库粒度判定）。
    pub status: String,
    /// 库内照片数缓存。
    pub asset_count: u64,
    /// 库内照片容量缓存（字节）。
    pub size_bytes: u64,
}

impl From<PhotosLibraryRow> for PhotoLibraryDto {
    fn from(row: PhotosLibraryRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            root_path: row.root_path,
            created_at: row.created_at,
            status: row.status,
            asset_count: row.asset_count,
            size_bytes: row.size_bytes,
        }
    }
}

/// photo_library_remove 结果（camelCase）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoLibraryRemoveResult {
    /// 连带删除的资产记录数（delete_records=false 时为 0）。
    pub records_deleted: u64,
}

/// photo_library_relocate 结果（预检 apply=false / 应用 apply=true 同形）：
/// affected=旧根前缀重写数；unaffected=不在旧根下保持原样数；
/// root_exists=新根当前是否在盘（可先改后挂载，缺失走库 offline）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoLibraryRelocateDto {
    pub affected: u64,
    pub unaffected: u64,
    pub root_exists: bool,
}

/// 照片库扫描任务状态 DTO（photo_library_scan_status 返回）：从文件夹建立的
/// 批量登记与手动放文件的增量扫描共用一条登记管道（单管道串行）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryScanStatusDto {
    pub library_id: String,
    /// idle | running | paused（导入让路）| done | failed。
    pub status: String,
    /// 本轮发现待登记文件数（目录 mtime 剪枝后估算；进度分母）。
    pub total: u64,
    /// 已登记数。
    pub registered: u64,
    /// 跳过数（同库哈希去重/硬链接 file-id 直跳/冷却窗未过/非图片扩展名等）。
    pub skipped: u64,
    pub error: Option<String>,
}

/// 相册名式名称归一：trim 后空串拒绝（错误文案直接面向用户）。
fn validate_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("照片库名称不能为空".into());
    }
    Ok(name.to_string())
}

/// 照片库列表核（photo_library_list）：激活数据库 photos_libraries 表
/// 全量登记（登记序）+ **在线状态即时对账**（§五 M2c 整库 offline 维护：
/// 逐库 stat root 在盘性——外置卷拔插后不等下一轮扫描（60s）即翻
/// status，存储页永远看到真值；有翻转时发 PhotoLibrariesChanged）。
pub fn fetch_photo_library_list(state: &super::AppState) -> Result<Vec<PhotoLibraryDto>, String> {
    let db = super::app_database_db(state)?;
    let mut rows = db.photos_library_list().map_err(|e| e.to_string())?;
    let mut changed = false;
    for row in &mut rows {
        let online = std::path::Path::new(&row.root_path).is_dir();
        let want = if online { "online" } else { "offline" };
        if row.status != want {
            db.photos_library_set_status(&row.id, want)
                .map_err(|e| e.to_string())?;
            row.status = want.to_string();
            changed = true;
        }
    }
    if changed {
        state.bus.publish(AppEvent::PhotoLibrariesChanged);
    }
    Ok(rows.into_iter().map(PhotoLibraryDto::from).collect())
}

/// 照片库列表（photo_library_list）：激活数据库 photos_libraries 表
/// 全量登记（登记序）+ 在线状态即时对账（stat root；DB 访问走后台线程）。
#[tauri::command]
pub async fn photo_library_list(
    state: State<'_, SharedState>,
) -> Result<Vec<PhotoLibraryDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, fetch_photo_library_list).await
}

/// 新建照片库核（photo_library_create）：reference=false 登记空文件夹
///（不存在则创建，存在则必须为空——纯新库）；reference=true 从已有文件夹
/// 建立——只登记不搬文件，同时落批量登记任务行（library_scan_jobs
/// pending）并 kick 扫描 worker 立即拾取（M4b 登记管道：进度入库/可取消/
/// 导入让路；文件绝不移动/重命名）。root 统一走 normalize_library_path
/// 闸门 + root 互斥校验（§八-6：库间互斥、与数据库目录互斥）。
pub fn fetch_photo_library_create(
    state: &super::AppState,
    name: &str,
    root_path: &str,
    reference: bool,
) -> Result<PhotoLibraryDto, String> {
    let name = validate_name(name)?;
    let root_path = crate::settings::normalize_library_path(root_path)?;
    let root = std::path::Path::new(&root_path);
    if reference {
        if !root.is_dir() {
            return Err(format!("文件夹不存在或不可访问：{root_path}"));
        }
    } else {
        match std::fs::read_dir(root) {
            Ok(mut entries) => {
                if entries.next().is_some() {
                    return Err(format!(
                        "文件夹不是空的：{root_path}\n如要登记已有照片，请使用「从已有文件夹建立」"
                    ));
                }
            }
            Err(_) => {
                // 尚不存在：为用户创建空文件夹（前端原生对话框可选未建目录）
                std::fs::create_dir_all(root)
                    .map_err(|e| format!("创建照片库文件夹失败（{root_path}）: {e}"))?;
            }
        }
    }
    let db = super::app_database_db(state)?;
    let row = db.photos_library_register(&name, &root_path, &super::app_database_dir(state)?)?;
    if reference {
        // 从文件夹建立：登记即建任务（持久化——重启后 worker 仍会拾取），
        // kick 让轮询线程秒级开跑（不等 60s 轮询拍）。
        db.library_scan_job_enqueue(&row.id)
            .map_err(|e| e.to_string())?;
        state
            .library_scan_kick
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    Ok(PhotoLibraryDto::from(row))
}

/// 新建照片库（photo_library_create）：reference=false 登记空文件夹；
/// reference=true 从已有文件夹建立（只登记不搬文件；批量登记 M4 闭环）。
#[tauri::command]
pub async fn photo_library_create(
    app: AppHandle,
    state: State<'_, SharedState>,
    name: String,
    root_path: String,
    reference: bool,
) -> Result<PhotoLibraryDto, String> {
    let shared = state.inner().clone();
    let dto = run_blocking(shared, move |state| {
        fetch_photo_library_create(state, &name, &root_path, reference)
    })
    .await?;
    app.emit("app://event", &AppEvent::PhotoLibrariesChanged)
        .map_err(|e| e.to_string())?;
    Ok(dto)
}

/// 移除登记核（photo_library_remove）：永不删照片文件（用户红线）；
/// delete_records=true 连库内资产记录一并删（问过用户后）。库内统计缓存
/// 随登记行消失；相册引用经 FK 级联清理。
pub fn fetch_photo_library_remove(
    state: &super::AppState,
    id: &str,
    delete_records: bool,
) -> Result<PhotoLibraryRemoveResult, String> {
    let db = super::app_database_db(state)?;
    let records_deleted = db.photos_library_remove(id, delete_records)?;
    Ok(PhotoLibraryRemoveResult { records_deleted })
}

/// 移除登记（photo_library_remove）。物理照片永不删；delete_records=true
/// 连库内资产记录一并删。
#[tauri::command]
pub async fn photo_library_remove(
    app: AppHandle,
    state: State<'_, SharedState>,
    id: String,
    delete_records: bool,
) -> Result<PhotoLibraryRemoveResult, String> {
    let shared = state.inner().clone();
    let result = run_blocking(shared, move |state| {
        fetch_photo_library_remove(state, &id, delete_records)
    })
    .await?;
    app.emit("app://event", &AppEvent::PhotoLibrariesChanged)
        .map_err(|e| e.to_string())?;
    Ok(result)
}

/// 照片库整体重定位核（photo_library_relocate）：改登记 + 重写库内路径
/// 前缀（资产归属不变）；apply=false 只预检返回计数。新根不必当前在盘
///（可先改后挂载；缺失走库 offline 语义）。
pub fn fetch_photo_library_relocate(
    state: &super::AppState,
    id: &str,
    new_root_path: &str,
    apply: bool,
) -> Result<PhotoLibraryRelocateDto, String> {
    let new_root = crate::settings::normalize_library_path(new_root_path)?;
    let root_exists = std::path::Path::new(&new_root).is_dir();
    let db = super::app_database_db(state)?;
    let library = db
        .photos_library_get(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("照片库不存在：{id}"))?;
    let same_root = new_root.eq_ignore_ascii_case(&library.root_path);
    let (affected, unaffected) = if apply && !same_root {
        // 先互斥校验（排除自身）再改库 root；路径前缀批量重写同库事务。
        db.photos_library_set_root(id, &new_root, &super::app_database_dir(state)?)?;
        db.rewrite_asset_roots(&library.root_path, &new_root)?
    } else {
        db.inspect_asset_roots(&library.root_path)?
    };
    Ok(PhotoLibraryRelocateDto {
        affected,
        unaffected,
        root_exists,
    })
}

/// 照片库整体重定位（photo_library_relocate）：改登记 + 重写库内路径前缀
///（资产归属不变）；apply=false 只预检返回计数。
#[tauri::command]
pub async fn photo_library_relocate(
    app: AppHandle,
    state: State<'_, SharedState>,
    id: String,
    new_root_path: String,
    apply: bool,
) -> Result<PhotoLibraryRelocateDto, String> {
    let shared = state.inner().clone();
    let dto = run_blocking(shared, move |state| {
        fetch_photo_library_relocate(state, &id, &new_root_path, apply)
    })
    .await?;
    if apply {
        app.emit("app://event", &AppEvent::PhotoLibrariesChanged)
            .map_err(|e| e.to_string())?;
    }
    Ok(dto)
}

/// 各照片库扫描任务状态核（photo_library_scan_status）：library_scan_jobs
/// 行 + 导入旗投影。pending/running → running（cancelling=取消已受理、
/// 当前文件收尾中，同样投影 running）；pending 且导入在场 → paused（§八-5
/// 让路——批量任务排在导入后，导入结束自动续跑）；done/failed 原样。
/// 无任务行的库不返回（前端「无记录 = 无任务信息」自然降级 idle）。
pub fn fetch_photo_library_scan_status(
    state: &super::AppState,
) -> Result<Vec<LibraryScanStatusDto>, String> {
    let db = super::app_database_db(state)?;
    let import_running = state
        .import_running
        .load(std::sync::atomic::Ordering::SeqCst);
    let rows = db.library_scan_job_list().map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|job| {
            let status = match job.status.as_str() {
                "pending" if import_running => "paused",
                "pending" | "running" | "cancelling" => "running",
                "failed" => "failed",
                // done（及未来扩展状态）
                _ => "done",
            };
            LibraryScanStatusDto {
                library_id: job.library_id,
                status: status.to_string(),
                total: job.total,
                registered: job.registered,
                skipped: job.skipped,
                error: job.error,
            }
        })
        .collect())
}

/// 各照片库扫描任务状态（photo_library_scan_status；DB 访问走后台线程）。
#[tauri::command]
pub async fn photo_library_scan_status(
    state: State<'_, SharedState>,
) -> Result<Vec<LibraryScanStatusDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, fetch_photo_library_scan_status).await
}

/// 取消某库扫描任务核（photo_library_scan_cancel；软信号）：pending 行
/// 直接删（任务尚未开跑即消失）；running 行置 cancelling——worker 文件
/// 边界响应后删行（「当前目录登记完即停」）；无任务行/已收尾 no-op。
pub fn fetch_photo_library_scan_cancel(
    state: &super::AppState,
    library_id: &str,
) -> Result<(), String> {
    let db = super::app_database_db(state)?;
    db.library_scan_job_cancel(library_id)
        .map_err(|e| e.to_string())
}

/// 取消某库在跑的扫描任务（photo_library_scan_cancel；软信号）。
#[tauri::command]
pub async fn photo_library_scan_cancel(
    state: State<'_, SharedState>,
    library_id: String,
) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_photo_library_scan_cancel(state, &library_id)
    })
    .await
}

// ---------------------------------------------------------------------------
// 增量扫描编排（M2b，§三/§八 1/2/3/5/7；核心实现 crate::scan）
// ---------------------------------------------------------------------------

/// 库扫描轮询间隔：目录 mtime 剪枝下一轮空转成本 ≈ 每目录一次 stat，
/// 温和频率即可「手动放文件 → 一分钟内自动登记」；冷却窗 10s 保证大
/// 文件夹拷贝期间逐步拾取、最终收敛。
pub const LIBRARY_SCAN_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// 单轮全库扫描编排（M2b 数据底座；M4 的进度/取消 UI 挂在同一条线上）：
/// 导入在场 → 让路整轮跳过（§八-5/索引让路闸同骨架）；否则逐库跑
/// [`crate::scan::scan_library_once`]，单库失败记日志不炸整轮。
/// 返回 (library_id, 报告) 列表。
pub fn scan_all_libraries_once(state: &AppState) -> Vec<(String, crate::scan::LibraryScanReport)> {
    // 登记让路（§八-5）：导入落盘产出与扫描发现共用单管道，导入在场时
    // 本轮让路——下轮再拾（冷却窗兜住刚落盘文件，无死区）。
    if state
        .import_running
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Vec::new();
    }
    let Ok(db_dir) = super::app_database_dir(state) else {
        eprintln!("[library-scan] 数据库不可用，跳过本轮");
        return Vec::new();
    };
    let Ok(db) = super::open_library_db(&db_dir) else {
        eprintln!("[library-scan] 数据库不可用，跳过本轮");
        return Vec::new();
    };
    let Ok(libraries) = db.photos_library_list() else {
        eprintln!("[library-scan] 照片库登记表不可读，跳过本轮");
        return Vec::new();
    };
    let gate = std::sync::Arc::clone(&state.register_gate);
    let options = crate::scan::ScanOptions::default();
    let mut out = Vec::new();
    for library in &libraries {
        match crate::scan::scan_library_once(
            &db,
            &db_dir,
            library,
            &options,
            Some(&gate),
            Some(&state.bus),
        ) {
            Ok(report) => out.push((library.id.clone(), report)),
            Err(error) => eprintln!("[library-scan] 库「{}」扫描失败: {error}", library.name),
        }
    }
    out
}

/// 后台库扫描轮询线程（supervisor 派发）：启动即扫第一轮（既有库的
/// missing/边车状态随启动收敛 + 批量登记任务续跑——持久化任务行由
/// [`run_pending_batch_jobs`] 拾取），随后每 [`LIBRARY_SCAN_POLL_INTERVAL`]
/// 一轮；导入在场让路由 [`scan_all_libraries_once`] 内建；kick（从文件夹
/// 建立新照片库）立即打断等待秒级开跑。
pub fn spawn_library_scan_worker(
    state: super::SharedState,
    supervisor: &std::sync::Arc<crate::tasks::TaskSupervisor>,
) {
    supervisor.spawn("scan", "library-scan-poller".into(), move |controls| {
        loop {
            // 批量登记任务先行（M4b）：worker 单线程串行即登记单管道的进程
            // 内形态（批量在场时增量轮排队其后；跨线程与导入落盘的登记段
            // 互斥由 register_gate 保证，§八-5）
            run_pending_batch_jobs(&state);
            for (library_id, report) in scan_all_libraries_once(&state) {
                eprintln!(
                    "[library-scan] {library_id}: 登记 {} 跳过 {} 重绑 {} 恢复 {} 重算 {} 标缺 {} 边车 {}",
                    report.registered,
                    report.skipped_event_total(),
                    report.rebound,
                    report.restored,
                    report.recomputed,
                    report.missing_marked,
                    report.sidecar_read
                );
            }
            // 软退出轮：每秒醒来一次便于取消响应；kick 打断等待（新任务）
            for _ in 0..LIBRARY_SCAN_POLL_INTERVAL.as_secs() {
                if controls.is_cancelled() {
                    return;
                }
                if state
                    .library_scan_kick
                    .swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    break;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    });
}

// ---------------------------------------------------------------------------
// 批量登记任务编排（M4b「从文件夹建立」§三；核心 crate::scan::scan_library_batch）
// ---------------------------------------------------------------------------

/// library_scan_jobs 行的进度入库 sink：行计数是**跨轮累计绝对值**（续跑
/// 时进度条从上次位置继续，不从零跳变）——持有本轮基线（pickup 行值，
/// 含上一进程崩溃前的进度），扫描推进时累加报告增量落库。
struct JobProgressSink<'a> {
    db: &'a crate::db::Db,
    library_id: String,
    /// 本轮基线（任务行 pickup 时的累计值）。
    base_registered: u64,
    base_skipped: u64,
    /// 预点数分母（on_total 前为上一轮存量值）。
    total: u64,
}

impl crate::scan::BatchProgressSink for JobProgressSink<'_> {
    fn on_total(&mut self, total: u64) {
        self.total = total;
        let _ = self.db.library_scan_job_progress(
            &self.library_id,
            self.total,
            self.base_registered,
            self.base_skipped,
        );
    }

    fn on_progress(&mut self, report: &crate::scan::LibraryScanReport) {
        // 口径与事件一致：registered 含重绑，skipped 含冷却
        let _ = self.db.library_scan_job_progress(
            &self.library_id,
            self.total,
            self.base_registered + report.registered + report.rebound,
            self.base_skipped + report.skipped_event_total(),
        );
    }
}

/// 拾取并串行跑完全部待处理批量登记任务（worker 每轮调用；导入让路→任务
/// 回队 pending、进度经 [`JobProgressSink`] 入库、软取消→任务行删除、崩溃
/// 遗留 running 行续跑）。冷却窗用生产默认（刚写的文件下轮拾取——轮询
/// 周期 60s > 冷却窗 10s，自然收敛）。
pub fn run_pending_batch_jobs(state: &AppState) {
    run_pending_batch_jobs_with(state, &crate::scan::ScanOptions::default());
}

/// 同上，测试注入扫描选项（冷却窗关闭等；生产行为走 [`run_pending_batch_jobs`]）。
pub fn run_pending_batch_jobs_with(state: &AppState, options: &crate::scan::ScanOptions) {
    // 导入在场整轮让路（§八-5）：任务保持 pending（状态查询投影 paused），
    // 导入结束后的下一轮拾取
    if state
        .import_running
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return;
    }
    let Ok(db_dir) = super::app_database_dir(state) else {
        eprintln!("[library-scan] 数据库不可用，批量登记任务本轮跳过");
        return;
    };
    let Ok(db) = super::open_library_db(&db_dir) else {
        eprintln!("[library-scan] 数据库不可用，批量登记任务本轮跳过");
        return;
    };
    let Ok(jobs) = db.library_scan_jobs_pickup() else {
        eprintln!("[library-scan] 批量登记任务表不可读，本轮跳过");
        return;
    };
    let gate = std::sync::Arc::clone(&state.register_gate);
    for job in jobs {
        // 每任务前重查导入在场（上一任务跑的当口用户可能开了导入）
        if state
            .import_running
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        // 库行已移除（移除登记的孤儿任务，随删）
        let Some(library) = db.photos_library_get(&job.library_id).ok().flatten() else {
            let _ = db.library_scan_job_delete(&job.library_id);
            continue;
        };
        // 条件标记 running = 取消竞态闸：软取消先行（删行/置 cancelling）
        // → 0 行受影响，放弃拾取
        if !db
            .library_scan_job_mark_running(&job.library_id)
            .unwrap_or(false)
        {
            continue;
        }
        // 软取消轮询（核心每文件边界调一次）：任务行非 running（cancelling
        // /已被收尾）或行消失 → 停。显式持引用捕获（move 会搬走 Db 本体）
        let cancel_db = &db;
        let cancel_id = job.library_id.clone();
        let cancel_requested = move || {
            cancel_db
                .library_scan_job_get(&cancel_id)
                .ok()
                .flatten()
                .is_none_or(|row| row.status != "running")
        };
        let signals = crate::scan::BatchSignals {
            cancel_requested: Some(&cancel_requested),
            yield_now: Some(state.import_running.as_ref()),
        };
        let mut sink = JobProgressSink {
            db: &db,
            library_id: job.library_id.clone(),
            base_registered: job.registered,
            base_skipped: job.skipped,
            total: job.total,
        };
        let run = crate::scan::scan_library_batch(
            &db,
            &db_dir,
            &library,
            options,
            Some(&gate),
            Some(&state.bus),
            signals,
            Some(&mut sink),
        );
        match run {
            Ok(run) => {
                let registered = job.registered + run.report.registered + run.report.rebound;
                let skipped = job.skipped + run.report.skipped_event_total();
                match run.outcome {
                    crate::scan::BatchOutcome::Completed => {
                        let _ = db.library_scan_job_finish(
                            &job.library_id,
                            "done",
                            sink.total,
                            registered,
                            skipped,
                            None,
                        );
                    }
                    // 软取消收尾：任务行删除（状态归 idle）；未登记文件由
                    // 增量扫描常态拾取（单管道哲学 §八-5，取消不是封禁）
                    crate::scan::BatchOutcome::Cancelled => {
                        let _ = db.library_scan_job_delete(&job.library_id);
                    }
                    // 导入让路/库根离线：回队 pending（计数保留，进度连续）
                    crate::scan::BatchOutcome::Yielded | crate::scan::BatchOutcome::Offline => {
                        let _ = db.library_scan_job_requeue(&job.library_id);
                    }
                }
                eprintln!(
                    "[library-scan] 批量登记「{}」: {:?}（登记 {} 跳过 {}）",
                    library.name, run.outcome, registered, skipped
                );
            }
            Err(error) => {
                // 行留 failed（状态查询可见错误文案；下轮 pickup 自愈重试）
                let _ = db.library_scan_job_finish(
                    &job.library_id,
                    "failed",
                    sink.total,
                    job.registered,
                    job.skipped,
                    Some(&error),
                );
                eprintln!("[library-scan] 批量登记「{}」失败: {error}", library.name);
            }
        }
    }
}
