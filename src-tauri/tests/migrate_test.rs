//! 目录迁移（M3，spec §5.11）：dbDir 两阶段（成功全链 / journal 断点恢复 /
//! 文件失败回滚不留脏）、photoRoot switch 零物理变化 + external 语义、
//! migrate 批量移动 + 路径更新 + 崩溃窗口对账恢复、迁移期间拒绝导入、
//! 验证拒绝项与事件 camelCase 契约。

mod common;

pub use common::{
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks,
    thumbs, videos,
};

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::state_with_library;
use db::AssetRow;
use events::AppEvent;
use ipc::AppState;
use settings::Settings;

/// 迁移测试用 AppState：库 db_dir/photo_root 指定，config_dir 独立（可落盘）。
fn migration_state(db_dir: &Path, photo_root: &Path) -> Arc<AppState> {
    let src = tempfile::tempdir().unwrap();
    let state = state_with_library(db_dir, src.path(), Duration::from_millis(1));
    {
        let mut s = state.settings.lock().unwrap();
        let lib = s.libraries.first_mut().unwrap();
        lib.photo_root = photo_root.to_string_lossy().into_owned();
    }
    Arc::new(state)
}

/// 造一个有 db + 缩略图 + 嵌套文件的库目录，返回 db（已关）与文件清单。
fn build_library_dir(db_dir: &Path) -> db::Db {
    let database = common::open_db(db_dir);
    database
        .insert_asset(&AssetRow {
            path: "X:\\photo\\a.jpg".into(),
            filename: "a.jpg".into(),
            size: 5,
            mtime: "2026-09-01T00:00:00.000Z".into(),
            xxhash: 1,
            kind: events::AssetKind::Photo,
            captured_at: None,
            camera: None,
            source: "imported".into(),
            created_at: "2026-09-01T00:00:00.000Z".into(),
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
            orientation: None,
            flash: None,
            metering_mode: None,
            white_balance: None,
            exposure_program: None,
            software: None,
            artist: None,
            gps_lat: None,
            gps_lon: None,
        })
        .unwrap();
    std::fs::create_dir_all(db_dir.join("thumbs").join("256")).unwrap();
    std::fs::write(
        db_dir.join("thumbs").join("256").join("ca.jpg"),
        b"thumb-bytes",
    )
    .unwrap();
    std::fs::create_dir_all(db_dir.join("logs")).unwrap();
    std::fs::write(db_dir.join("logs").join("run.log"), b"log-bytes").unwrap();
    database
}

