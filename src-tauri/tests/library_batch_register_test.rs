//! 从文件夹建立批量登记闭环（M4b，计划 §三「从已有文件夹建立」+ §四
//! 跨库重复总结）：持久化任务行（进度入库/崩溃续跑累计）、软取消（拾取
//! 前/跑动中）、导入让路（任务回队 + 状态投影 paused）、库统计缓存随登记
//! 维护；两级识别第二级——同库内容哈希去重 skip（第一级 file-id 硬链接
//! 直跳见 library_scan_test）。

mod common;

pub use common::{
    ai, bursts, db, devices, events, geo, import, index, ipc, metadata, platform,
    scan, settings, tasks, thumbs,
};

use std::cell::Cell;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use common::{open_db, state_with_library};
use ipc::photo_library::{
    fetch_photo_library_create, fetch_photo_library_scan_cancel, fetch_photo_library_scan_status,
    run_pending_batch_jobs_with,
};

/// 冷却关闭的扫描选项（文件 mtime 判定直通；两级识别不受影响）。
fn relaxed() -> scan::ScanOptions {
    scan::ScanOptions {
        cooldown: Duration::ZERO,
        now: None,
    }
}

/// 唯一内容的合法 JPEG（魔数 + 唯一化字节；同 tag = 同 (size, xxhash)）。
fn jpg(tag: u32) -> Vec<u8> {
    let mut content = common::shrink(vec![0xFF, 0xD8, 0xFF, 0xE0], 4096);
    content[10..14].copy_from_slice(&tag.to_be_bytes());
    content
}

fn write(path: &Path, tag: u32) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, jpg(tag)).unwrap();
}

/// 测试夹具：临时 db 目录 + AppState（应用唯一数据库 = config_dir）+ 预建
/// 源目录（state_with_library 需要）。
fn setup() -> (tempfile::TempDir, ipc::AppState, std::path::PathBuf) {
    let holder = tempfile::tempdir().unwrap();
    let db_dir = holder.path().join("db");
    let src = holder.path().join("src");
    for dir in [&db_dir, &src] {
        fs::create_dir_all(dir).unwrap();
    }
    let state = state_with_library(&db_dir, &src, Duration::from_millis(1));
    (holder, state, db_dir)
}

fn count_assets(db: &db::Db) -> i64 {
    db.0
        .query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0))
        .unwrap()
}

fn job_row(db: &db::Db, library_id: &str) -> db::libraries::LibraryScanJobRow {
    db.library_scan_job_get(library_id).unwrap().unwrap()
}

// ---------------------------------------------------------------------------
// 文件夹登记闭环：create(reference) 落任务行 + kick → worker 跑到 done
// ---------------------------------------------------------------------------

#[test]
fn from_folder_registration_runs_to_done_with_stats_and_event() {
    let (holder, state, db_dir) = setup();
    let photos = holder.path().join("photos");
    // 已有文件夹：嵌套目录 + 图片/JPEG/RAW + 不可识别扩展名（不计数）
    write(&photos.join("2024/05/a.jpg"), 1);
    write(&photos.join("2024/06/b.nef"), 2);
    fs::write(photos.join("2024/05/readme.txt"), b"not a photo").unwrap();

    let mut rx = state.bus.subscribe();
    let library = fetch_photo_library_create(
        &state,
        "从文件夹",
        &photos.to_string_lossy(),
        true,
    )
    .unwrap();

    // 任务行已落（pending）+ worker 唤醒旗已置（秒级开跑，不等 60s 轮询拍）
    let database = open_db(&db_dir);
    assert_eq!(job_row(&database, &library.id).status, "pending");
    assert!(state.library_scan_kick.load(Ordering::SeqCst));
    // pending 投影 running（已排队即跑）
    let rows = fetch_photo_library_scan_status(&state).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].library_id, library.id);
    assert_eq!(rows[0].status, "running");

    run_pending_batch_jobs_with(&state, &relaxed());

    let job = job_row(&database, &library.id);
    assert_eq!(job.status, "done");
    assert_eq!(job.registered, 2);
    assert_eq!(job.skipped, 0);
    assert_eq!(job.total, 2, "预点数只认可识别图片扩展名");
    assert_eq!(count_assets(&database), 2);
    // 库统计缓存随登记维护（§二 asset_count/size_bytes）
    let lib = database.photos_library_get(&library.id).unwrap().unwrap();
    assert_eq!(lib.asset_count, 2);
    assert_eq!(lib.size_bytes, 8192);
    // 收尾事件（registered/skipped；无跨库重复不带扩展字段）
    let mut finished = false;
    while let Ok(event) = rx.try_recv() {
        if let events::AppEvent::LibraryScanFinished {
            library_id,
            registered,
            skipped,
            cross_library_duplicates,
        } = event
        {
            if library_id == library.id {
                assert_eq!(registered, 2);
                assert_eq!(skipped, 0);
                assert_eq!(cross_library_duplicates, None);
                finished = true;
            }
        }
    }
    assert!(finished, "批量收尾应发 LibraryScanFinished");
    // done 行不再拾取（幂等：再跑一轮零动作）
    run_pending_batch_jobs_with(&state, &relaxed());
    assert_eq!(count_assets(&database), 2);
    // done 状态原样投影
    assert_eq!(
        fetch_photo_library_scan_status(&state).unwrap()[0].status,
        "done"
    );
}

