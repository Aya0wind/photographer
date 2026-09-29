//! 库照片存储目录整体重定位（2026-09-28 用户定案）：db 层前缀重写
//! （大小写/斜杠不敏感、尾段原样、兄弟目录不误伤）+ thumb 任务重排 +
//! ipc 层预检/执行/幂等重试/配置落盘语义。

mod common;

pub use common::{
    platform,
    ai, bursts, db, devices, events, import, index, geo, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::path::Path;

use common::state_with_library;
use db::AssetRow;
use events::AssetKind;
use rusqlite::params;

fn row(path: &str) -> AssetRow {
    AssetRow {
        path: path.into(),
        filename: Path::new(path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        size: 1,
        mtime: "2026-09-28T00:00:00.000Z".into(),
        xxhash: 1,
        kind: AssetKind::Photo,
        captured_at: None,
        camera: None,
        source: "imported".into(),
        created_at: "2026-09-28T00:00:00.000Z".into(),
        origin: "imported".into(),
        width: None,
        height: None,
        iso: None,
        f_number: None,
        exposure_time: None,
        focal_length: None,
        lens: None,
        pair_asset_id: None,
        thumb_state: 1,
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

/// 按 path 查资产 id。
fn id_of(database: &db::Db, path: &str) -> i64 {
    database
        .0
        .query_row("SELECT id FROM assets WHERE path = ?1", params![path], |r| r.get(0))
        .unwrap()
}

#[test]
fn strip_root_prefix_matches_case_and_slash_insensitive_but_not_siblings() {
    use db::strip_root_prefix as strip;
    // 大小写/斜杠方向不敏感
    assert_eq!(strip(r"I:\Photos\2026\a.jpg", r"i:/photos"), Some(r"2026\a.jpg"));
    // 兄弟目录不误伤（I:\x 不得匹配 I:\x2\...）
    assert_eq!(strip(r"I:\x2\a.jpg", r"I:\x"), None);
    // 尾分隔符归一
    assert_eq!(strip(r"I:\x\a.jpg", r"I:\x\"), Some("a.jpg"));
    // 恰好等于根（防御路径）
    assert_eq!(strip(r"I:\x", r"I:\x"), Some(""));
    // 前缀不同
    assert_eq!(strip(r"J:\x\a.jpg", r"I:\x"), None);
}

#[test]
fn dry_run_counts_without_writing() {
    let root = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = state_with_library(db_dir.path(), root.path(), std::time::Duration::ZERO);
    let database = common::open_db(db_dir.path());
    database
        .insert_asset(&row(&root.path().join("a.jpg").to_string_lossy()))
        .unwrap();
    database
        .insert_asset(&row(&format!(r"{}\sub\b.jpg", root.path().to_string_lossy())))
        .unwrap();
    database.insert_asset(&row(r"C:\elsewhere\c.jpg")).unwrap();
    drop(database);

    let new_root = db_dir.path().join("new-root");
    let before = ipc::settings::fetch_library_relocate(
        &state,
        "lib-1",
        new_root.to_str().unwrap(),
        false,
    )
    .unwrap();
    assert_eq!(before.affected, 2, "旧根下 2 张");
    assert_eq!(before.unaffected, 1, "外部根 1 张不动");
    assert!(!before.root_exists, "新根未创建");

    // 预检零写入：外部路径原样
    let database = common::open_db(db_dir.path());
    let kept: i64 = database
        .0
        .query_row(
            "SELECT COUNT(*) FROM assets WHERE path = 'C:\\elsewhere\\c.jpg'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    drop(database);
    assert_eq!(kept, 1);
}

#[test]
fn apply_rewrites_paths_resets_thumbs_and_updates_settings() {
    let root = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = state_with_library(db_dir.path(), root.path(), std::time::Duration::ZERO);
    let old_root = root.path().to_string_lossy().into_owned();
    let database = common::open_db(db_dir.path());
    let path_a = format!(r"{}\a.jpg", old_root);
    database.insert_asset(&row(&path_a)).unwrap();
    // 正斜杠形态的库内路径（历史导入遗留）也必须命中
    let path_b = format!("{}/2026/b.jpg", old_root.replace('\\', "/"));
    database.insert_asset(&row(&path_b)).unwrap();
    database.insert_asset(&row(r"C:\elsewhere\c.jpg")).unwrap();
    let a = id_of(&database, &path_a);
    let outside = id_of(&database, r"C:\elsewhere\c.jpg");
    drop(database);

    let new_root = db_dir.path().join("new-root");
    std::fs::create_dir_all(&new_root).unwrap();
    let dto = ipc::settings::fetch_library_relocate(&state, "lib-1", new_root.to_str().unwrap(), true)
        .unwrap();
    assert_eq!(dto.affected, 2);
    assert_eq!(dto.unaffected, 1);
    assert!(dto.root_exists);

    let database = common::open_db(db_dir.path());
    let (got_a, ts_a): (String, i32) = database
        .0
        .query_row("SELECT path, thumb_state FROM assets WHERE id = ?1", params![a], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(
        got_a,
        new_root.join("a.jpg").to_string_lossy().into_owned(),
        "尾段原样保留，前缀换新根"
    );
    assert_eq!(ts_a, 0, "重写行缩略图状态复位待重建");
    let pending: i64 = database
        .0
        .query_row(
            "SELECT COUNT(*) FROM index_tasks WHERE kind = 'thumb' AND asset_id = ?1",
            params![a],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pending, 1, "重写行恰一条缩略图任务（apply 后台 worker 可能已消费，状态不限）");
    let (path_out, ts_out): (String, i32) = database
        .0
        .query_row("SELECT path, thumb_state FROM assets WHERE id = ?1", params![outside], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(path_out, r"C:\elsewhere\c.jpg");
    assert_eq!(ts_out, 1, "外部行不动（含缩略图状态）");
    drop(database);

    // 配置已更新（内存 + 落盘）
    let expected_new = new_root.to_string_lossy().into_owned();
    {
        let settings = state.settings.lock().unwrap();
        assert_eq!(settings.libraries[0].photo_root, expected_new);
    }
    let saved = settings::SettingsManager::load(&state.config_dir).unwrap();
    assert_eq!(saved.libraries[0].photo_root, expected_new);

    // 同根再执行：计数照常（都在新根下）但零副作用（路径/任务不变）
    let again =
        ipc::settings::fetch_library_relocate(&state, "lib-1", new_root.to_str().unwrap(), true)
            .unwrap();
    assert_eq!(again.affected, 2, "同根重试只读计数");
    let database = common::open_db(db_dir.path());
    let (got_a2, _): (String, i32) = database
        .0
        .query_row("SELECT path FROM assets WHERE id = ?1", params![a], |r| {
            Ok((r.get::<_, String>(0)?, 0))
        })
        .unwrap();
    assert_eq!(got_a2, new_root.join("a.jpg").to_string_lossy().into_owned(), "路径未被再次改写");
    let pending2: i64 = database
        .0
        .query_row(
            "SELECT COUNT(*) FROM index_tasks WHERE kind = 'thumb' AND asset_id = ?1",
            params![a],
            |r| r.get(0),
        )
        .unwrap();
    drop(database);
    assert_eq!(pending2, 1, "缩略图任务不重复入账（同根重试零写入）");
}

#[test]
fn rejects_relative_and_missing_library() {
    let root = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = state_with_library(db_dir.path(), root.path(), std::time::Duration::ZERO);
    let err =
        ipc::settings::fetch_library_relocate(&state, "lib-1", "I:relative", false).unwrap_err();
    assert!(err.contains("绝对路径"), "盘符相对路径拒绝：{err}");
    let err =
        ipc::settings::fetch_library_relocate(&state, "no-such-lib", r"I:\x", false).unwrap_err();
    assert!(err.contains("库不存在"));
}