/// 轮询总线直到 migrationFinished，返回 (ok, failed)；附带收集 started。
fn wait_migration_finished(state: &AppState) -> (AppEvent, Option<AppEvent>) {
    let mut rx = state.bus.subscribe();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut started = None;
    loop {
        while let Ok(event) = rx.try_recv() {
            match event {
                AppEvent::MigrationStarted { .. } => started = Some(event.clone()),
                AppEvent::MigrationFinished { .. } => return (event, started),
                _ => {}
            }
        }
        if Instant::now() > deadline {
            panic!("30s 内未收到 migrationFinished");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn settings_on_disk(state: &AppState) -> Settings {
    settings::SettingsManager::load(&state.config_dir).unwrap()
}

#[test]
fn db_dir_migration_success_full_chain() {
    let old_dir = tempfile::tempdir().unwrap();
    let new_holder = tempfile::tempdir().unwrap();
    let new_dir = new_holder.path().join("fresh"); // 不存在，由迁移创建
    build_library_dir(old_dir.path());
    let state = migration_state(old_dir.path(), Path::new("X:\\photos"));
    let mut rx = state.bus.subscribe();

    migrate::db_dir_migrate(&state, "lib-1", &new_dir.to_string_lossy()).unwrap();
    let (finished, started) = wait_migration_finished(&state);
    assert!(
        matches!(
            finished,
            AppEvent::MigrationFinished {
                ok: true,
                failed: 0
            }
        ),
        "{finished:?}"
    );
    assert!(matches!(
        started,
        Some(AppEvent::MigrationStarted { kind, total_bytes }) if kind == "dbDir" && total_bytes > 0
    ));
    // 进度事件至少一条
    assert!(
        (0..64).any(|_| matches!(rx.try_recv(), Ok(AppEvent::MigrationProgress { .. }))),
        "应有进度事件"
    );

    // 注册表切换：内存 + 磁盘
    let mem_lib = state
        .settings
        .lock()
        .unwrap()
        .active_library()
        .unwrap()
        .clone();
    assert_eq!(PathBuf::from(&mem_lib.db_dir), new_dir);
    let disk = settings_on_disk(&state);
    assert_eq!(disk.libraries[0].db_dir, new_dir.to_string_lossy());

    // 新目录内容齐全且库可开、资产在册
    assert!(new_dir.join("library.db").is_file());
    assert!(new_dir.join("thumbs").join("256").join("ca.jpg").is_file());
    assert!(new_dir.join("logs").join("run.log").is_file());
    assert_eq!(common::count_assets(&common::open_db(&new_dir)), 1);

    // 旧目录：文件原样保留 + 迁移标记
    assert!(old_dir.path().join("library.db").is_file());
    let marker = old_dir.path().join(".migrated-bak");
    assert!(marker.is_file(), "旧目录必须有 .migrated-bak 标记");
    let marker_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&marker).unwrap()).unwrap();
    assert_eq!(marker_json["newDir"], new_dir.to_string_lossy().to_string());

    // 守卫解除
    assert!(
        state.migrations.lock().unwrap().is_empty(),
        "迁移完成后守卫必须解除"
    );
}

#[test]
fn db_dir_migration_resumes_from_journal_without_recopy() {
    let old_dir = tempfile::tempdir().unwrap();
    let new_dir = tempfile::tempdir().unwrap();
    std::fs::write(old_dir.path().join("a.bin"), vec![1u8; 4096]).unwrap();
    std::fs::write(old_dir.path().join("b.bin"), vec![2u8; 8192]).unwrap();
    let state = migration_state(old_dir.path(), Path::new("X:\\photos"));

    // 模拟上次中断：a.bin 已复制+校验入 journal，b.bin 未开始
    std::fs::create_dir_all(new_dir.path()).unwrap();
    std::fs::copy(old_dir.path().join("a.bin"), new_dir.path().join("a.bin")).unwrap();
    let journal = serde_json::json!({
        "schema": 1,
        "oldDir": old_dir.path().to_string_lossy(),
        "verified": [{"rel": "a.bin", "size": 4096, "xxh": 0}]
    });
    std::fs::write(
        new_dir.path().join(".db-migration.json"),
        journal.to_string(),
    )
    .unwrap();
    let copied_mtime = std::fs::metadata(new_dir.path().join("a.bin"))
        .unwrap()
        .modified()
        .unwrap();
    std::thread::sleep(Duration::from_millis(120));

    migrate::db_dir_migrate(&state, "lib-1", &new_dir.path().to_string_lossy()).unwrap();
    let (finished, _) = wait_migration_finished(&state);
    assert!(
        matches!(
            finished,
            AppEvent::MigrationFinished {
                ok: true,
                failed: 0
            }
        ),
        "{finished:?}"
    );

    // a.bin 未重拷（mtime 不变）；b.bin 补齐；注册表已切换
    let after = std::fs::metadata(new_dir.path().join("a.bin"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(copied_mtime, after, "journal 已校验的文件不得重拷");
    assert!(new_dir.path().join("b.bin").is_file());
    let mem_lib = state
        .settings
        .lock()
        .unwrap()
        .active_library()
        .unwrap()
        .clone();
    assert_eq!(PathBuf::from(&mem_lib.db_dir), new_dir.path());
}

#[test]
fn db_dir_migration_file_failure_rolls_back_clean() {
    let old_dir = tempfile::tempdir().unwrap();
    let new_holder = tempfile::tempdir().unwrap();
    let new_dir = new_holder.path().join("m");
    build_library_dir(old_dir.path());
    std::fs::write(old_dir.path().join("locked.bin"), vec![9u8; 1024]).unwrap();
    let state = migration_state(old_dir.path(), Path::new("X:\\photos"));

    // Windows 独占打开一个源文件（share_mode=0）→ 复制必败
    #[cfg(windows)]
    let lock = {
        use std::os::windows::fs::OpenOptionsExt;
        Mutex::new((std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(old_dir.path().join("locked.bin"))
            .unwrap(),))
    };

    migrate::db_dir_migrate(&state, "lib-1", &new_dir.to_string_lossy()).unwrap();
    let (finished, _) = wait_migration_finished(&state);
    assert!(
        matches!(finished, AppEvent::MigrationFinished { ok: false, .. }),
        "{finished:?}"
    );
    drop(lock);

    // 不留脏：新目录整体回滚、注册表未动、旧目录原样无标记
    assert!(!new_dir.exists(), "失败必须回滚删除新目录");
    let mem_lib = state
        .settings
        .lock()
        .unwrap()
        .active_library()
        .unwrap()
        .clone();
    assert_eq!(PathBuf::from(&mem_lib.db_dir), old_dir.path());
    assert!(old_dir.path().join("library.db").is_file());
    assert!(!old_dir.path().join(".migrated-bak").exists());
    assert!(
        state.migrations.lock().unwrap().is_empty(),
        "失败后守卫同样解除"
    );
}

#[test]
fn db_dir_migration_validations() {
    let old_dir = tempfile::tempdir().unwrap();
    build_library_dir(old_dir.path());
    let state = migration_state(old_dir.path(), Path::new("X:\\photos"));

    // 库不存在
    assert!(migrate::db_dir_migrate(&state, "nope", "X:\\somewhere").is_err());
    // 相同目录
    let same = old_dir.path().to_string_lossy().into_owned();
    assert!(migrate::db_dir_migrate(&state, "lib-1", &same).is_err());
    // 嵌套（新目录在旧目录内）
    let nested = old_dir.path().join("inside").to_string_lossy().into_owned();
    assert!(migrate::db_dir_migrate(&state, "lib-1", &nested).is_err());
    // 新目录非空且无 journal
    let occupied = tempfile::tempdir().unwrap();
    std::fs::write(occupied.path().join("junk.txt"), b"x").unwrap();
    let occ = occupied.path().to_string_lossy().into_owned();
    let err = migrate::db_dir_migrate(&state, "lib-1", &occ).unwrap_err();
    assert!(err.contains("非空"), "{err}");
    assert!(
        state.migrations.lock().unwrap().is_empty(),
        "验证失败不得残留守卫"
    );
}

fn build_photo_library(old_root: &Path, db_dir: &Path) {
    // 2 个库内资产（旧根下）+1 个外部索引 +1 个库内但旧根外
    std::fs::create_dir_all(old_root.join("SmartPhoto").join("2026")).unwrap();
    std::fs::write(
        old_root.join("SmartPhoto").join("2026").join("a.jpg"),
        vec![1u8; 2048],
    )
    .unwrap();
    std::fs::write(
        old_root.join("SmartPhoto").join("2026").join("b.jpg"),
        vec![2u8; 4096],
    )
    .unwrap();
    let database = common::open_db(db_dir);
    let row = |path: String, origin: &str| AssetRow {
        path,
        filename: "x.jpg".into(),
        size: 10,
        mtime: "2026-09-01T00:00:00.000Z".into(),
        xxhash: 1,
        kind: events::AssetKind::Photo,
        captured_at: None,
        camera: None,
        source: "imported".into(),
        created_at: "2026-09-01T00:00:00.000Z".into(),
        origin: origin.into(),
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
        orientation: None,
        flash: None,
        metering_mode: None,
        white_balance: None,
        exposure_program: None,
        software: None,
        artist: None,
        gps_lat: None,
        gps_lon: None,
    };
    database
        .insert_asset(&row(
            old_root
                .join("SmartPhoto")
                .join("2026")
                .join("a.jpg")
                .to_string_lossy()
                .into_owned(),
            "imported",
        ))
        .unwrap();
    database
        .insert_asset(&row(
            old_root
                .join("SmartPhoto")
                .join("2026")
                .join("b.jpg")
                .to_string_lossy()
                .into_owned(),
            "imported",
        ))
        .unwrap();
    database
        .insert_asset(&row("W:\\elsewhere\\c.jpg".into(), "external"))
        .unwrap();
    database
        .insert_asset(&row("Z:\\outside\\d.jpg".into(), "imported"))
        .unwrap();
}

#[test]
fn photo_root_switch_mode_changes_config_only() {
    let old_root = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    build_photo_library(old_root.path(), db_dir.path());
    let state = migration_state(db_dir.path(), old_root.path());
    let mut rx = state.bus.subscribe();

    migrate::photo_root_switch(&state, "lib-1", "Q:\\newroot", "switch").unwrap();
    // switch 为同步路径：两条事件即时可见
    assert!(
        matches!(rx.try_recv(), Ok(AppEvent::MigrationStarted { kind, .. }) if kind == "photoRoot")
    );
    assert!(matches!(
        rx.try_recv(),
        Ok(AppEvent::MigrationFinished {
            ok: true,
            failed: 0
        })
    ));

    // 仅配置变化
    let lib = state
        .settings
        .lock()
        .unwrap()
        .active_library()
        .unwrap()
        .clone();
    assert_eq!(lib.photo_root, "Q:\\newroot");
    assert_eq!(
        settings_on_disk(&state).libraries[0].photo_root,
        "Q:\\newroot"
    );

    // 物理零变化：文件仍在原位，新根不存在
    assert!(old_root
        .path()
        .join("SmartPhoto")
        .join("2026")
        .join("a.jpg")
        .is_file());
    assert!(!Path::new("Q:\\newroot").exists());

    // 旧照片转 external 只读语义；外部/根外资产不受影响
    let database = common::open_db(db_dir.path());
    let origin = |path: &str| {
        database
            .0
            .query_row("SELECT origin FROM assets WHERE path = ?1", [path], |r| {
                r.get::<_, String>(0)
            })
            .unwrap()
    };
    let a = old_root
        .path()
        .join("SmartPhoto")
        .join("2026")
        .join("a.jpg")
        .to_string_lossy()
        .into_owned();
    assert_eq!(origin(&a), "external", "旧根下库内资产转 external");
    assert_eq!(origin("W:\\elsewhere\\c.jpg"), "external");
    assert_eq!(origin("Z:\\outside\\d.jpg"), "imported", "根外库内资产不变");
}

#[test]
fn photo_root_migrate_moves_files_and_updates_paths() {
    let old_root = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let new_root = tempfile::tempdir().unwrap();
    build_photo_library(old_root.path(), db_dir.path());
    let state = migration_state(db_dir.path(), old_root.path());

    migrate::photo_root_switch(
        &state,
        "lib-1",
        &new_root.path().to_string_lossy(),
        "migrate",
    )
    .unwrap();
    let (finished, started) = wait_migration_finished(&state);
    assert!(
        matches!(
            finished,
            AppEvent::MigrationFinished {
                ok: true,
                failed: 0
            }
        ),
        "{finished:?}"
    );
    assert!(matches!(
        started,
        Some(AppEvent::MigrationStarted { kind, total_bytes }) if kind == "photoRoot" && total_bytes == 20
    ));

    // 物理移动：旧根下文件消失、新根按原相对布局出现
    assert!(!old_root
        .path()
        .join("SmartPhoto")
        .join("2026")
        .join("a.jpg")
        .exists());
    assert!(new_root
        .path()
        .join("SmartPhoto")
        .join("2026")
        .join("a.jpg")
        .is_file());
    assert!(new_root
        .path()
        .join("SmartPhoto")
        .join("2026")
        .join("b.jpg")
        .is_file());

    // 资产路径批量前缀替换；external 与根外不动
    let database = common::open_db(db_dir.path());
    let count_at = |pattern: String| {
        database
            .0
            .query_row(
                "SELECT COUNT(*) FROM assets WHERE path LIKE ?1",
                [pattern],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
    };
    assert_eq!(
        count_at(format!("{}\\%", new_root.path().to_string_lossy())),
        2,
        "两个资产已指向新根"
    );
    assert_eq!(
        count_at(format!("{}\\%", old_root.path().to_string_lossy())),
        0,
        "旧根下不再有资产"
    );
    let still_at = |path: &str| {
        database
            .0
            .query_row("SELECT COUNT(*) FROM assets WHERE path = ?1", [path], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap()
    };
    assert_eq!(still_at("W:\\elsewhere\\c.jpg"), 1, "外部索引资产路径不动");
    assert_eq!(still_at("Z:\\outside\\d.jpg"), 1, "旧根外库内资产不动");

    // 注册表切到新根 + 任务落账
    let lib = state
        .settings
        .lock()
        .unwrap()
        .active_library()
        .unwrap()
        .clone();
    assert_eq!(lib.photo_root, new_root.path().to_string_lossy());
    let status: String = database
        .0
        .query_row(
            "SELECT status FROM jobs WHERE kind='photo-root-migrate'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "done");
    assert!(state.migrations.lock().unwrap().is_empty());
}

#[test]
fn photo_root_migrate_resumes_crashed_window() {
    let old_root = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let new_root = tempfile::tempdir().unwrap();
    build_photo_library(old_root.path(), db_dir.path());
    let state = migration_state(db_dir.path(), old_root.path());

    // 模拟崩溃窗口：a.jpg 物理已移动，但 journal 仍 pending、资产路径未更新
    let a_old = old_root
        .path()
        .join("SmartPhoto")
        .join("2026")
        .join("a.jpg");
    let a_new = new_root
        .path()
        .join("SmartPhoto")
        .join("2026")
        .join("a.jpg");
    std::fs::create_dir_all(a_new.parent().unwrap()).unwrap();
    std::fs::rename(&a_old, &a_new).unwrap();

    migrate::photo_root_switch(
        &state,
        "lib-1",
        &new_root.path().to_string_lossy(),
        "migrate",
    )
    .unwrap();
    let (finished, _) = wait_migration_finished(&state);
    assert!(
        matches!(
            finished,
            AppEvent::MigrationFinished {
                ok: true,
                failed: 0
            }
        ),
        "{finished:?}"
    );

    // 对账：a.jpg 资产路径补更新；b.jpg 正常补搬；注册表切换
    let database = common::open_db(db_dir.path());
    let a_row: String = database
        .0
        .query_row(
            "SELECT path FROM assets WHERE path LIKE ?1",
            [format!("{}\\%", new_root.path().to_string_lossy())],
            |r| r.get(0),
        )
        .unwrap();
    assert!(a_row.ends_with("a.jpg"), "a.jpg 资产应已指向新根: {a_row}");
    assert!(new_root
        .path()
        .join("SmartPhoto")
        .join("2026")
        .join("b.jpg")
        .is_file());
    assert!(!old_root
        .path()
        .join("SmartPhoto")
        .join("2026")
        .join("b.jpg")
        .exists());
    let lib = state
        .settings
        .lock()
        .unwrap()
        .active_library()
        .unwrap()
        .clone();
    assert_eq!(lib.photo_root, new_root.path().to_string_lossy());
}

#[test]
fn imports_rejected_while_library_migrating() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    common::build_many(src.path(), 2);
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));

    state.migrations.lock().unwrap().insert("lib-1".into());
    let err = ipc::start_import(&state, common::ipc_plan(&state, Path::new("X:\\t"))).unwrap_err();
    assert!(err.contains("迁移中"), "{err}");
    assert!(ipc::resume_import(&state, 1)
        .unwrap_err()
        .contains("迁移中"));
    state.migrations.lock().unwrap().remove("lib-1");
    // 解除后不再拦：幽灵设备报"不在线"（不会启动真实导入污染文件系统）
    let mut plan = common::ipc_plan(&state, Path::new("X:\\t"));
    plan.source_id = "ghost-device".into();
    let ok_err = ipc::start_import(&state, plan).unwrap_err();
    assert!(!ok_err.contains("迁移中"), "{ok_err}");
}

#[test]
fn migration_events_serialize_camel_case() {
    let started = serde_json::to_value(AppEvent::MigrationStarted {
        kind: "dbDir".into(),
        total_bytes: 123,
    })
    .unwrap();
    assert_eq!(started["type"], "migrationStarted");
    assert_eq!(started["kind"], "dbDir");
    assert_eq!(started["totalBytes"], 123);

    let progress = serde_json::to_value(AppEvent::MigrationProgress {
        done_bytes: 10,
        current: "a.jpg".into(),
    })
    .unwrap();
    assert_eq!(progress["type"], "migrationProgress");
    assert_eq!(progress["doneBytes"], 10);
    assert_eq!(progress["current"], "a.jpg");

    let finished = serde_json::to_value(AppEvent::MigrationFinished {
        ok: true,
        failed: 0,
    })
    .unwrap();
    assert_eq!(finished["type"], "migrationFinished");
    assert_eq!(finished["ok"], true);
    assert_eq!(finished["failed"], 0);
}
