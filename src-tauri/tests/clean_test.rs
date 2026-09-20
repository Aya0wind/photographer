//! M2 F1 安全清卡测试：候选列表（已校验 + 源仍在设备）→ 删前逐文件复验
//! （size+xxh64 与库中指纹一致才删）→ 每文件日志 + 事件 + IPC 编排。

mod common;

pub use common::{
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::fs;
use std::path::Path;
use std::time::Duration;

use common::{build_source, open_db, plan_for, state_with_library, wait_done};
use devices::volume::VolumeSource;
use events::{AppEvent, EventBus, FileState};
use import::clean::{clean_apply, clean_candidates, CleanCandidateDto};
use import::engine::Engine;
use ipc::{apply_clean, list_clean_candidates, start_import};

// ---------------------------------------------------------------------------
// 测试脚手架
// ---------------------------------------------------------------------------

/// 复制导入 3 文件，返回 (job_id, files)。
fn import_all(src_dir: &Path, db_dir: &Path, target_dir: &Path) -> (i64, Vec<(String, Vec<u8>)>) {
    let files = build_source(src_dir);
    let db = open_db(db_dir);
    let mut engine = Engine::new(
        db,
        EventBus::new(),
        Box::new(VolumeSource::new(src_dir)),
        plan_for(target_dir),
    );
    let job_id = engine.begin().unwrap();
    let stats = engine.run();
    assert_eq!(stats.done_files, 3, "前置导入须全部成功: {stats:?}");
    (job_id, files)
}

fn src_path(src_dir: &Path, rel: &str) -> std::path::PathBuf {
    src_dir.join(rel.replace('/', "\\"))
}

fn job_logs(db: &db::Db, job_id: i64, level: &str) -> Vec<String> {
    db.0.prepare("SELECT message FROM logs WHERE job_id = ?1 AND level = ?2")
        .unwrap()
        .query_map(rusqlite::params![job_id, level], |r| r.get::<_, String>(0))
        .unwrap()
        .filter_map(Result::ok)
        .collect()
}

// ---------------------------------------------------------------------------
// F1 用例
// ---------------------------------------------------------------------------

#[test]
fn candidates_list_only_verified_present_files() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let (job_id, files) = import_all(src.path(), db_dir.path(), target.path());

    // 候选排除项：文件 0 的源已消失；文件 1 的 journal 行被置为 failed
    fs::remove_file(src_path(src.path(), &files[0].0)).unwrap();
    let db = open_db(db_dir.path());
    db.upsert_job_file(&db::JobFileRow {
        job_id,
        src: files[1].0.clone(),
        dst: String::new(),
        size: files[1].1.len() as u64,
        state: FileState::Failed,
        error: Some("人工置失败".into()),
        xxhash: None,
        dst2: String::new(),
    })
    .unwrap();

    let source = VolumeSource::new(src.path());
    let candidates = clean_candidates(&db, &source, job_id).unwrap();
    assert_eq!(candidates.len(), 1, "仅剩文件 2 是合法候选: {candidates:?}");
    let candidate = &candidates[0];
    assert_eq!(candidate.src, files[2].0);
    assert_eq!(candidate.rel_path, files[2].0);
    assert_eq!(candidate.size as usize, files[2].1.len());
    assert!(candidate.asset_id > 0, "必须携带库内资产 id");

    // camelCase 负载契约
    let json = serde_json::to_value(candidate).unwrap();
    assert!(json.get("relPath").is_some(), "{json}");
    assert!(json.get("assetId").is_some(), "{json}");

    // 任务不存在 → 报错
    let err = clean_candidates(&db, &source, job_id + 999).unwrap_err();
    assert!(err.contains("任务不存在"), "{err}");

    // CleanCandidateDto 可调试/克隆（DTO 契约完整性）
    let _: CleanCandidateDto = candidate.clone();
}

