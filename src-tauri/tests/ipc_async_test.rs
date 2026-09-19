//! IPC 异步命令壳测试（铁律：慢命令一律 async + spawn_blocking，主线程
//! 只做纯内存/原子操作）。
//!
//! 状态命令的壳统一为 `ipc::run_blocking(SharedState, work)`——本函数不
//! 依赖 tauri 运行时外壳（AppHandle/mock app 会拉入 comctl32 v6 依赖，
//! 裸测试 exe 无 manifest 加载即失败），可直接 `block_on` 验证
//! 「离开调用线程 + 后台执行 + 正确返回 + 错误语义保持」；
//! 无状态命令（fs_list_dirs）直接 await async fn。

mod common;

pub use common::{
    ai, db, devices, events, import, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::sync::Arc;
use std::time::Duration;

use common::{build_many, state_with_library};
use ipc::{files_by_id, run_blocking, SharedState};

#[test]
fn run_blocking_executes_off_thread_and_returns() {
    let src = tempfile::tempdir().unwrap();
    build_many(src.path(), 5);
    let db_dir = tempfile::tempdir().unwrap();
    let shared: SharedState = Arc::new(state_with_library(
        db_dir.path(),
        src.path(),
        Duration::from_millis(1),
    ));
    let device_id = shared
        .devices
        .lock()
        .unwrap()
        .keys()
        .next()
        .cloned()
        .unwrap();
    let main_thread = std::thread::current().id();

    // 正常路径：后台线程执行核心逻辑并返回（附带线程 id 证明离开调用线程）
    let (files, worker_thread) =
        tauri::async_runtime::block_on(run_blocking(shared.clone(), move |state| {
            let worker = std::thread::current().id();
            files_by_id(state, &device_id).map(|files| (files, worker))
        }))
        .expect("run_blocking 应成功");
    assert_eq!(files.len(), 5);
    assert!(files[0].rel_path.starts_with("DCIM/"));
    assert_ne!(
        worker_thread, main_thread,
        "慢命令工作必须离开调用（主）线程"
    );
    // 负载契约不变（camelCase）
    let json = serde_json::to_value(&files[0]).unwrap();
    assert!(json["relPath"].as_str().is_some());

    // 错误语义经壳同样保持
    let err =
        tauri::async_runtime::block_on(run_blocking(shared, |state| files_by_id(state, "missing")))
            .unwrap_err();
    assert!(err.contains("不在线"), "{err}");
}

#[test]
fn async_fs_list_dirs_shell_keeps_contract() {
    // 无 State 的纯 IO 命令直接 await async fn：spawn_blocking 后台执行，
    // 返回类型不变（非 Result，失败→空数组）
    let drives = tauri::async_runtime::block_on(ipc::device::fs_list_dirs(None));
    assert!(
        drives.iter().any(|d| d.path == "C:\\"),
        "应包含 C:\\ : {drives:?}"
    );
    assert!(
        tauri::async_runtime::block_on(ipc::device::fs_list_dirs(Some(
            r"C:\definitely\not\here".into()
        )))
        .is_empty(),
        "非法路径 → 空数组不报错"
    );
}
