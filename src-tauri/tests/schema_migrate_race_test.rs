//! 新库并发首开迁移竞态回归（2026-09-29 实测：删库重建后导入选「新建相册」
//! 报「库迁移失败： table album already exists」）。
//!
//! 根因：[`db::Db::migrate`] 先读一次 user_version 再顺序执行；新库首开时
//! 前端并发 IPC 与后台 worker 各开一条连接同时从 0 起跑，后提交者在
//! 0001（CREATE TABLE album）撞表。修复 = 迁移全程持进程级锁，后到者
//! 锁内重读版本即 no-op。本测试用 SMARTPHOTO_MIGRATE_RACE_TEST_MS 把
//! 「读版本 → 首个建表」窗口放大到确定性复现（同 DL 故障注入风格）。

mod common;

pub use common::{
    platform, ai, bursts, db, devices, events, import, index, geo, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::sync::{Arc, Barrier};

/// 测试期间设置竞态窗口放大器；Drop 兜底清理，防泄漏拖慢同进程其他测试
/// （每个测试都开新库迁移，环境变量残留会让全部用例多睡一次）。
struct RaceWindow;

impl RaceWindow {
    fn arm(ms: &str) -> Self {
        std::env::set_var("SMARTPHOTO_MIGRATE_RACE_TEST_MS", ms);
        RaceWindow
    }
}

impl Drop for RaceWindow {
    fn drop(&mut self) {
        std::env::remove_var("SMARTPHOTO_MIGRATE_RACE_TEST_MS");
    }
}

#[test]
fn concurrent_first_open_never_clashes_on_create_table() {
    let _window = RaceWindow::arm("1500");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.db");

    // 8 条连接在屏障处同时起跑：模拟建库后第一波并发 IPC / worker 首开。
    let barrier = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            let path = path.clone();
            std::thread::spawn(move || {
                barrier.wait();
                // 生产同款入口（open+migrate 同锁串行），而非 open 后再 migrate。
                db::Db::open_migrated(&path)
            })
        })
        .collect();
    for handle in handles {
        handle
            .join()
            .expect("migrate 线程 panic")
            .expect("并发首开迁移应全部成功，不得撞 table already exists");
    }

    // 终态：版本推进到位 + 关键表真实可查（空表）。
    let db = db::Db::open(&path).unwrap();
    db.migrate().unwrap();
    let version: i64 = db
        .0
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert!(version >= 1, "user_version 未推进: {version}");
    for table in ["album", "album_item", "assets"] {
        let n: i64 = db
            .0
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap_or(i64::MIN);
        assert_eq!(n, 0, "{table} 应存在且为空表");
    }
}
