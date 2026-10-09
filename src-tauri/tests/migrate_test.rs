//! migrate 体系退役后的单版本 schema 语义（2026-10-09 §二定案）：建表脚本
//! 即唯一版本——打开幂等（重复打开零变更、不写 user_version、不产生
//! `.db-migration.json` 迁移标记）；旧库并发首开由 IF NOT EXISTS +
//! busy_timeout 收敛，无迁移锁。旧 dbDir/photoRoot 迁移命令随库注册表
//! 一并退役，不再有任何迁移路径。

mod common;

pub use common::{
    ai, bursts, db, devices, events, geo, import, index, ipc, metadata, platform, scan, settings,
    tasks, tethering, thumbs,
};

/// 打开即建表：重复打开幂等——对象清单不变、无迁移标记文件产生。
#[test]
fn single_version_schema_reopens_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let first = common::open_db(dir.path());
    let objects: Vec<String> = first
        .0
        .prepare("SELECT name FROM sqlite_master WHERE type IN ('table','index') ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    drop(first);

    // 二次打开（同进程不同连接）：幂等，无新增对象、无迁移标记
    let second = common::open_db(dir.path());
    let objects_again: Vec<String> = second
        .0
        .prepare("SELECT name FROM sqlite_master WHERE type IN ('table','index') ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(objects, objects_again, "重复打开不得增删对象");
    for marker in [".db-migration.json", ".db-migration.json.tmp", ".migrated-bak"] {
        assert!(
            !dir.path().join(marker).exists(),
            "单版本 schema 不得产生迁移标记：{marker}"
        );
    }

    // 并发再开（首开建表后多连接同时打开）：WAL 读并发无迁移锁
    let concurrent_dir = tempfile::tempdir().unwrap();
    drop(common::open_db(concurrent_dir.path())); // 首开建表
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let path = concurrent_dir.path().to_path_buf();
            std::thread::spawn(move || common::open_db(&path).photos_library_list().is_ok())
        })
        .collect();
    for handle in handles {
        assert!(handle.join().expect("concurrent open thread"), "并发再开应成功");
    }
}

/// photos_libraries 登记表就位：登记/查询/统计缓存口径（M1 数据层承载
/// settings 退役后的库注册表职责）。
#[test]
fn photos_libraries_registry_is_the_new_library_roster() {
    let dir = tempfile::tempdir().unwrap();
    let db = common::open_db(dir.path());
    assert!(db.photos_library_list().unwrap().is_empty());

    // root 必须在数据库目录之外（§八-6 互斥：root 不得与数据库目录互相包含）
    let photos = tempfile::tempdir().unwrap();
    let root = photos.path().to_path_buf();
    let row = db
        .photos_library_register("主库", &root.to_string_lossy(), dir.path())
        .unwrap();
    assert_eq!(row.status, "online");
    assert_eq!(row.asset_count, 0);
    assert_eq!(db.photos_library_list().unwrap().len(), 1);

    // root 互斥：同 root 二次登记拒绝；数据库目录内 root 拒绝（§八-6）
    let err = db
        .photos_library_register("重复", &root.to_string_lossy(), dir.path())
        .unwrap_err();
    assert!(err.contains("相同或互相包含"), "{err}");
    let inside_db = dir.path().join("photos-inside");
    let err2 = db
        .photos_library_register("越界", &inside_db.to_string_lossy(), dir.path())
        .unwrap_err();
    assert!(err2.contains("数据库目录"), "{err2}");

    // 统计缓存重算：登记一条库内资产后 asset_count/size_bytes 就位
    db.0.execute(
        "INSERT INTO assets (path, filename, size, mtime, xxhash, kind, source, created_at, \
         origin, library_id) VALUES ('x.jpg','x.jpg',42,'2026-01-01T00:00:00Z',1,'photo',\
         'imported','2026-01-01T00:00:00Z','imported',?1)",
        [&row.id],
    )
    .unwrap();
    db.photos_library_refresh_stats(&row.id).unwrap();
    let refreshed = db.photos_library_get(&row.id).unwrap().unwrap();
    assert_eq!(refreshed.asset_count, 1);
    assert_eq!(refreshed.size_bytes, 42);
}