#[test]
fn apply_reverifies_then_deletes_and_skips_tampered() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let (job_id, files) = import_all(src.path(), db_dir.path(), target.path());

    // 篡改文件 1（追加字节：size 与 xxh64 均变）——复验必须拦下
    let tampered_path = src_path(src.path(), &files[1].0);
    let mut tampered = fs::read(&tampered_path).unwrap();
    tampered.extend_from_slice(b"TAMPERED");
    fs::write(&tampered_path, &tampered).unwrap();

    let db = open_db(db_dir.path());
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let result = clean_apply(&db, &bus, &VolumeSource::new(src.path()), job_id).unwrap();

    assert_eq!(result.deleted, 2, "{result:?}");
    assert_eq!(result.failed, 1);
    assert_eq!(
        result.freed_bytes,
        files[0].1.len() as u64 + files[2].1.len() as u64
    );
    assert_eq!(result.errors.len(), 1, "被篡改文件必须记 error: {result:?}");
    assert!(
        result.errors[0].contains("IMG_0002"),
        "error 应定位到文件: {result:?}"
    );

    // 源状态：未篡改的两个已删，篡改的保留（数据安全优先）
    assert!(!src_path(src.path(), &files[0].0).exists());
    assert!(tampered_path.exists(), "复验不一致的文件绝不可删");
    assert!(!src_path(src.path(), &files[2].0).exists());

    // 每文件日志：删除 info ×2 + 复验失败 warn ×1
    let infos = job_logs(&db, job_id, "info");
    assert_eq!(infos.iter().filter(|m| m.contains("已删除")).count(), 2);
    let warns = job_logs(&db, job_id, "warn");
    assert_eq!(
        warns.iter().filter(|m| m.contains("IMG_0002")).count(),
        1,
        "复验失败须逐文件告警: {warns:?}"
    );

    // 事件：cleanStarted（3 个候选含被篡改者）+ cleanFinished（统计一致）
    let mut started = None;
    let mut finished = None;
    while let Ok(ev) = rx.try_recv() {
        match ev {
            AppEvent::CleanStarted { .. } => started = Some(ev),
            AppEvent::CleanFinished { .. } => finished = Some(ev),
            _ => {}
        }
    }
    match started.expect("cleanStarted 事件") {
        AppEvent::CleanStarted {
            job_id: j,
            count,
            bytes,
        } => {
            assert_eq!(j, job_id);
            assert_eq!(count, 3, "候选计数含被篡改文件（删除前才复验）");
            assert_eq!(
                bytes,
                files.iter().map(|(_, c)| c.len() as u64).sum::<u64>()
            );
        }
        _ => unreachable!(),
    }
    match finished.expect("cleanFinished 事件") {
        AppEvent::CleanFinished { job_id: j, stats } => {
            assert_eq!(j, job_id);
            assert_eq!(stats.deleted, 2);
            assert_eq!(stats.failed, 1);
            assert_eq!(stats.freed_bytes, result.freed_bytes);
            assert_eq!(stats.errors.len(), 1);
        }
        _ => unreachable!(),
    }

    // 二次 apply：仅剩被篡改文件为候选 → 再次复验失败，零删除
    let second = clean_apply(&db, &bus, &VolumeSource::new(src.path()), job_id).unwrap();
    assert_eq!(second.deleted, 0, "{second:?}");
    assert_eq!(second.failed, 1);
    assert!(tampered_path.exists());
}

#[test]
fn apply_with_no_candidates_is_noop() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let (job_id, files) = import_all(src.path(), db_dir.path(), target.path());

    // 设备上的源全部消失（模拟用户已手动清理）→ 零候选，clean_apply 无害
    for (rel, _) in &files {
        fs::remove_file(src_path(src.path(), rel)).unwrap();
    }
    let db = open_db(db_dir.path());
    let result = clean_apply(
        &db,
        &EventBus::new(),
        &VolumeSource::new(src.path()),
        job_id,
    )
    .unwrap();
    assert_eq!(result.deleted, 0);
    assert_eq!(result.failed, 0);
    assert_eq!(result.freed_bytes, 0);
    assert!(result.errors.is_empty());

    // 任务不存在 → 报错
    let err = clean_apply(
        &db,
        &EventBus::new(),
        &VolumeSource::new(src.path()),
        job_id + 999,
    )
    .unwrap_err();
    assert!(err.contains("任务不存在"), "{err}");
}

// ---------------------------------------------------------------------------
// F1 IPC 编排（list_clean_candidates / apply_clean：库 + 设备注册表接线）
// ---------------------------------------------------------------------------

#[test]
fn ipc_clean_flow_and_offline_device_rejected() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_source(src.path());
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let device_id = state
        .devices
        .lock()
        .unwrap()
        .keys()
        .next()
        .cloned()
        .unwrap();

    // 经 IPC 导入（copy）→ 完成
    let plan = import::engine::ImportPlan {
        source_id: device_id,
        ..plan_for(target.path())
    };
    let job_id = start_import(&state, plan).unwrap();
    assert!(wait_done(&state, Duration::from_secs(15)), "导入应完成");

    // 候选：3 个，经 IPC 走 registry 源
    let candidates = list_clean_candidates(&state, job_id).unwrap();
    assert_eq!(candidates.len(), 3, "{candidates:?}");
    assert!(candidates.iter().all(|c| c.asset_id > 0));

    // 设备离线 → 两个命令都报错
    state.devices.lock().unwrap().clear();
    let err = list_clean_candidates(&state, job_id).unwrap_err();
    assert!(err.contains("不在线"), "{err}");
    let err = apply_clean(&state, job_id).unwrap_err();
    assert!(err.contains("不在线"), "{err}");

    // 任务不存在 → 报错
    let err = list_clean_candidates(&state, job_id + 999).unwrap_err();
    assert!(err.contains("任务不存在"), "{err}");
}
