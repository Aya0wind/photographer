//! 索引触发链（真机修复 2026-09-19）：启动补触发（模型就位后存量资产自愈）、
//! index_kick_now 手动触发契约（未就绪明确报错 / 幂等）、index_status 计数。

mod common;

pub use common::{
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks,
    thumbs, videos,
};

use std::time::Duration;

use common::open_db;
use db::AssetRow;
use events::AssetKind;

/// 造资产行（photo/raw 会自动挂 thumb 任务）。
fn asset_row(path: &str, kind: AssetKind) -> AssetRow {
    AssetRow {
        path: path.into(),
        filename: path.rsplit(['/', '\\']).next().unwrap_or(path).into(),
        size: 100,
        mtime: "2026-09-19T00:00:00.000Z".into(),
        xxhash: 1,
        kind,
        captured_at: None,
        camera: None,
        source: "imported".into(),
        created_at: "2026-09-19T00:00:00.000Z".into(),
        origin: "imported".into(),
        width: None,
        height: None,
        iso: None,
        f_number: None,
        exposure_time: None,
        focal_length: None,
        lens: None,
        pair_asset_id: None,
        thumb_state: 0,
        rating: 0,
        flagged: 0,
        color_label: None,
        rejected: 0,
        orientation: None,
        flash: None,
        metering_mode: None,
        white_balance: None,
        exposure_program: None,
        software: None,
        artist: None,
        gps_lat: None,
        gps_lon: None,
    }
}

/// 在 ModelManager 根目录放假 ONNX 文件（ready 检查只看文件存在性，
/// 让触发链可测；真模型推理由 #[ignore] smoke 覆盖）。
fn fake_models(models_root: &std::path::Path, ids: &[&str]) {
    std::fs::create_dir_all(models_root).unwrap();
    for id in ids {
        std::fs::write(models_root.join(format!("{id}.onnx")), b"fake").unwrap();
    }
}

