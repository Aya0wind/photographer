//! 库内增量扫描专项（M2b，计划 §三/§八 1/2/3/5/7）：目录 mtime 剪枝、
//! 不跟随符号链接/junction、文件稳定度冷却窗（含跨轮收敛）、missing 两轮
//! 延迟确认、missing 哈希/file-id 重绑（边车补写 + 缩略图复用）、同名
//! 不同内容恢复校验、晚到边车补读、登记单管道并发、导入让路。

mod common;

pub use common::{
    ai, bursts, db, devices, events, geo, import, index, ipc, metadata, platform,
    scan, settings, tasks, thumbs,
};

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::open_db;
use db::libraries::PhotosLibraryRow;

/// 测试库 fixture：临时 db 目录 + 照片库 root（登记于应用唯一数据库）。
fn setup(name: &str) -> (tempfile::TempDir, db::Db, PhotosLibraryRow, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    let photos = dir.path().join("photos");
    fs::create_dir_all(&db_dir).unwrap();
    fs::create_dir_all(&photos).unwrap();
    let database = open_db(&db_dir);
    let library = database
        .photos_library_register(name, &photos.to_string_lossy(), &db_dir)
        .unwrap();
    (dir, database, library, db_dir)
}

/// 冷却关闭的扫描选项（文件 mtime 判定直通；mtime 剪枝与内容识别不受影响）。
fn relaxed() -> scan::ScanOptions {
    scan::ScanOptions {
        cooldown: Duration::ZERO,
        now: None,
    }
}

/// 跑一轮（库行现取——status 可能被上轮翻转）。
fn run(db: &db::Db, db_dir: &Path, library_id: &str) -> scan::LibraryScanReport {
    run_with(db, db_dir, library_id, relaxed())
}

fn run_with(
    db: &db::Db,
    db_dir: &Path,
    library_id: &str,
    options: scan::ScanOptions,
) -> scan::LibraryScanReport {
    let library = db.photos_library_get(library_id).unwrap().unwrap();
    scan::scan_library_once(db, db_dir, &library, &options, None, None).unwrap()
}

/// 唯一内容的合法 JPEG（魔数 + 唯一化字节；两文件不同 (size, xxh)）。
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

fn asset_row(db: &db::Db, asset_id: i64) -> (String, bool, i64, i64) {
    db.0.query_row(
        "SELECT path, missing, rating, xmp_dirty FROM assets WHERE id = ?1",
        [asset_id],
        |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get(2)?, r.get(3)?)),
    )
    .unwrap()
}

fn asset_id_by_path(db: &db::Db, path: &str) -> i64 {
    db.asset_id_by_path(path).unwrap().unwrap()
}

fn count_assets(db: &db::Db) -> i64 {
    db.0.query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0))
        .unwrap()
}