// ---------------------------------------------------------------------------
// 两级识别第二级（§三）：同库同内容（不同文件）→ 哈希去重 skip
// ---------------------------------------------------------------------------

#[test]
fn same_content_copy_within_library_skipped_by_hash() {
    let (holder, state, db_dir) = setup();
    let photos = holder.path().join("photos");
    write(&photos.join("a.jpg"), 1);
    write(&photos.join("a-copy.jpg"), 1); // 同内容副本（新 inode：file-id 不命中）
    write(&photos.join("b.jpg"), 2);

    let library =
        fetch_photo_library_create(&state, "去重", &photos.to_string_lossy(), true).unwrap();
    run_pending_batch_jobs_with(&state, &relaxed());

    let database = open_db(&db_dir);
    let job = job_row(&database, &library.id);
    assert_eq!(job.status, "done");
    assert_eq!(job.registered, 2, "唯一内容各登记一行");
    assert_eq!(job.skipped, 1, "同库同内容副本 skip（duplicatePolicy）");
    // 被跳过的路径对库完全不可见（§三）
    assert_eq!(count_assets(&database), 2);
}

// ---------------------------------------------------------------------------
// 软取消：拾取前取消 = 任务直接消失
// ---------------------------------------------------------------------------

#[test]
fn cancel_before_pickup_deletes_job_without_registering() {
    let (holder, state, db_dir) = setup();
    let photos = holder.path().join("photos");
    write(&photos.join("a.jpg"), 1);

    let library =
        fetch_photo_library_create(&state, "取消", &photos.to_string_lossy(), true).unwrap();
    fetch_photo_library_scan_cancel(&state, &library.id).unwrap();

    let database = open_db(&db_dir);
    assert!(
        database.library_scan_job_get(&library.id).unwrap().is_none(),
        "pending 行直接删（未开跑即消失）"
    );
    run_pending_batch_jobs_with(&state, &relaxed());
    assert_eq!(count_assets(&database), 0);
    assert!(
        fetch_photo_library_scan_status(&state)
            .unwrap()
            .is_empty(),
        "无任务行 = 无任务信息（前端自然降级 idle）"
    );
}

// ---------------------------------------------------------------------------
// 软取消：跑动中文件边界停（核心级；当前文件处理完即停）
// ---------------------------------------------------------------------------

#[test]
fn batch_core_stops_on_cancel_and_yields_to_import() {
    let (holder, _state, db_dir) = setup();
    let photos = holder.path().join("photos");
    write(&photos.join("a.jpg"), 1);
    write(&photos.join("b.jpg"), 2);
    write(&photos.join("c.jpg"), 3);

    let database = open_db(&db_dir);
    let library = database
        .photos_library_register("核心取消", &photos.to_string_lossy(), &db_dir)
        .unwrap();

    // 取消轮询：前两次调用放行（walk 入口 + 首文件边界），第三次起取消
    // → 首文件登记完毕、余下留给下轮
    let calls = Cell::new(0u32);
    let cancel = || {
        calls.set(calls.get() + 1);
        calls.get() > 2
    };
    let signals = scan::BatchSignals {
        cancel_requested: Some(&cancel),
        yield_now: None,
    };
    let run = scan::scan_library_batch(
        &database,
        &db_dir,
        &library,
        &relaxed(),
        None,
        None,
        signals,
        None,
    )
    .unwrap();
    assert_eq!(run.outcome, scan::BatchOutcome::Cancelled);
    assert_eq!(run.report.registered, 1, "当前文件处理完即停");
    assert_eq!(count_assets(&database), 1);

    // 导入让路旗置位 → 未处理任何新文件即 Yielded（已有资产短路直过）
    let import = AtomicBool::new(true);
    let signals = scan::BatchSignals {
        cancel_requested: None,
        yield_now: Some(&import),
    };
    let library = database.photos_library_get(&library.id).unwrap().unwrap();
    let run = scan::scan_library_batch(
        &database,
        &db_dir,
        &library,
        &relaxed(),
        None,
        None,
        signals,
        None,
    )
    .unwrap();
    assert_eq!(run.outcome, scan::BatchOutcome::Yielded);
    assert_eq!(run.report.registered, 0);
    assert_eq!(count_assets(&database), 1, "让路不丢已登记数据");
}

// ---------------------------------------------------------------------------
// 导入让路投影 paused + 崩溃续跑进度累计（持久化任务行）
// ---------------------------------------------------------------------------

