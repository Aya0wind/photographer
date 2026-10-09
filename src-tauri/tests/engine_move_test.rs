//! 移动导入（mode=move）：校验入册后删源、空目录深优先清理、删源失败降级
//! 告警、copy 源零改动，以及源/目标互相嵌套的守卫拒绝。

mod common;

pub use common::{
    platform, scan,
    ai, bursts, db, devices, events, import, index, geo, ipc, metadata, plan_with_album,
    settings, tasks, thumbs,
};

use std::fs;
use std::path::PathBuf;

use common::{
    build_source, count_assets, expected_mtime_dir, journal_states, open_db, plan_for,
    run_engine,
    DeleteFailSource,
};
use devices::volume::VolumeSource;
use events::{EventBus, FileState};
use import::engine::{Engine, ImportMode};

#[test]
fn move_mode_deletes_source_and_cleans_empty_dirs() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let files = build_source(src.path());
    // move 会删源：期望目标路径（依赖源 mtime）必须在运行前预计算
    let expected_dst: Vec<(String, Vec<u8>, PathBuf)> = files
        .iter()
        .map(|(rel, content)| {
            (
                rel.clone(),
                content.clone(),
                expected_mtime_dir(target.path(), &src.path().join(rel))
                    .join(rel.rsplit('/').next().unwrap()),
            )
        })
        .collect();

    let (job_id, stats) = run_engine(src.path(), db_dir.path(), target.path(), |plan| {
        plan.mode = ImportMode::Move;
    });

    assert_eq!(stats.done_files, 3);
    assert_eq!(stats.moved, 3, "成功移动计数: {stats:?}");
    assert_eq!(stats.source_delete_failed, 0);
    assert_eq!(stats.failed_files, 0);

    // 目标字节级一致 + assets 在库 + journal 全 verified
    let db = open_db(db_dir.path());
    assert_eq!(count_assets(&db), 3);
    assert!(journal_states(&db, job_id)
        .into_iter()
        .all(|s| s == FileState::Verified));
    for (rel, content, dst) in &expected_dst {
        assert_eq!(fs::read(dst).unwrap(), *content);
        // 源文件已删
        assert!(
            !src.path().join(rel.replace('/', "\\")).exists(),
            "move 后源文件应删除: {rel}"
        );
    }

    // 空的源中间子目录已清理（DCIM/100CANON 整链），源根本身保留
    assert!(!src.path().join("DCIM").exists(), "空源子目录应清理");
    assert!(src.path().exists(), "源根必须保留");
}

#[test]
fn move_delete_failure_is_warning_not_import_failure() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let files = build_source(src.path());

    let db = open_db(db_dir.path());
    let mut plan = plan_with_album(&db, db_dir.path(), target.path());
    plan.mode = ImportMode::Move;
    let mut engine = Engine::new(
        db,
        EventBus::new(),
        Box::new(DeleteFailSource {
            inner: VolumeSource::new(src.path()),
        }),
        plan,
    );
    let job_id = engine.begin().unwrap();
    let stats = engine.run();

    // 删源失败 ≠ 导入失败：文件照常入册
    assert_eq!(stats.done_files, 3, "删源失败不得判导入失败: {stats:?}");
    assert_eq!(stats.failed_files, 0);
    assert_eq!(stats.moved, 0);
    assert_eq!(stats.source_delete_failed, 3);
    assert_eq!(count_assets(&open_db(db_dir.path())), 3);

    // 源文件原样保留（删源失败时数据不动）
    for (rel, content) in &files {
        assert_eq!(
            fs::read(src.path().join(rel.replace('/', "\\"))).unwrap(),
            *content,
            "删源失败时源必须原样保留: {rel}"
        );
    }

    // warn 日志条目（每文件一条删源失败告警）
    let db = open_db(db_dir.path());
    let warns: Vec<String> =
        db.0.prepare("SELECT message FROM logs WHERE job_id = ?1 AND level = 'warn'")
            .unwrap()
            .query_map([job_id], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
    assert_eq!(
        warns.iter().filter(|m| m.contains("删源失败")).count(),
        3,
        "每文件一条删源失败 warn: {warns:?}"
    );
}

