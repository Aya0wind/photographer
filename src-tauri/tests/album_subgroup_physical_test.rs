//! 子组物理化（0022）：subgroup 从纯 DB 标记改为真实子文件夹——
//! `photoRoot/{创建YYYY}/{创建MM}/{dir_name}/{子组}/`（存储布局唯一不平铺
//! 例外）。覆盖：公式表驱动（album_item_home_rel，含 sanitize）、导入带
//! 子组物理落位（无子组 = 平铺回归）、移组三向物理挪移（根↔组A↔组B：
//! 物理 + DB + assets.path + XMP 边车随行 + 失败行不落账 + 冲突后缀 +
//! 回收站拒绝）、claim 带子组落子文件夹。

mod common;

use common::library_fixture as setup;

pub use common::{
    platform,
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::path::{Path, PathBuf};

use common::{open_db, run_engine};
use db::AssetRow;
use events::AssetKind;
use ipc::album::fetch_album_item_move_subgroup;
use ipc::claim::fetch_album_claim_assets;

fn ins(db: &db::Db, path: &Path, captured: Option<&str>) -> i64 {
    let path_str = path.to_string_lossy().into_owned();
    db.insert_asset(&AssetRow {
        filename: path_str.rsplit(['\\', '/']).next().unwrap_or(&path_str).to_string(),
        path: path_str.clone(),
        size: 100,
        mtime: "2026-09-01T00:00:00.000Z".to_string(),
        xxhash: 42,
        kind: AssetKind::Photo,
        captured_at: captured.map(str::to_string),
        camera: Some("Sony A7M4".to_string()),
        source: "imported".to_string(),
        created_at: "2026-09-01T00:00:00.000Z".to_string(),
        origin: "imported".to_string(),
        width: None,
        height: None,
        iso: None,
        f_number: None,
        exposure_time: None,
        focal_length: None,
        lens: None,
        pair_asset_id: None,
        thumb_state: 0,
        orientation: None,
        flash: None,
        metering_mode: None,
        white_balance: None,
        exposure_program: None,
        software: None,
        artist: None,
        gps_lat: None,
        gps_lon: None,
        rating: 0,
        flagged: 0,
        color_label: None,
        rejected: 0,
    })
    .unwrap();
    db.asset_id_by_path(&path_str).unwrap().unwrap()
}


fn subgroup_of(db: &db::Db, album_id: i64, asset_id: i64) -> Option<String> {
    db.0.query_row(
        "SELECT subgroup FROM album_item WHERE album_id = ?1 AND asset_id = ?2",
        rusqlite::params![album_id, asset_id],
        |r| r.get(0),
    )
    .unwrap()
}

fn path_of(db: &db::Db, asset_id: i64) -> String {
    db.0.query_row("SELECT path FROM assets WHERE id = ?1", [asset_id], |r| {
        r.get(0)
    })
    .unwrap()
}

/// 路径串归一（`/` → `\`）：库内 path 混存两种分隔符形态（引擎渲染段为
/// `/`，claim/移组 join 产物为 `\`），字符串断言统一口径。
fn norm(p: &str) -> String {
    p.replace('/', "\\")
}

/// 固定创建时间的相册（2026-03-05）→ 主目录 {photo_root}\2026\03\{dir_name}
/// （home 段分隔符归一为平台原生，与 IPC 层 dst_dir 构造一致）。
fn fixed_album(db: &db::Db, name: &str) -> db::AlbumRow {
    let album = db.album_create(name).unwrap();
    db.0.execute(
        "UPDATE album SET created_at = '2026-03-05T08:00:00.000Z' WHERE id = ?1",
        [album.id],
    )
    .unwrap();
    album
}

/// 相册主目录的物理全路径（photo_root + home 段，平台分隔符）。
fn album_root(dir: &tempfile::TempDir, db: &db::Db, album: &db::AlbumRow) -> PathBuf {
    let home = db.album_home_rel(album.id).unwrap().unwrap();
    dir.path()
        .join("photos")
        .join(home.replace('/', std::path::MAIN_SEPARATOR_STR))
}

// ---------------------------------------------------------------------------
// 公式表驱动（album_item_home_rel：唯一不平铺例外 = 子组段）
// ---------------------------------------------------------------------------

#[test]
fn album_item_home_rel_formula_table_driven() {
    let tmp = tempfile::tempdir().unwrap();
    let db = open_db(tmp.path());
    let album = fixed_album(&db, "公式册");

    // None = 平铺现状（与 album_home_rel 同值）
    assert_eq!(
        db.album_item_home_rel(album.id, None).unwrap().as_deref(),
        Some("2026/03/公式册")
    );
    // Some = 追加净化子组段
    assert_eq!(
        db.album_item_home_rel(album.id, Some("成片")).unwrap().as_deref(),
        Some("2026/03/公式册/成片")
    );
    // sanitize 口径与 dir_name 一致：非法字符折叠 `-`、分隔符拍平（不支持嵌套）
    assert_eq!(
        db.album_item_home_rel(album.id, Some("A/B\\C:D"))
            .unwrap()
            .as_deref(),
        Some("2026/03/公式册/A-B-C-D")
    );
    // 保留设备名前缀 / 超长截断 / 空白兜底
    assert_eq!(
        db.album_item_home_rel(album.id, Some("CON")).unwrap().as_deref(),
        Some("2026/03/公式册/album-CON")
    );
    let long = "x".repeat(120);
    let seg = db
        .album_item_home_rel(album.id, Some(&long))
        .unwrap()
        .unwrap();
    assert_eq!(seg, format!("2026/03/公式册/{}", "x".repeat(80)));
    assert_eq!(
        db.album_item_home_rel(album.id, Some("   ")).unwrap().as_deref(),
        Some("2026/03/公式册/album")
    );
    // 相册不存在 → None
    assert_eq!(db.album_item_home_rel(999, Some("成片")).unwrap(), None);
}

// ---------------------------------------------------------------------------
// 导入：带子组物理落位 / 无子组平铺回归
// ---------------------------------------------------------------------------

#[test]
fn import_with_subgroup_lands_physical_subfolder_and_none_stays_flat() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let db = open_db(db_dir.path());
    let subgroup_album = db.album_create("子组导入册").unwrap();
    common::build_source(src.path());

    // 带子组：3 件全部落 {target}/{创建YYYY}/{创建MM}/{dir_name}/{净化子组}/
    let (_job, stats) = run_engine(src.path(), db_dir.path(), target.path(), |p| {
        p.album_id = Some(subgroup_album.id);
        p.album_subgroup = Some("机内直出".into());
    });
    assert_eq!(stats.done_files, 3);
    let home = db.album_home_rel(subgroup_album.id).unwrap().unwrap();
    let sub_dir = target.path().join(format!("{home}/机内直出"));
    let paths: Vec<String> = db
        .0
        .prepare("SELECT path FROM assets ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(paths.len(), 3);
    for path in &paths {
        let p = PathBuf::from(path);
        assert!(
            p.starts_with(&sub_dir),
            "应落子组文件夹 {sub_dir:?}，实得 {path}"
        );
        assert!(p.is_file(), "物理文件就位: {path}");
    }

    // 无子组 = 平铺现状回归：直接落相册主目录（无额外层）。
    // 独立库（同内容二次导入会被精确查重合法跳过）
    let src2 = tempfile::tempdir().unwrap();
    let db_dir2 = tempfile::tempdir().unwrap();
    let target2 = tempfile::tempdir().unwrap();
    common::build_source(src2.path());
    let flat_album = open_db(db_dir2.path()).album_create("平铺回归册").unwrap();
    let (_job2, stats2) = run_engine(src2.path(), db_dir2.path(), target2.path(), |p| {
        p.album_id = Some(flat_album.id);
    });
    assert_eq!(stats2.done_files, 3);
    let db2 = open_db(db_dir2.path());
    let home2 = db2.album_home_rel(flat_album.id).unwrap().unwrap();
    let flat_dir = target2.path().join(&home2);
    let paths2: Vec<String> = db2
        .0
        .prepare("SELECT path FROM assets ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(paths2.len(), 3);
    for path in &paths2 {
        let p = PathBuf::from(path);
        assert!(
            p.starts_with(&flat_dir),
            "无子组导入平铺在相册主目录 {flat_dir:?}，实得 {path}"
        );
        // 平铺 = 落位文件直接在主目录下（无任何额外子层）
        assert_eq!(
            p.parent().unwrap(),
            flat_dir.as_path(),
            "父目录就是相册主目录"
        );
    }
}

// ---------------------------------------------------------------------------
// 移组三向：物理挪移 + DB + assets.path + XMP 边车随行
// ---------------------------------------------------------------------------

#[test]
fn move_subgroup_three_way_physical_db_path_and_sidecar() {
    let (_dir, state, db) = setup();
    let album = fixed_album(&db, "移组册");
    let root = album_root(&_dir, &db, &album);
    std::fs::create_dir_all(&root).unwrap();

    // 相册根平铺两件；a 自带 XMP 边车
    let a = root.join("a.jpg");
    let b = root.join("b.jpg");
    std::fs::write(&a, b"jpeg-a").unwrap();
    std::fs::write(a.with_file_name("a.xmp"), b"<xmp/>").unwrap();
    std::fs::write(&b, b"jpeg-b").unwrap();
    let id_a = ins(&db, &a, Some("2026-03-05T10:00:00.000Z"));
    let id_b = ins(&db, &b, Some("2026-03-05T10:00:01.000Z"));
    db.album_add_assets(album.id, &[id_a, id_b], None).unwrap();

    // 根 → 组A：物理挪进子文件夹，XMP 随行，DB 双改
    let sub_a = root.join("成片");
    let moved = fetch_album_item_move_subgroup(&state, album.id, &[id_a], Some("成片")).unwrap();
    assert_eq!(moved, 1);
    assert!(!a.exists(), "源位已空");
    assert!(sub_a.join("a.jpg").is_file());
    assert!(sub_a.join("a.xmp").is_file(), "XMP 边车随行");
    assert_eq!(norm(&path_of(&db, id_a)), sub_a.join("a.jpg").to_string_lossy());
    assert_eq!(subgroup_of(&db, album.id, id_a).as_deref(), Some("成片"));
    // 未挪的行不动
    assert!(b.is_file());
    assert_eq!(subgroup_of(&db, album.id, id_b), None);

    // 组A → 组B（三向之二）
    let sub_b = root.join("精选");
    fetch_album_item_move_subgroup(&state, album.id, &[id_a], Some("精选")).unwrap();
    assert!(!sub_a.join("a.jpg").exists());
    assert!(sub_b.join("a.jpg").is_file());
    assert!(sub_b.join("a.xmp").is_file());
    assert_eq!(norm(&path_of(&db, id_a)), sub_b.join("a.jpg").to_string_lossy());
    assert_eq!(subgroup_of(&db, album.id, id_a).as_deref(), Some("精选"));

    // 组B → 根（三向之三：None = 挪回根，平铺现状）
    fetch_album_item_move_subgroup(&state, album.id, &[id_a], None).unwrap();
    assert!(root.join("a.jpg").is_file());
    assert!(root.join("a.xmp").is_file());
    assert!(!sub_b.join("a.jpg").exists());
    assert_eq!(norm(&path_of(&db, id_a)), root.join("a.jpg").to_string_lossy());
    assert_eq!(subgroup_of(&db, album.id, id_a), None);

    // 已在目标目录的重复挪移 = 幂等（只补账本，文件不动）
    let again = fetch_album_item_move_subgroup(&state, album.id, &[id_a], None).unwrap();
    assert_eq!(again, 1);
    assert!(root.join("a.jpg").is_file());

    // 不在册 id：0 行语义（不挪文件、不报错）
    let stranger_dir = _dir.path().join("outside");
    std::fs::create_dir_all(&stranger_dir).unwrap();
    let stranger = stranger_dir.join("s.jpg");
    std::fs::write(&stranger, b"jpeg-s").unwrap();
    let id_s = ins(&db, &stranger, None);
    let n = fetch_album_item_move_subgroup(&state, album.id, &[id_s], Some("成片")).unwrap();
    assert_eq!(n, 0, "不在册 id 不命中");
    assert!(stranger.is_file(), "不在册文件绝不动");
}

#[test]
fn move_subgroup_physical_failure_keeps_row_untouched() {
    let (_dir, state, db) = setup();
    let album = fixed_album(&db, "失败册");
    let root = album_root(&_dir, &db, &album);
    std::fs::create_dir_all(&root).unwrap();

    // 好文件 + 悬空路径（盘上无文件）
    let good = root.join("good.jpg");
    std::fs::write(&good, b"jpeg-good").unwrap();
    let id_good = ins(&db, &good, None);
    let ghost = root.join("ghost.jpg"); // 故意不写盘
    let id_ghost = ins(&db, &ghost, None);
    db.album_add_assets(album.id, &[id_good, id_ghost], None).unwrap();

    // 混合批：好行挪成，悬空行失败不落账（先物理后账本）
    let moved =
        fetch_album_item_move_subgroup(&state, album.id, &[id_good, id_ghost], Some("成片"))
            .unwrap();
    assert_eq!(moved, 1, "好行成功、坏行不计");
    assert_eq!(subgroup_of(&db, album.id, id_good).as_deref(), Some("成片"));
    assert!(root.join("成片").join("good.jpg").is_file());
    assert_eq!(subgroup_of(&db, album.id, id_ghost), None, "失败行 DB 不动");
    assert_eq!(
        path_of(&db, id_ghost),
        ghost.to_string_lossy(),
        "失败行 assets.path 不改写"
    );

    // 全部失败 → 命令报错（前端提示）
    let err = fetch_album_item_move_subgroup(&state, album.id, &[id_ghost], Some("成片"));
    assert!(err.is_err());
    assert_eq!(subgroup_of(&db, album.id, id_ghost), None);

    // 回收站资产拒绝挪移（行失败，账本不动）
    db.album_add_assets(album.id, &[id_good], None).unwrap(); // 已在册
    db.0.execute(
        "UPDATE assets SET in_trash = 1 WHERE id = ?1",
        [id_good],
    )
    .unwrap();
    let trashed = fetch_album_item_move_subgroup(&state, album.id, &[id_good], Some("原片"));
    assert!(trashed.is_err());
    assert_eq!(
        subgroup_of(&db, album.id, id_good).as_deref(),
        Some("成片"),
        "回收站行账本保持"
    );
}

#[test]
fn move_subgroup_conflict_suffix_and_same_batch_names() {
    let (_dir, state, db) = setup();
    let album = fixed_album(&db, "冲突册");
    let root = album_root(&_dir, &db, &album);
    let sub = root.join("成片");
    std::fs::create_dir_all(&sub).unwrap();

    // 目标子组文件夹已有同名文件（平铺时代的同名散照已先归位）→ " (2)"
    std::fs::write(sub.join("c.jpg"), b"existing").unwrap();
    let c = root.join("c.jpg");
    std::fs::write(&c, b"jpeg-c").unwrap();
    let id_c = ins(&db, &c, None);
    db.album_add_assets(album.id, &[id_c], None).unwrap();
    fetch_album_item_move_subgroup(&state, album.id, &[id_c], Some("成片")).unwrap();
    assert!(sub.join("c (2).jpg").is_file(), "冲突追加 (2) 后缀");
    assert_eq!(
        norm(&path_of(&db, id_c)),
        sub.join("c (2).jpg").to_string_lossy()
    );

    // 同批两个同名文件（不同源子组）挪入同一目标 → 第二个让名
    let sub_x = root.join("X组");
    let sub_y = root.join("Y组");
    std::fs::create_dir_all(&sub_x).unwrap();
    std::fs::create_dir_all(&sub_y).unwrap();
    std::fs::write(sub_x.join("d.jpg"), b"jpeg-d1").unwrap();
    std::fs::write(sub_y.join("d.jpg"), b"jpeg-d2").unwrap();
    let id_d1 = ins(&db, &sub_x.join("d.jpg"), None);
    let id_d2 = ins(&db, &sub_y.join("d.jpg"), None);
    db.album_add_assets(album.id, &[id_d1], Some("X组")).unwrap();
    db.album_add_assets(album.id, &[id_d2], Some("Y组")).unwrap();
    let moved = fetch_album_item_move_subgroup(&state, album.id, &[id_d1, id_d2], Some("Z组"))
        .unwrap();
    assert_eq!(moved, 2);
    let sub_z = root.join("Z组");
    assert!(sub_z.join("d.jpg").is_file());
    assert!(sub_z.join("d (2).jpg").is_file(), "同批同名第二个让名");
}

// ---------------------------------------------------------------------------
// claim 带子组：物理落子文件夹（含从相册根散照归子组）
// ---------------------------------------------------------------------------

#[test]
fn claim_with_subgroup_moves_root_scatter_into_subfolder() {
    let (_dir, state, db) = setup();
    let album = fixed_album(&db, "交付册");
    let root = album_root(&_dir, &db, &album);
    std::fs::create_dir_all(&root).unwrap();

    // 相册根散照（DB subgroup NULL）带子组 claim → 物理挪进子文件夹
    let photo = root.join("DSC_0001.jpg");
    std::fs::write(&photo, b"jpeg").unwrap();
    let id = ins(&db, &photo, Some("2026-06-01T10:00:00.000Z"));
    db.album_add_assets(album.id, &[id], None).unwrap();

    let result = fetch_album_claim_assets(&state, album.id, &[id], Some("成片")).unwrap();
    assert_eq!(result.moved, 1, "{result:?}");
    assert!(!photo.exists());
    let sub = root.join("成片").join("DSC_0001.jpg");
    assert!(sub.is_file());
    assert_eq!(norm(&path_of(&db, id)), sub.to_string_lossy());
    assert_eq!(subgroup_of(&db, album.id, id).as_deref(), Some("成片"));

    // 幂等重试：已在目标子组目录（前缀含子组段）→ skipped
    let again = fetch_album_claim_assets(&state, album.id, &[id], Some("成片")).unwrap();
    assert_eq!(again.skipped, 1);
}