#[test]
fn import_running_yields_projects_paused_and_resumes_accumulated() {
    let (holder, state, db_dir) = setup();
    let photos = holder.path().join("photos");
    write(&photos.join("a.jpg"), 1);

    let library =
        fetch_photo_library_create(&state, "让路", &photos.to_string_lossy(), true).unwrap();
    let database = open_db(&db_dir);

    // 导入在场：整轮让路，任务保持 pending，状态投影 paused
    state.import_running.store(true, Ordering::SeqCst);
    run_pending_batch_jobs_with(&state, &relaxed());
    assert_eq!(count_assets(&database), 0);
    let rows = fetch_photo_library_scan_status(&state).unwrap();
    assert_eq!(rows[0].status, "paused", "pending + 导入在场 → paused");
    assert_eq!(rows[0].total, 0, "尚未预点数");

    // 模拟崩溃续跑：上一进程已登记 a.jpg 且进度已入库（running + 计数 1），
    // 崩溃后又放入 b.jpg
    let library_row = database.photos_library_get(&library.id).unwrap().unwrap();
    scan::scan_library_once(&database, &db_dir, &library_row, &relaxed(), None, None).unwrap();
    write(&photos.join("b.jpg"), 2);
    database
        .library_scan_job_mark_running(&library.id)
        .unwrap();
    database
        .library_scan_job_progress(&library.id, 1, 1, 0)
        .unwrap();

    // 导入结束 → 拾取 running 遗留行续跑，计数在基线上累计（进度条不回跳）
    state.import_running.store(false, Ordering::SeqCst);
    run_pending_batch_jobs_with(&state, &relaxed());
    let job = job_row(&database, &library.id);
    assert_eq!(job.status, "done");
    assert_eq!(job.registered, 2, "1 基线 + 1 新登记");
    assert_eq!(job.total, 2, "预点数含崩溃后新放入的文件");
    assert_eq!(count_assets(&database), 2);
}

// ---------------------------------------------------------------------------
// 跨库重复（§四）：照常登记不去重，收尾总结「N 张与其他照片库内容相同」
// ---------------------------------------------------------------------------

#[test]
fn cross_library_duplicate_registers_and_summarizes() {
    let (holder, state, db_dir) = setup();
    let photos_a = holder.path().join("lib-a");
    let photos_b = holder.path().join("lib-b");
    write(&photos_a.join("same.jpg"), 7);
    write(&photos_b.join("same.jpg"), 7); // 与库 A 同内容（跨库重复）
    write(&photos_b.join("unique.jpg"), 8);

    let mut rx = state.bus.subscribe();
    let lib_a =
        fetch_photo_library_create(&state, "库A", &photos_a.to_string_lossy(), true).unwrap();
    run_pending_batch_jobs_with(&state, &relaxed());
    let lib_b =
        fetch_photo_library_create(&state, "库B", &photos_b.to_string_lossy(), true).unwrap();
    run_pending_batch_jobs_with(&state, &relaxed());

    let database = open_db(&db_dir);
    // 跨库不去重（§四 数据层定案）：3 行独立资产
    assert_eq!(count_assets(&database), 3);
    let job_b = job_row(&database, &lib_b.id);
    assert_eq!(job_b.registered, 2, "跨库同内容照常登记");
    // 收尾总结：库 B 的 Finished 带跨库重复计数（registered 的子集）
    let mut summary_b: Option<u64> = None;
    let mut summary_a: Option<u64> = None;
    while let Ok(event) = rx.try_recv() {
        if let events::AppEvent::LibraryScanFinished {
            library_id,
            cross_library_duplicates,
            ..
        } = event
        {
            if library_id == lib_a.id {
                summary_a = cross_library_duplicates;
            } else if library_id == lib_b.id {
                summary_b = cross_library_duplicates;
            }
        }
    }
    assert_eq!(summary_b, Some(1), "库 B 收尾提示 1 张与库 A 内容相同");
    assert_eq!(summary_a, None, "库 A 先建时无他库可重复");
}

// ---------------------------------------------------------------------------
// 库根离线：任务保持 pending（根回来续跑收敛，不失败不清零）
// ---------------------------------------------------------------------------

#[test]
fn offline_root_keeps_job_pending_for_resume() {
    let (holder, state, db_dir) = setup();
    let photos = holder.path().join("photos");
    write(&photos.join("a.jpg"), 1);

    let library =
        fetch_photo_library_create(&state, "离线", &photos.to_string_lossy(), true).unwrap();
    fs::remove_dir_all(&photos).unwrap();
    run_pending_batch_jobs_with(&state, &relaxed());

    let database = open_db(&db_dir);
    assert_eq!(
        job_row(&database, &library.id).status,
        "pending",
        "根离线不消耗任务"
    );
    assert_eq!(count_assets(&database), 0);
    assert_eq!(
        database.photos_library_get(&library.id).unwrap().unwrap().status,
        "offline"
    );

    // 根回来 → 任务续跑收敛（online 翻回 + 登记）
    write(&photos.join("a.jpg"), 1);
    run_pending_batch_jobs_with(&state, &relaxed());
    assert_eq!(job_row(&database, &library.id).status, "done");
    assert_eq!(count_assets(&database), 1);
}