/// asset_metadata.value JSON 的 keywords 数组（无记录/损坏 → 空）。
fn asset_keywords(db: &db::Db, asset_id: i64) -> Vec<String> {
    #[derive(serde::Deserialize)]
    struct KeywordsOnly {
        #[serde(default)]
        keywords: Vec<String>,
    }
    db.0
        .query_row(
            "SELECT value FROM asset_metadata WHERE asset_id = ?1",
            [asset_id],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|text| serde_json::from_str::<KeywordsOnly>(&text).ok())
        .map(|parsed| parsed.keywords)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// 登记幂等 + 目录 mtime 剪枝（§三）
// ---------------------------------------------------------------------------

#[test]
fn scan_registers_new_files_then_prunes_unchanged_dirs() {
    let (dir, database, library, db_dir) = setup("剪枝");
    let root = dir.path().join("photos");
    write(&root.join("2026/10/a.jpg"), 1);
    write(&root.join("2026/10/b.nef"), 2);

    let first = run(&database, &db_dir, &library.id);
    assert_eq!(first.registered, 2, "新文件全部登记");
    assert_eq!(first.cooled, 0);
    assert_eq!(count_assets(&database), 2);
    // 登记指纹已记录（NTFS 卷有 file id；exFAT 测试环境退化为 NULL 也合法，
    // 此处只断言不 panic——指纹可用性由平台单测覆盖）
    assert!(first.online);

    // 无变化第二轮：目录 mtime 未变 → 剪枝（零处理零跳过）
    let second = run(&database, &db_dir, &library.id);
    assert_eq!(second.registered, 0, "幂等：二次扫描零变更");
    assert_eq!(second.skipped, 0, "剪枝目录不枚举不查重");
    assert_eq!(second.cooled, 0);

    // 新文件落入既有子目录（目录 mtime 变化 → 重枚举该目录）
    write(&root.join("2026/10/c.jpg"), 3);
    let third = run(&database, &db_dir, &library.id);
    assert_eq!(third.registered, 1);
    assert_eq!(count_assets(&database), 3);

    // 深层新目录（父目录 mtime 变化 → 递归进入新目录）
    write(&root.join("2026/11/deep/d.jpg"), 4);
    let fourth = run(&database, &db_dir, &library.id);
    assert_eq!(fourth.registered, 1);
    assert_eq!(count_assets(&database), 4);
}

// ---------------------------------------------------------------------------
// 冷却窗（§八-2）：mtime 距今小于窗 → 本轮跳过；跨轮收敛（目录不被剪枝
// 缓存饿死）
// ---------------------------------------------------------------------------

#[test]
fn cooldown_window_skips_fresh_files_until_elapsed() {
    let (dir, database, library, db_dir) = setup("冷却");
    let root = dir.path().join("photos");
    write(&root.join("a.jpg"), 1);
    let now = SystemTime::now();

    let cooling = scan::ScanOptions {
        cooldown: Duration::from_secs(10),
        now: Some(now),
    };
    let first = run_with(&database, &db_dir, &library.id, cooling.clone());
    assert_eq!(first.cooled, 1, "mtime 距今 0s → 冷却跳过");
    assert_eq!(first.registered, 0);
    assert_eq!(count_assets(&database), 0);

    // 时间前进越过冷却窗（目录 mtime 未变，但上轮未收敛 → 强制重枚举）
    let elapsed = scan::ScanOptions {
        cooldown: Duration::from_secs(10),
        now: Some(now + Duration::from_secs(11)),
    };
    let second = run_with(&database, &db_dir, &library.id, elapsed);
    assert_eq!(second.cooled, 0, "跨轮收敛：过窗文件被拾取");
    assert_eq!(second.registered, 1);
    assert_eq!(count_assets(&database), 1);
}

// ---------------------------------------------------------------------------
// missing 两轮延迟确认（§八-7）
// ---------------------------------------------------------------------------

#[test]
fn missing_marked_only_after_two_rounds() {
    let (dir, database, library, db_dir) = setup("两轮");
    let root = dir.path().join("photos");
    write(&root.join("a.jpg"), 1);
    write(&root.join("b.jpg"), 2);
    run(&database, &db_dir, &library.id);
    let a = asset_id_by_path(&database, &root.join("a.jpg").to_string_lossy());

    fs::remove_file(root.join("a.jpg")).unwrap();
    let first = run(&database, &db_dir, &library.id);
    assert_eq!(first.missing_marked, 0, "首轮缺席只记账不标缺");
    assert_eq!(asset_row(&database, a).1, false, "首轮不误报");

    let second = run(&database, &db_dir, &library.id);
    assert_eq!(second.missing_marked, 1, "次轮仍在 → 标 missing");
    assert_eq!(asset_row(&database, a).1, true);
    // 幂等：已 missing 不重复标
    let third = run(&database, &db_dir, &library.id);
    assert_eq!(third.missing_marked, 0);
}

// ---------------------------------------------------------------------------
// missing 哈希重绑（§八-1）：真 rename（file-id 命中）与复制（哈希命中）
// ---------------------------------------------------------------------------

#[test]
fn rebind_after_real_rename_via_file_id_keeps_logic_and_writes_sidecar() {
    let (dir, database, library, db_dir) = setup("重绑-rename");
    let root = dir.path().join("photos");
    let a = root.join("2026").join("a.jpg");
    write(&a, 1);
    run(&database, &db_dir, &library.id);
    let id = asset_id_by_path(&database, &a.to_string_lossy());
    // 逻辑元数据 + 离线期改动（重绑应补写边车并保留星级）
    database
        .0
        .execute(
            "UPDATE assets SET rating = 5, xmp_dirty = 1 WHERE id = ?1",
            [id],
        )
        .unwrap();

    // 用户真改名（同一物理文件：file-id 不变；哈希/大小不变）。新名字立刻
    // 在盘：轮1 = 旧名缺席记账（新名 file-id 命中在线资产 → 硬链接直跳）；
    // 轮2 = 旧名标 missing + 新名命中 missing 指纹 → 重绑（同轮原子收敛）。
    fs::rename(&a, root.join("2026").join("renamed.jpg")).unwrap();
    let first = run(&database, &db_dir, &library.id);
    assert_eq!(first.missing_marked, 0, "首轮缺席只记账");
    assert_eq!(first.skipped, 1, "新名字先按 file-id 直跳（在线指纹）");
    let report = run(&database, &db_dir, &library.id);
    assert_eq!(report.rebound, 1, "次轮标缺 + 新位置命中 missing 指纹 → 重绑");
    let (path, missing, rating, xmp_dirty) = asset_row(&database, id);
    assert_eq!(path, root.join("2026").join("renamed.jpg").to_string_lossy());
    assert_eq!(missing, false);
    assert_eq!(rating, 5, "逻辑元数据保留");
    assert_eq!(xmp_dirty, 0, "边车补写后清标志");
    // 边车已补写到新位置（与 rating 写入方向同路径：DB 真值投影）
    let sidecar = root.join("2026").join("renamed.xmp");
    let text = fs::read_to_string(&sidecar).unwrap();
    assert_eq!(metadata::xmp::sidecar_rating(&text), Some(5));
    assert_eq!(count_assets(&database), 1, "不产生第二行");
}

#[test]
fn rebind_after_copy_via_hash_reuses_thumbnails_by_content() {
    let (dir, database, library, db_dir) = setup("重绑-copy");
    let root = dir.path().join("photos");
    let a = root.join("a.jpg");
    write(&a, 1);
    run(&database, &db_dir, &library.id);
    let id = asset_id_by_path(&database, &a.to_string_lossy());

    // 旧路径缩略图缓存（直接落缓存键文件；键规则 = 源路径小写 xxh64-
    // <mtime 秒>，与 thumbs::cache_path 同式）
    let old_cache = thumb_cache_path(&db_dir, &a, 12345);
    fs::create_dir_all(old_cache.parent().unwrap()).unwrap();
    fs::write(&old_cache, b"thumb-bytes").unwrap();

    // 删原件 → 两轮 missing；同内容复制件出现在新名（新 inode：file-id
    // 不命中，走同库哈希层命中 missing 资产 → 重绑）
    fs::remove_file(&a).unwrap();
    run(&database, &db_dir, &library.id);
    run(&database, &db_dir, &library.id);
    assert_eq!(asset_row(&database, id).1, true);
    let moved = root.join("moved.jpg");
    fs::write(&moved, jpg(1)).unwrap();

    let report = run(&database, &db_dir, &library.id);
    assert_eq!(report.rebound, 1);
    assert_eq!(asset_row(&database, id).1, false);
    // 缩略图按内容复用：旧路径键缓存迁到新路径键（mtime 段保留）
    assert!(!old_cache.exists(), "旧键缓存已迁走");
    let new_cache = thumb_cache_path(&db_dir, &moved, 12345);
    assert_eq!(
        fs::read(&new_cache).unwrap(),
        b"thumb-bytes",
        "新键缓存复用旧内容"
    );
}

/// 与 thumbs::cache_path 同式的缓存键路径（256 档位；Windows 键 = 路径
/// 小写 xxh64——测试钉死键规则，规则变更时此处提醒同步 rebind 逻辑）。
fn thumb_cache_path(db_dir: &Path, src: &Path, mtime_secs: u64) -> PathBuf {
    let mut xxh = xxhash_rust::xxh64::Xxh64::new(0);
    #[cfg(windows)]
    xxh.update(src.to_string_lossy().to_lowercase().as_bytes());
    #[cfg(not(windows))]
    xxh.update(src.to_string_lossy().as_bytes());
    db_dir
        .join("thumbs")
        .join("256")
        .join(format!("{:016x}-{mtime_secs}.jpg", xxh.digest()))
}

// ---------------------------------------------------------------------------
// 恢复校验（§八-1）：同哈希恢复 / 同名不同内容重算索引保逻辑
// ---------------------------------------------------------------------------

#[test]
fn restore_same_path_same_hash_clears_missing() {
    let (dir, database, library, db_dir) = setup("恢复-同哈希");
    let root = dir.path().join("photos");
    let a = root.join("a.jpg");
    write(&a, 1);
    run(&database, &db_dir, &library.id);
    let id = asset_id_by_path(&database, &a.to_string_lossy());

    fs::remove_file(&a).unwrap();
    run(&database, &db_dir, &library.id);
    run(&database, &db_dir, &library.id);
    assert_eq!(asset_row(&database, id).1, true);

    // 放回同内容（mtime 变化不影响：哈希一致 → 单纯恢复）
    write(&a, 1);
    let report = run(&database, &db_dir, &library.id);
    assert_eq!(report.restored, 1);
    assert_eq!(report.rebound, 0);
    assert_eq!(asset_row(&database, id).1, false);
    assert_eq!(count_assets(&database), 1);
}

#[test]
fn restore_same_path_new_content_recomputes_index_keeps_logic() {
    let (dir, database, library, db_dir) = setup("恢复-新内容");
    let root = dir.path().join("photos");
    let a = root.join("a.jpg");
    write(&a, 1);
    run(&database, &db_dir, &library.id);
    let id = asset_id_by_path(&database, &a.to_string_lossy());
    database
        .0
        .execute(
            "UPDATE assets SET rating = 4, width = 100 WHERE id = ?1",
            [id],
        )
        .unwrap();
    let old_hash: i64 = database
        .0
        .query_row("SELECT xxhash FROM assets WHERE id = ?1", [id], |r| r.get(0))
        .unwrap();

    fs::remove_file(&a).unwrap();
    run(&database, &db_dir, &library.id);
    run(&database, &db_dir, &library.id);
    assert_eq!(asset_row(&database, id).1, true);

    // 放回同名不同内容：校验哈希不一致 → 重算索引、保留逻辑元数据并提示
    let bus = events::EventBus::new();
    let mut rx = bus.subscribe();
    let library_row = database.photos_library_get(&library.id).unwrap().unwrap();
    write(&a, 2);
    let report = scan::scan_library_once(
        &database,
        &db_dir,
        &library_row,
        &relaxed(),
        None,
        Some(&bus),
    )
    .unwrap();
    assert_eq!(report.recomputed, 1);
    let (path, missing, rating, _) = asset_row(&database, id);
    assert_eq!(path, a.to_string_lossy());
    assert_eq!(missing, false);
    assert_eq!(rating, 4, "逻辑元数据（评分）保留");
    let new_hash: i64 = database
        .0
        .query_row("SELECT xxhash FROM assets WHERE id = ?1", [id], |r| r.get(0))
        .unwrap();
    assert_ne!(old_hash, new_hash, "内容指纹已按新内容更新");
    let (width, thumb_state): (Option<i64>, i64) = database
        .0
        .query_row(
            "SELECT width, thumb_state FROM assets WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(width.is_none(), "深 EXIF 列清空待重提取");
    assert_eq!(thumb_state, 0, "缩略图状态复位");
    let pending: i64 = database
        .0
        .query_row(
            "SELECT COUNT(*) FROM index_tasks WHERE asset_id = ?1 \
             AND kind IN ('thumb', 'exif', 'phash') AND state = 'pending'",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pending, 3, "CPU 索引通道全部重排");
    // 提示事件（warn，面向用户）
    let mut warned = false;
    while let Ok(event) = rx.try_recv() {
        if let events::AppEvent::AppError { level, message, .. } = event {
            if level == "warn" && message.contains("内容已变化") {
                warned = true;
            }
        }
    }
    assert!(warned, "恢复校验不一致应发用户提示");
}

// ---------------------------------------------------------------------------
// 晚到边车补读（§八-3）：与 rating_watch 读入方向同路径，触发一次
// ---------------------------------------------------------------------------

#[test]
fn late_sidecar_backfills_rating_once_until_updated() {
    let (dir, database, library, db_dir) = setup("晚到边车");
    let root = dir.path().join("photos");
    let a = root.join("a.jpg");
    write(&a, 1);
    run(&database, &db_dir, &library.id);
    let id = asset_id_by_path(&database, &a.to_string_lossy());
    assert_eq!(asset_row(&database, id).2, 0, "登记时无边车 → 0 星");

    // 边车晚于登记时间到达（LR 文件夹边车后到场景）：星级 + 颜色 + 关键字
    //（§三/§六 读入方向——关键字落 asset_metadata，与星级/颜色同规则）
    std::thread::sleep(Duration::from_millis(30));
    fs::write(
        root.join("a.xmp"),
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmp:Rating="4" xmp:Label="Red"><dc:subject><rdf:Bag><rdf:li>旅行</rdf:li><rdf:li>wedding</rdf:li></rdf:Bag></dc:subject></rdf:Description></rdf:RDF></x:xmpmeta>"#,
    )
    .unwrap();
    let first = run(&database, &db_dir, &library.id);
    assert_eq!(first.sidecar_read, 1, "晚到边车触发一次读入");
    assert_eq!(asset_row(&database, id).2, 4, "星级回填（DB 无值才写）");
    assert_eq!(
        asset_keywords(&database, id),
        vec!["旅行".to_string(), "wedding".to_string()],
        "关键字回填 asset_metadata"
    );

    // 同边车下轮不重读（触发一次；目录因写边车重枚举但 mtime 已记账）
    let second = run(&database, &db_dir, &library.id);
    assert_eq!(second.sidecar_read, 0);

    // LR 再改边车（mtime 变化）→ 再读一次；DB 已有值不被覆盖（应用内优先）
    std::thread::sleep(Duration::from_millis(30));
    fs::write(
        root.join("a.xmp"),
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmp:Rating="2"><dc:subject><rdf:Bag><rdf:li>别的关键字</rdf:li></rdf:Bag></dc:subject></rdf:Description></rdf:RDF></x:xmpmeta>"#,
    )
    .unwrap();
    let third = run(&database, &db_dir, &library.id);
    assert_eq!(third.sidecar_read, 1, "边车更新 → 再读一次");
    assert_eq!(asset_row(&database, id).2, 4, "DB 已有评分不被边车覆盖");
    assert_eq!(
        asset_keywords(&database, id),
        vec!["旅行".to_string(), "wedding".to_string()],
        "DB 已有关键字不被边车覆盖（应用内值优先）"
    );
}

// ---------------------------------------------------------------------------
// 两级识别（§三）：file-id 命中 = 硬链接直跳（库内导出物零哈希）
// ---------------------------------------------------------------------------

#[test]
#[cfg(windows)]
fn hardlink_export_is_skipped_by_file_id() {
    let (dir, database, library, db_dir) = setup("硬链接");
    let root = dir.path().join("photos");
    let a = root.join("a.jpg");
    write(&a, 1);
    let first = run(&database, &db_dir, &library.id);
    assert_eq!(first.registered, 1);

    // 用户把库内文件硬链接导出到库内另一名字（同卷同 file id）
    std::fs::hard_link(&a, root.join("export.jpg")).unwrap();
    let second = run(&database, &db_dir, &library.id);
    assert_eq!(second.registered, 0, "file-id 命中 → 零哈希直跳");
    assert_eq!(count_assets(&database), 1, "被跳过的路径对库完全不可见");
}

// ---------------------------------------------------------------------------
// 不跟随符号链接/junction（§八-7）
// ---------------------------------------------------------------------------

#[test]
#[cfg(windows)]
fn junction_into_library_is_not_followed() {
    let (dir, database, library, db_dir) = setup("junction");
    let root = dir.path().join("photos");
    // 库外目录带图片；junction 指进库内
    let outside = dir.path().join("outside");
    write(&outside.join("linked.jpg"), 1);
    let link = root.join("junction-link");
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&outside)
        .output()
        .unwrap();
    assert!(status.status.success(), "mklink /J 应可用（无需管理员）");

    let report = run(&database, &db_dir, &library.id);
    assert_eq!(report.registered, 0, "junction 目标内容不登记");
    assert_eq!(count_assets(&database), 0);
}

// ---------------------------------------------------------------------------
// 库上线状态翻转（§五 整库粒度）
// ---------------------------------------------------------------------------

#[test]
fn offline_root_flips_status_and_recovers_when_back() {
    let (holder, database, library, db_dir) = setup("offline");
    let root = holder.path().join("photos");
    write(&root.join("a.jpg"), 1);
    assert!(run(&database, &db_dir, &library.id).online);

    // 模拟拔盘：根目录消失，但登记路径不变。
    fs::remove_dir_all(&root).unwrap();
    let report = run(&database, &db_dir, &library.id);
    assert!(!report.online, "根不在盘 → online=false");
    assert_eq!(
        database
            .photos_library_get(&library.id)
            .unwrap()
            .unwrap()
            .status,
        "offline"
    );

    // 根回来（重挂载）→ online 翻回；同库资产因根消失进入缺席流程
    write(&root.join("a.jpg"), 1);
    let back = run(&database, &db_dir, &library.id);
    assert!(back.online);
    assert_eq!(
        database
            .photos_library_get(&library.id)
            .unwrap()
            .unwrap()
            .status,
        "online"
    );
}

// ---------------------------------------------------------------------------
// 文件级零哈希预跳过（AfterFrame changed-media 借鉴）：(size, mtime) 与登记
// 指纹一致 → 零动作直过；命中 missing 指纹 → 零整读重绑
// ---------------------------------------------------------------------------

#[test]
fn unchanged_file_fingerprint_skips_when_dir_reenumerated() {
    let (dir, database, library, db_dir) = setup("指纹未变");
    let root = dir.path().join("photos");
    write(&root.join("a.jpg"), 1);
    run(&database, &db_dir, &library.id);

    // 目录 mtime 变化（新文件落入）触发重枚举：未变文件 (size, mtime) 与登记
    // 指纹一致 → 零动作（不进任何计数，不算新文件也不算消失）
    write(&root.join("b.jpg"), 2);
    let report = run(&database, &db_dir, &library.id);
    assert_eq!(report.registered, 1, "仅新文件登记");
    assert_eq!(report.skipped, 0, "未变文件零动作直过（不计 skip）");
    assert_eq!(report.cooled, 0);
    assert_eq!(report.rebound, 0);

    // 边车语义保留（§八-3）：预跳过不饿死晚到边车——未变文件旁出现晚于
    // 登记时间的边车仍被读入
    let id = asset_id_by_path(&database, &root.join("a.jpg").to_string_lossy());
    std::thread::sleep(Duration::from_millis(30));
    fs::write(
        root.join("a.xmp"),
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="3"></rdf:Description></rdf:RDF></x:xmpmeta>"#,
    )
    .unwrap();
    let with_sidecar = run(&database, &db_dir, &library.id);
    assert_eq!(with_sidecar.sidecar_read, 1, "晚到边车不被预跳过饿死");
    assert_eq!(asset_row(&database, id).2, 3);
}

#[test]
fn missing_fingerprint_hit_rebinds_without_reading_content() {
    let (dir, database, library, db_dir) = setup("指纹重绑");
    let root = dir.path().join("photos");
    let a = root.join("a.jpg");
    write(&a, 1);
    let registered_mtime = fs::metadata(&a).unwrap().modified().unwrap();
    run(&database, &db_dir, &library.id);
    let id = asset_id_by_path(&database, &a.to_string_lossy());
    let registered_hash: i64 = database
        .0
        .query_row("SELECT xxhash FROM assets WHERE id = ?1", [id], |r| r.get(0))
        .unwrap();

    // 删原件 → 两轮 missing；出现同 (size, mtime) 的复制件（内容不同字节、
    // 同长度，mtime 对齐登记值）→ 指纹命中零整读重绑（AfterFrame 语义：
    // size+mtime 未变即视为同一文件；哈希沿用登记值不重算）
    fs::remove_file(&a).unwrap();
    run(&database, &db_dir, &library.id);
    run(&database, &db_dir, &library.id);
    assert_eq!(asset_row(&database, id).1, true);

    let copy = root.join("moved.jpg");
    // 内容不同（tag=9 vs 1，仅 [10..14] 差异）但长度相同（jpg() 固定 4096）
    fs::write(&copy, jpg(9)).unwrap();
    // 对齐 mtime 与 size：jpg() 固定 4096 字节 → size 相同；mtime 显式设置
    fs::File::options()
        .write(true)
        .open(&copy)
        .unwrap()
        .set_modified(registered_mtime)
        .unwrap();

    let report = run(&database, &db_dir, &library.id);
    assert_eq!(report.rebound, 1, "指纹命中 → 零哈希直接重绑");
    let (path, missing, _, _) = asset_row(&database, id);
    assert_eq!(path, copy.to_string_lossy());
    assert_eq!(missing, false);
    assert_eq!(count_assets(&database), 1, "不产生第二行（未走哈希查重登记）");
    let hash: i64 = database
        .0
        .query_row("SELECT xxhash FROM assets WHERE id = ?1", [id], |r| r.get(0))
        .unwrap();
    assert_eq!(hash, registered_hash, "内容指纹沿用登记值（零整读）");
}

// ---------------------------------------------------------------------------
// presence 精准事件（AfterFrame changed-media 借鉴）：missing 判回在线按
// asset_id 集合广播，前端瓦片级刷新缺失角标
// ---------------------------------------------------------------------------

#[test]
fn presence_changed_event_fires_on_restore_and_rebind() {
    let (dir, database, library, db_dir) = setup("presence");
    let root = dir.path().join("photos");
    let a = root.join("a.jpg");
    write(&a, 1);
    run(&database, &db_dir, &library.id);
    let id = asset_id_by_path(&database, &a.to_string_lossy());

    // 删原件 → 两轮 missing → 放回同内容：恢复轮发 presence 事件
    fs::remove_file(&a).unwrap();
    run(&database, &db_dir, &library.id);
    run(&database, &db_dir, &library.id);
    assert_eq!(asset_row(&database, id).1, true);

    let bus = events::EventBus::new();
    let mut rx = bus.subscribe();
    let library_row = database.photos_library_get(&library.id).unwrap().unwrap();
    write(&a, 1);
    let report = scan::scan_library_once(
        &database,
        &db_dir,
        &library_row,
        &relaxed(),
        None,
        Some(&bus),
    )
    .unwrap();
    assert_eq!(report.restored, 1);
    let mut presence_ids: Option<Vec<i64>> = None;
    while let Ok(event) = rx.try_recv() {
        if let events::AppEvent::AssetsPresenceChanged { library_id, asset_ids } = event {
            assert_eq!(library_id, library.id);
            presence_ids = Some(asset_ids);
        }
    }
    assert_eq!(presence_ids, Some(vec![id]), "恢复轮按集合发 presence 事件");

    // 改名场景（两轮 missing 后新名回到指纹/file-id 命中）：重绑同样发事件
    let bus2 = events::EventBus::new();
    let mut rx2 = bus2.subscribe();
    fs::rename(&a, root.join("renamed.jpg")).unwrap();
    // rename 保留 mtime 与 size → 指纹闸直接命中（新 inode 场景同款）
    run(&database, &db_dir, &library.id); // 原路径缺席记账
    let library_row = database.photos_library_get(&library.id).unwrap().unwrap();
    // 先把原路径推到 missing（缺席确认），新名下轮重绑
    database
        .0
        .execute("UPDATE assets SET missing = 1 WHERE id = ?1", [id])
        .unwrap();
    let report = scan::scan_library_once(
        &database,
        &db_dir,
        &library_row,
        &relaxed(),
        None,
        Some(&bus2),
    )
    .unwrap();
    assert_eq!(report.rebound, 1, "新名指纹/file-id 命中 → 重绑");
    let mut presence_ids: Option<Vec<i64>> = None;
    while let Ok(event) = rx2.try_recv() {
        if let events::AppEvent::AssetsPresenceChanged { asset_ids, .. } = event {
            presence_ids = Some(asset_ids);
        }
    }
    assert_eq!(presence_ids, Some(vec![id]), "重绑轮按集合发 presence 事件");
    assert_eq!(asset_row(&database, id).1, false);
}

// ---------------------------------------------------------------------------
// 登记单管道（§八-5）：并发扫描同一文件只登记一次
// ---------------------------------------------------------------------------

#[test]
fn registration_gate_serializes_concurrent_scans() {
    let (dir, _database, library, db_dir) = setup("单管道");
    let root = dir.path().join("photos");
    write(&root.join("a.jpg"), 1);
    let gate = std::sync::Arc::new(std::sync::Mutex::new(()));
    let options = relaxed();

    let mut handles = Vec::new();
    for _ in 0..2 {
        let db = open_db(&db_dir); // 每线程独立连接（与生产 worker/引擎同构）
        let library_row = db.photos_library_get(&library.id).unwrap().unwrap();
        let gate = std::sync::Arc::clone(&gate);
        let options = options.clone();
        let db_dir = db_dir.clone();
        handles.push(std::thread::spawn(move || {
            scan::scan_library_once(&db, &db_dir, &library_row, &options, Some(&gate), None)
                .unwrap()
        }));
    }
    let (r1, r2) = (
        handles.remove(0).join().unwrap(),
        handles.remove(0).join().unwrap(),
    );
    assert_eq!(
        r1.registered + r2.registered,
        1,
        "登记段互斥：后到者查重命中 → 只登记一次（r1={:?} r2={:?})",
        r1,
        r2
    );
    let check = open_db(&db_dir);
    assert_eq!(count_assets(&check), 1);
}

// ---------------------------------------------------------------------------
// 导入让路（§八-5/索引让路闸同骨架）：ipc 编排层
// ---------------------------------------------------------------------------

#[test]
fn scan_yields_while_import_running() {
    let holder = tempfile::tempdir().unwrap();
    let db_dir = holder.path().join("db");
    let src = holder.path().join("src"); // AppState 预注册源（空目录即可）
    let photos = holder.path().join("photos"); // 与数据库目录分离（root 互斥）
    for dir in [&db_dir, &src, &photos] {
        fs::create_dir_all(dir).unwrap();
    }
    let state = common::state_with_library(&db_dir, &src, Duration::from_millis(1));
    let database = open_db(&db_dir);
    database
        .photos_library_register("让路库", &photos.to_string_lossy(), &db_dir)
        .unwrap();
    write(&photos.join("a.jpg"), 1);

    // 导入在场 → 整轮让路（不扫不登记）
    state
        .import_running
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(ipc::photo_library::scan_all_libraries_once(&state).is_empty());
    assert_eq!(count_assets(&database), 0);

    // 导入结束 → 下轮恢复扫描。默认冷却窗 10s：刚写的文件本轮 cooled
    //（§八-2 语义——轮询周期 60s > 冷却窗，自然收敛登记；拾取链路由
    // cooldown 专项与 relaxed 扫描直测覆盖）。
    state
        .import_running
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let rounds = ipc::photo_library::scan_all_libraries_once(&state);
    assert_eq!(rounds.len(), 1, "放行后恢复扫描");
    assert_eq!(rounds[0].1.cooled, 1, "看到新文件（冷却中，下轮拾取）");
    assert_eq!(rounds[0].1.registered, 0);
}