#[test]
fn copy_mode_leaves_source_untouched() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let files = build_source(src.path());

    let (_, stats) = run_engine(src.path(), db_dir.path(), target.path(), |plan| {
        plan.mode = ImportMode::Copy; // 显式 copy（与缺省同义）
    });

    assert_eq!(stats.done_files, 3);
    assert_eq!(stats.moved, 0, "copy 模式 moved 恒 0");
    assert_eq!(stats.source_delete_failed, 0);
    // 源文件原样保留（含目录结构）
    for (rel, content) in &files {
        assert_eq!(
            fs::read(src.path().join(rel.replace('/', "\\"))).unwrap(),
            *content,
            "copy 模式源不得有任何改动: {rel}"
        );
    }
    assert!(src.path().join("DCIM").join("100CANON").exists());
}

#[test]
fn nesting_guard_rejects_overlapping_source_and_target() {
    let src = tempfile::tempdir().unwrap();
    let separate = tempfile::tempdir().unwrap();
    build_source(src.path());
    // 各分支独立数据库（照片库 root 互斥闸门会拒绝 src 与 src/vault 同库
    // 并存——嵌套守卫逐分支验证，互不污染登记表）
    let db_a = tempfile::tempdir().unwrap();
    let db_b = tempfile::tempdir().unwrap();
    let db_c = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();

    // ① 目标在源内（尚未存在——守卫须先建目录再 canonical 比较）
    let target_inside = src.path().join("vault");
    let mut engine = Engine::new(
        open_db(db_a.path()),
        EventBus::new(),
        Box::new(VolumeSource::new(src.path())),
        plan_for(&open_db(db_a.path()), db_a.path(), &target_inside),
    );
    let err = engine.begin().expect_err("目标在源内应拒绝");
    assert!(
        err.to_string().contains("嵌套"),
        "错误信息应说明嵌套: {err}"
    );

    // ② 源在目标内（target = 专用父目录，源是其一侧子目录；不得用 temp 根
    //    ——那会把数据库目录一并包含，登记闸门先行拒绝）
    let parent_dir = tempfile::tempdir().unwrap();
    let src_nested = parent_dir.path().join("card");
    std::fs::create_dir_all(&src_nested).unwrap();
    build_source(&src_nested);
    let mut engine = Engine::new(
        open_db(db_b.path()),
        EventBus::new(),
        Box::new(VolumeSource::new(&src_nested)),
        plan_for(&open_db(db_b.path()), db_b.path(), parent_dir.path()),
    );
    let err = engine.begin().expect_err("源在目标内应拒绝");
    assert!(
        err.to_string().contains("嵌套"),
        "错误信息应说明嵌套: {err}"
    );

    // ③ 目标与源相同 → 拒绝
    let mut engine = Engine::new(
        open_db(db_c.path()),
        EventBus::new(),
        Box::new(VolumeSource::new(src.path())),
        plan_for(&open_db(db_c.path()), db_c.path(), src.path()),
    );
    assert!(engine.begin().is_err(), "目标与源相同应拒绝");

    // ④ 相互独立的目录 → 放行
    let separate_db = open_db(db_dir.path());
    let plan = plan_with_album(&separate_db, db_dir.path(), separate.path());
    let mut engine = Engine::new(
        separate_db,
        EventBus::new(),
        Box::new(VolumeSource::new(src.path())),
        plan,
    );
    assert!(engine.begin().is_ok(), "独立目录不得误拒");

    // 拒绝时不留任务（前三次 begin 失败未建 job；④ 库仅 1 条）
    for db_case in [db_a.path(), db_b.path(), db_c.path()] {
        let db = open_db(db_case);
        let jobs: i64 =
            db.0.query_row("SELECT COUNT(*) FROM jobs", [], |r| r.get(0))
                .unwrap();
        assert_eq!(jobs, 0, "拒绝的 begin 不得建任务");
    }
    let jobs: i64 = open_db(db_dir.path())
        .0
        .query_row("SELECT COUNT(*) FROM jobs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(jobs, 1, "仅第④次 begin 建任务");
}