fn ai_task_count(db: &db::Db) -> i64 {
    db.0.query_row(
        "SELECT COUNT(*) FROM index_tasks WHERE kind = 'ai'",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

/// 轮询到条件成立（deadline 内），返回是否达成。
fn wait_until(deadline: Duration, mut pred: impl FnMut() -> bool) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < deadline {
        if pred() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    pred()
}

// ---------------------------------------------------------------------------
// 启动补触发（lib.rs setup 的等价逻辑：resume_and_kick + enable_clip 时踢回填）
// ---------------------------------------------------------------------------

#[test]
fn startup_kick_after_models_ready_creates_ai_tasks_for_legacy_assets() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_db(dir.path());
    // 存量资产：模型就位前导入（ai_indexed_at IS NULL）
    for i in 0..4 {
        db.insert_asset(&asset_row(&format!("X:/p/{i}.jpg"), AssetKind::Photo))
            .unwrap();
    }
    assert_eq!(ai_task_count(&db), 0, "现状复现：无人踢则 0 条 ai 待办");

    // 账本幂等（确定性，不派 worker）：pending 存在时不重复建任务
    assert_eq!(db.create_ai_tasks_for_unindexed().unwrap(), 4);
    assert_eq!(
        db.create_ai_tasks_for_unindexed().unwrap(),
        0,
        "pending 去重"
    );

    // 账本幂等：已记账（ai_indexed_at）的资产不会被回建
    let id = db.asset_id_by_path("X:/p/0.jpg").unwrap().unwrap();
    db.set_ai_indexed(id).unwrap();
    db.0.execute(
        "UPDATE index_tasks SET state = 'done' WHERE asset_id = ?1",
        [id],
    )
    .unwrap();
    assert_eq!(
        db.create_ai_tasks_for_unindexed().unwrap(),
        0,
        "已记账资产不回建"
    );

    // 模拟「模型已就绪」（dummy 文件通过 ready 检查）+ 走与 lib.rs 启动补
    // 触发相同的入口：kick 后存量资产全部有 ai 任务（含刚才回建消除的 3 条）
    let models_root = dir.path().join("models");
    fake_models(
        &models_root,
        &["siglip2-visual", "siglip2-text", "siglip2-tokenizer"],
    );
    let manager = ai::ModelManager::new(
        models_root,
        events::EventBus::new(),
        tasks::TaskSupervisor::new(events::EventBus::new()),
    );
    assert!(manager.semantic_ready());
    let bus = events::EventBus::new();
    let supervisor = tasks::TaskSupervisor::new(bus.clone());
    ai::semantic::kick_semantic_if_ready(dir.path().to_path_buf(), &manager, &bus, &supervisor);
    assert!(
        wait_until(Duration::from_secs(30), || ai_task_count(&db) >= 4),
        "启动补触发应为存量资产建 ai 任务: {}",
        ai_task_count(&db)
    );
}

/// 回归：失败任务不能在每次手动点击时重复插入；手动重试应原地复位同一行。
#[test]
fn failed_ai_task_is_retried_without_duplicate_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_db(dir.path());
    db.insert_asset(&asset_row("X:/p/retry.jpg", AssetKind::Photo))
        .unwrap();
    assert_eq!(db.create_ai_tasks_for_unindexed().unwrap(), 1);
    let task_id: i64 =
        db.0.query_row("SELECT id FROM index_tasks WHERE kind = 'ai'", [], |r| {
            r.get(0)
        })
        .unwrap();
    for _ in 0..3 {
        db.finish_index_task(task_id, false).unwrap();
    }
    assert_eq!(db.create_ai_tasks_for_unindexed().unwrap(), 0);
    assert_eq!(ai_task_count(&db), 1, "失败后补种不得产生重复任务");

    assert_eq!(db.retry_failed_index_tasks("ai").unwrap(), 1);
    let (state, attempts): (String, i64) =
        db.0.query_row(
            "SELECT state, attempts FROM index_tasks WHERE id = ?1",
            [task_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((state.as_str(), attempts), ("pending", 0));
}

/// 回归（真机修复 2026-09-19）：人脸回填与语义同病灶——以「本轮新建任务数」
/// 为闸，任务已存在时新建数为 0 直接空转，存量 pending 无人消费（真机 face
/// 干 9 条后经一次重启就永久停摆）。修复后判据=存量 pending。
#[test]
fn face_backfill_consumes_existing_pending_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_db(dir.path());
    db.insert_asset(&asset_row("X:/p/f1.jpg", AssetKind::Photo))
        .unwrap();
    assert_eq!(db.create_face_tasks_for_unindexed().unwrap(), 1);
    assert_eq!(db.pending_index_task_count("face").unwrap(), 1);

    // 模型目录为空 → 处理必失败；但任务必须被 worker 消费（attempts 封顶 → failed）
    let manager = ai::ModelManager::new(
        dir.path().join("models-empty"),
        events::EventBus::new(),
        tasks::TaskSupervisor::new(events::EventBus::new()),
    );
    let done = ai::face::run_face_backfill(
        dir.path(),
        std::sync::Arc::new(manager),
        &events::EventBus::new(),
    );
    assert_eq!(done, 0, "无模型不可能成功");
    assert_eq!(
        db.pending_index_task_count("face").unwrap(),
        0,
        "存量 pending 必须被 worker 消费"
    );
    let failed: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM index_tasks WHERE kind = 'face' AND state = 'failed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(failed, 1);
}

// ---------------------------------------------------------------------------
// index_kick_now
// ---------------------------------------------------------------------------

#[test]
fn index_kick_now_contract() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    {
        let db = open_db(db_dir.path());
        for i in 0..3 {
            db.insert_asset(&asset_row(&format!("X:/p/{i}.jpg"), AssetKind::Photo))
                .unwrap();
        }
    }

    // 未知通道报错
    assert!(ipc::indexing::fetch_index_kick_now(&state, "nope").is_err());

    // thumb 通道：幂等触发（不建新任务，只跑待办）
    ipc::indexing::fetch_index_kick_now(&state, "thumb").unwrap();
    ipc::indexing::fetch_index_kick_now(&state, "exif").unwrap();

    // ai 通道：模型未就绪 → 明确中文错误
    let err = ipc::indexing::fetch_index_kick_now(&state, "ai").unwrap_err();
    assert!(err.contains("请先在设置中下载模型"), "实际错误: {err}");
    // face 通道同款
    let err = ipc::indexing::fetch_index_kick_now(&state, "face").unwrap_err();
    assert!(err.contains("请先在设置中下载模型"), "实际错误: {err}");

    // 模型就绪但开关未开 → 指向设置开关
    fake_models(
        &db_dir.path().join("models"),
        &["siglip2-visual", "siglip2-text", "siglip2-tokenizer"],
    );
    let err = ipc::indexing::fetch_index_kick_now(&state, "ai").unwrap_err();
    assert!(err.contains("语义索引未开启"), "实际错误: {err}");

    // 开关打开 → 触发成功且为存量资产建任务
    state.settings.lock().unwrap().ai.enable_clip = true;
    ipc::indexing::fetch_index_kick_now(&state, "ai").unwrap();
    let db = open_db(db_dir.path());
    assert!(
        wait_until(Duration::from_secs(30), || ai_task_count(&db) == 3),
        "手动触发应建 ai 任务: {}",
        ai_task_count(&db)
    );

    // thumb 幂等：触发不改变任务总数（只消费待办，不新增）
    let before = {
        let db = open_db(db_dir.path());
        db.0.query_row(
            "SELECT COUNT(*) FROM index_tasks WHERE kind = 'thumb'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    ipc::indexing::fetch_index_kick_now(&state, "thumb").unwrap();
    let after = {
        let db = open_db(db_dir.path());
        db.0.query_row(
            "SELECT COUNT(*) FROM index_tasks WHERE kind = 'thumb'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    assert_eq!(before, after, "thumb 触发幂等（不建新任务）");
}

// ---------------------------------------------------------------------------
// index_status
// ---------------------------------------------------------------------------

#[test]
fn index_status_counts_by_kind_and_state() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let db = open_db(db_dir.path());
    // 3 photo + 1 video：thumb 任务 photo 自动挂 3 条 pending；video 永久占位
    for i in 0..3 {
        db.insert_asset(&asset_row(&format!("X:/p/{i}.jpg"), AssetKind::Photo))
            .unwrap();
    }
    db.insert_asset(&asset_row("X:/p/v.mp4", AssetKind::Video))
        .unwrap();
    // 人工布置各通道状态
    let a0 = db.asset_id_by_path("X:/p/0.jpg").unwrap().unwrap();
    db.0.execute(
        "UPDATE index_tasks SET state = 'done' WHERE kind = 'thumb' AND asset_id = ?1",
        [a0],
    )
    .unwrap();
    db.0.execute("UPDATE assets SET thumb_state = 1 WHERE id = ?1", [a0])
        .unwrap();
    let a1 = db.asset_id_by_path("X:/p/1.jpg").unwrap().unwrap();
    db.0.execute(
        "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
             VALUES ('ai', ?1, 'running', 0, '2026', '2026')",
        [a1],
    )
    .unwrap();
    let a2 = db.asset_id_by_path("X:/p/2.jpg").unwrap().unwrap();
    db.0.execute(
        "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
             VALUES ('ai', ?1, 'done', 0, '2026', '2026')",
        [a2],
    )
    .unwrap();
    db.0.execute(
        "UPDATE assets SET ai_indexed_at = '2026' WHERE id = ?1",
        [a2],
    )
    .unwrap();
    db.0.execute(
        "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
             VALUES ('face', ?1, 'pending', 0, '2026', '2026')",
        [a2],
    )
    .unwrap();
    db.0.execute(
        "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
             VALUES ('face', ?1, 'failed', 3, '2026', '2026')",
        [a0],
    )
    .unwrap();

    let status = ipc::indexing::fetch_index_status(&state).unwrap();
    assert_eq!(status.thumb.pending, 2);
    assert_eq!(status.thumb.done, 1);
    assert_eq!(status.thumb.failed, 0);
    assert_eq!(
        status.thumb.total, 3,
        "可索引资产 = photo/raw 数（video 不计）"
    );
    assert_eq!(status.exif.pending, 0, "EXIF 随导入同步完成，无后台待办");
    assert_eq!(status.exif.done, 3, "已入库资产的 EXIF 应计为已完成");
    assert_eq!(status.ai.pending, 0);
    assert_eq!(status.ai.running, 1);
    assert_eq!(status.ai.done, 1);
    assert_eq!(status.ai.total, 3);
    assert_eq!(status.face.pending, 1);
    assert_eq!(status.face.failed, 1);

    // camelCase 载荷契约（前端消费）
    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(json["thumb"]["pending"], 2);
    assert_eq!(json["thumb"]["total"], 3);
    assert_eq!(json["ai"]["running"], 1);
    assert_eq!(json["face"]["failed"], 1);
}
