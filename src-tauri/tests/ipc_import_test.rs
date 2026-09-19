//! IPC 导入编排：start/pause/resume/cancel 状态机（Busy/任务 ID 校验）、
//! 无库/离线设备拒绝、jobs/logs 分页、失败重试边界、journal 终态与
//! device_files camelCase 契约。

mod common;

pub use common::{
    ai, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::time::Duration;

use common::{build_many, ipc_plan, state_with_library, wait_done};
use devices::normalize_device_id;
use events::FileState;
use ipc::{
    active_library_db, cancel_import, device_registered, files_by_id, jobs_page, logs_page,
    retry_failed, set_import_paused, start_import,
};

#[test]
fn device_registered_is_case_insensitive_for_wpd_ids() {
    // 重复到达幂等的判定基座：注册表 key 与查找都过 normalize——
    // 同一相机的大小写两种到达形式必须命中同一条目
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    build_many(src.path(), 2);
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));

    let upper = r"\\?\USB#VID_054C&PID_0E0B#SRL#{6AC27878-A6FA-4155-BA85-F98F491D4F33}";
    let lower = normalize_device_id(upper);
    assert_eq!(lower, upper.to_ascii_lowercase());

    // 以（规范化后的）小写注册——启动枚举路径
    let source: std::sync::Arc<dyn devices::DeviceSource> =
        std::sync::Arc::new(devices::volume::VolumeSource::new(src.path()));
    let snapshot = devices::orchestrator::DeviceSnapshot {
        id: lower.clone(),
        name: "ILCE-7RM5".into(),
        kind: devices::SourceKind::Mtp,
        files_by_kind: Default::default(),
        bytes_total: 0,
        new_files: 0,
    };
    state
        .devices
        .lock()
        .unwrap()
        .insert(lower.clone(), ipc::DeviceEntry::ready(source, snapshot));

    // 大写到达形式的幂等判定命中（枚举空窗期不误报"未找到"）
    assert!(device_registered(&state, &lower));
    assert!(
        device_registered(&state, upper),
        "大小写变体必须命中同一注册条目"
    );
    assert!(!device_registered(
        &state,
        r"\\?\USB#VID_054C&PID_0E0B#OTHER#X"
    ));

    // 卷/文件夹 id 不受影响（未注册的盘符不命中）
    assert!(!device_registered(&state, "Z:"));
}

#[test]
fn start_pause_resume_cancel_state_machine() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_many(src.path(), 8);
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(25));

    let job_id = start_import(&state, ipc_plan(&state, target.path())).unwrap();
    assert!(job_id > 0);

    // Busy：进行中二次启动被拒
    let busy = start_import(&state, ipc_plan(&state, target.path())).unwrap_err();
    assert!(busy.contains("Busy"), "应返回 Busy 错误: {busy}");

    // 暂停 → 恢复 → 取消（job_id 不匹配报错）
    set_import_paused(&state, job_id, true).unwrap();
    let wrong = set_import_paused(&state, job_id + 999, false).unwrap_err();
    assert!(wrong.contains("不一致"));
    set_import_paused(&state, job_id, false).unwrap();
    cancel_import(&state, job_id).unwrap();
    let wrong_cancel = cancel_import(&state, job_id + 999).unwrap_err();
    assert!(wrong_cancel.contains("不一致"));

    assert!(wait_done(&state, Duration::from_secs(10)), "取消后应收尾");
    let rows = jobs_page(&state, 0, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "cancelled");
    assert_eq!(rows[0].id, job_id);

    // 取消后可再启动（活跃项被回收）
    let second = start_import(&state, ipc_plan(&state, target.path()));
    match second {
        Ok(new_id) => {
            assert_ne!(new_id, job_id);
            cancel_import(&state, new_id).unwrap();
            assert!(wait_done(&state, Duration::from_secs(10)));
        }
        Err(err) => panic!("取消后应允许再次导入: {err}"),
    }
}

#[test]
fn no_active_library_rejected() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    build_many(src.path(), 2);
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(5));
    state.settings.lock().unwrap().active_library_id = None;

    let err = match active_library_db(&state) {
        Err(err) => err,
        Ok(_) => panic!("无库时应报错"),
    };
    assert!(err.contains("尚未创建库"));
    let err = start_import(&state, ipc_plan(&state, db_dir.path())).unwrap_err();
    assert!(err.contains("尚未创建库"));
}

#[test]
fn offline_device_rejected() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    build_many(src.path(), 2);
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(5));
    let mut bad = ipc_plan(&state, db_dir.path());
    bad.source_id = "Z:".into();
    let err = start_import(&state, bad).unwrap_err();
    assert!(err.contains("不在线"));
}

#[test]
fn jobs_and_logs_paging_after_completion() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_many(src.path(), 3);
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));

    let job_id = start_import(&state, ipc_plan(&state, target.path())).unwrap();
    assert!(wait_done(&state, Duration::from_secs(10)));

    let rows = jobs_page(&state, 0, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "done");
    assert_eq!(rows[0].total_files, 3);
    // keyset 分页边界
    assert!(jobs_page(&state, job_id, 10).unwrap().is_empty());

    let logs = logs_page(&state, job_id, 0, 50).unwrap();
    assert!(logs.iter().any(|l| l.message.contains("导入会话开始")));
    assert!(logs.iter().any(|l| l.message.contains("导入会话结束")));
    assert!(logs.iter().all(|l| l.job_id == Some(job_id)));

    // 无失败 → retry 报错
    let err = retry_failed(&state, job_id).unwrap_err();
    assert!(err.contains("没有可重试"));
}

#[test]
fn device_files_passthrough_with_camel_case() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    build_many(src.path(), 2);
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let device_id = state
        .devices
        .lock()
        .unwrap()
        .keys()
        .next()
        .cloned()
        .unwrap();

    let files = files_by_id(&state, &device_id).unwrap();
    assert_eq!(files.len(), 2);
    assert!(files[0].rel_path.starts_with("DCIM/"));
    let json = serde_json::to_value(&files[0]).unwrap();
    assert_eq!(json["relPath"], serde_json::json!(files[0].rel_path));
    assert!(json["mtime"].as_str().is_some_and(|m| m.ends_with('Z')));

    let err = files_by_id(&state, "missing").unwrap_err();
    assert!(err.contains("不在线"));
}

#[test]
fn journal_reflects_final_states() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_many(src.path(), 4);
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));

    let job_id = start_import(&state, ipc_plan(&state, target.path())).unwrap();
    assert!(wait_done(&state, Duration::from_secs(10)));

    let db = active_library_db(&state).unwrap();
    let states: Vec<FileState> = db
        .all_job_files(job_id)
        .unwrap()
        .into_iter()
        .map(|r| r.state)
        .collect();
    assert!(states.iter().all(|s| *s == FileState::Verified));
    // 目标落位
    let count =
        db.0.query_row("SELECT COUNT(*) FROM assets", [], |r| r.get::<_, i64>(0))
            .unwrap();
    assert_eq!(count, 4);
}
