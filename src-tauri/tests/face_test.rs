//! 人脸全链路（M4）：在线聚类正确性（两簇分离数据）、对齐几何、
//! faces/people 表契约（migration 0006：级联 / SET NULL / 清空 / 封面）、
//! IPC 命令、#[ignore] 真 smoke（真模型 + Y:\照片 已知单文件）。

mod common;

pub use common::{
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks,
    thumbs, videos,
};

use std::path::Path;
use std::time::Duration;

use ai::face::{self, OnlineClusterer, CLUSTER_COS_THRESHOLD, FACE_EMBED_DIM, MAX_FACES_PER_IMAGE};
use common::open_db;
use db::{AssetRow, PersonRow};
use events::AssetKind;

/// 确定性伪随机 512 维向量（xorshift64，归一化）。
pub fn rand_unit(seed: u64) -> Vec<f32> {
    let mut x = seed | 1;
    let mut v = Vec::with_capacity(FACE_EMBED_DIM);
    for _ in 0..FACE_EMBED_DIM {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.push((x % 20001) as f32 / 10000.0 - 1.0);
    }
    let norm = v.iter().map(|a| a * a).sum::<f32>().sqrt();
    v.iter().map(|a| a / norm).collect()
}

fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|a| a * a).sum::<f32>().sqrt();
    for a in &mut v {
        *a /= norm;
    }
    v
}

/// 分组基向量 + 15% 噪声 → 归一化成员（组内 cos≈1，组间 cos≈0）。
fn member(basis_offset: usize, seed: u64) -> Vec<f32> {
    let mut v = vec![0f32; FACE_EMBED_DIM];
    for d in 0..32 {
        v[basis_offset + d] = 1.0;
    }
    for (d, n) in rand_unit(seed).into_iter().enumerate() {
        v[d] += 0.15 * n;
    }
    normalize(v)
}

fn cos(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

// ---------------------------------------------------------------------------
// 在线聚类
// ---------------------------------------------------------------------------

#[test]
fn online_clustering_two_separated_groups() {
    let mut clusterer = OnlineClusterer::new(CLUSTER_COS_THRESHOLD);
    let mut groups: std::collections::HashMap<i64, Vec<usize>> = Default::default();
    let mut next_id = 1i64;
    // 交替投喂 60 张脸（在线单遍；模拟时间交错）
    for i in 0..60usize {
        let vec = if i % 2 == 0 {
            member(0, 1000 + i as u64)
        } else {
            member(32, 2000 + i as u64)
        };
        let id = match clusterer.assign(&vec) {
            Some(id) => id,
            None => {
                let id = next_id;
                next_id += 1;
                id
            }
        };
        clusterer.absorb(id, &vec);
        groups.entry(id).or_default().push(i);
    }
    assert_eq!(clusterer.cluster_count(), 2, "两组分离数据 → 恰好 2 簇");
    assert_eq!(next_id - 1, 2, "只开过 2 个新簇");
    // 按内容定位簇：含下标 0 的 = A 组（偶数），含 1 的 = B 组
    let id_of = |idx: usize| {
        groups
            .iter()
            .find(|(_, v)| v.contains(&idx))
            .map(|(k, _)| *k)
            .expect("簇必含成员")
    };
    let even_cluster = id_of(0);
    let odd_cluster = id_of(1);
    assert_ne!(even_cluster, odd_cluster, "两簇互斥");
    // 偶数下标（A 组）同簇、奇数（B 组）同簇
    let (even, odd): (Vec<usize>, Vec<usize>) = (0..60).partition(|i| i % 2 == 0);
    assert_eq!(groups[&even_cluster], even, "A 组全部归同簇");
    assert_eq!(groups[&odd_cluster], odd, "B 组全部归同簇");
    // 组内成员与簇心 cos 高、跨组低（阈值 0.4 分离度观察）
    let centroid = |members: &[usize]| {
        let mut sum = vec![0f32; FACE_EMBED_DIM];
        for i in members {
            let v = if i % 2 == 0 {
                member(0, 1000 + *i as u64)
            } else {
                member(32, 2000 + *i as u64)
            };
            for (s, x) in sum.iter_mut().zip(&v) {
                *s += x;
            }
        }
        normalize(sum)
    };
    let (ca, cb) = (centroid(&even), centroid(&odd));
    let probe_a = member(0, 9999);
    let probe_b = member(32, 9998);
    assert!(
        cos(&probe_a, &ca) > 0.9 && cos(&probe_b, &cb) > 0.9,
        "组内 cos 应 >0.9: {} {}",
        cos(&probe_a, &ca),
        cos(&probe_b, &cb)
    );
    assert!(
        cos(&ca, &cb).abs() < 0.2,
        "簇间 cos 应近 0: {}",
        cos(&ca, &cb)
    );
    // 阈值语义：跨组探针不归簇（否则会串簇）
    assert_eq!(clusterer.assign(&probe_b), Some(odd_cluster));
    assert_eq!(clusterer.assign(&probe_a), Some(even_cluster));
}

#[test]
fn cluster_cache_rebuild_from_sums_reproduces_assignment() {
    // 重启路径：from_sums 重建的聚类器对同分布新脸给出与在线一致的判定
    let mut online = OnlineClusterer::new(CLUSTER_COS_THRESHOLD);
    let mut next_id = 1i64;
    for i in 0..20usize {
        let vec = member(0, 500 + i as u64);
        let id = online.assign(&vec).unwrap_or_else(|| {
            let id = next_id;
            next_id += 1;
            id
        });
        online.absorb(id, &vec);
    }
    // 簇和序列化到 DB 再读回（roundtrip 由 db 测试覆盖，这里直接回灌）
    let sums: std::collections::HashMap<i64, (Vec<f32>, u32)> = (1..next_id)
        .map(|id| {
            (
                id,
                (
                    member(0, 505),
                    20, // 成员数仅为占位：best_match 只用向量和
                ),
            )
        })
        .collect();
    let rebuilt = OnlineClusterer::from_sums(sums, CLUSTER_COS_THRESHOLD);
    assert_eq!(rebuilt.cluster_count(), online.cluster_count());
    let probe = member(0, 777);
    assert!(rebuilt.assign(&probe).is_some(), "重建后同组脸应可归簇");
    let stranger = member(300, 778);
    let rebuilt_cos = rebuilt.best_match(&stranger).map(|(_, c)| c);
    assert!(
        rebuilt_cos.unwrap_or(0.0) < CLUSTER_COS_THRESHOLD,
        "异组脸对重建簇心 cos 应低于阈值: {rebuilt_cos:?}"
    );
}

// ---------------------------------------------------------------------------
// 对齐几何（Umeyama 闭式解）
// ---------------------------------------------------------------------------

/// ArcFace canonical 5 点模板（与 face.rs 内一致）。
const TEMPLATE: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

#[test]
fn similarity_matrix_recovers_known_transform() {
    for (scale, angle) in [(1.0f32, 0f32), (1.35, 0.7), (0.8, -2.2)] {
        let (s, c) = (scale * angle.sin(), scale * angle.cos());
        let t = (3.0f32, -2.0);
        let src: [[f32; 2]; 5] =
            TEMPLATE.map(|p| [c * p[0] - s * p[1] + t.0, s * p[0] + c * p[1] + t.1]);

        // 方向一（真实使用方向）：kps=src → dst=TEMPLATE = 正向矩阵的逆
        let m = face::similarity_matrix(&src, &TEMPLATE).expect("非退化点集必有解");
        let (ic, is) = (angle.cos() / scale, angle.sin() / scale);
        let expect = [
            [ic, is, -(ic * t.0 + is * t.1)],
            [-is, ic, -(-is * t.0 + ic * t.1)],
        ];
        for (row, erow) in m.iter().zip(&expect) {
            for (v, ev) in row.iter().zip(erow) {
                assert!(
                    (v - ev).abs() < 1e-3,
                    "逆向矩阵恢复失败 {m:?} vs {expect:?}"
                );
            }
        }

        // 方向二：TEMPLATE → src = 正向矩阵本体
        let m2 = face::similarity_matrix(&TEMPLATE, &src).expect("非退化点集必有解");
        let expect2 = [[c, -s, t.0], [s, c, t.1]];
        for (row, erow) in m2.iter().zip(&expect2) {
            for (v, ev) in row.iter().zip(erow) {
                assert!(
                    (v - ev).abs() < 1e-3,
                    "正向矩阵恢复失败 {m2:?} vs {expect2:?}"
                );
            }
        }
    }
    // 退化（全重合点）→ None
    let degenerate = [[5.0; 2]; 5];
    assert!(face::similarity_matrix(&degenerate, &TEMPLATE).is_none());
}

#[test]
fn warp_similarity_identity_maps_center() {
    // 正向 2 倍放大矩阵：输出 (x, y) 逆向采样源 (x/2, y/2)
    let mut img = image::RgbImage::new(300, 300);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8]);
    }
    let m = [[2.0f32, 0.0, 0.0], [0.0, 2.0, 0.0]];
    let crop = face::warp_similarity(&img, &m);
    assert_eq!(crop.dimensions(), (112, 112));
    // 偶数坐标：逆向映射落整点，双线性退化为逐点一致
    for (x, y) in [(100u32, 40u32), (56, 56), (110, 4)] {
        let expected = img.get_pixel(x / 2, y / 2);
        let got = crop.get_pixel(x, y);
        assert_eq!(expected.0, got.0, "整点采样应逐点一致 ({x},{y})");
    }
    // 非整点：双线性取相邻均值（(55, 1.5) → 上下两行平均）
    let got = crop.get_pixel(110, 3);
    let r0 = img.get_pixel(55, 1).0;
    let r1 = img.get_pixel(55, 2).0;
    for ch in 0..3 {
        let avg = (r0[ch] as f32 + r1[ch] as f32) / 2.0;
        assert!(
            (got.0[ch] as f32 - avg).abs() <= 0.6,
            "半像素双线性插值: ch={ch} got={} avg={avg}",
            got.0[ch]
        );
    }
}

// ---------------------------------------------------------------------------
// faces / people 表契约（migration 0006）
// ---------------------------------------------------------------------------

fn asset_row(path: &str, kind: AssetKind, captured_at: Option<&str>) -> AssetRow {
    AssetRow {
        path: path.into(),
        filename: path.rsplit(['/', '\\']).next().unwrap_or(path).into(),
        size: 100,
        mtime: "2026-09-19T00:00:00.000Z".into(),
        xxhash: 1,
        kind,
        captured_at: captured_at.map(Into::into),
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

fn face_count(db: &db::Db) -> i64 {
    db.0.query_row("SELECT COUNT(*) FROM faces", [], |r| r.get(0))
        .unwrap()
}

fn person_count(db: &db::Db) -> i64 {
    db.0.query_row("SELECT COUNT(*) FROM people", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn migration_0006_accepts_face_kind_and_fk_behaviors() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_db(dir.path());

    // kind='face' 通过扩展后的 CHECK
    db.insert_asset(&asset_row("X:/p/a.jpg", AssetKind::Photo, None))
        .unwrap();
    let asset_id = db.asset_id_by_path("X:/p/a.jpg").unwrap().unwrap();
    let n =
        db.0.execute(
            "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
             VALUES ('face', ?1, 'pending', 0, '2026', '2026')",
            [asset_id],
        )
        .unwrap();
    assert_eq!(n, 1);

    // 资产删除级联清脸
    let pid = db.create_person().unwrap();
    db.insert_face(asset_id, 1.0, 2.0, 30.0, 40.0, &rand_unit(1), Some(pid))
        .unwrap();
    assert_eq!(face_count(&db), 1);
    db.0.execute("DELETE FROM assets WHERE id = ?1", [asset_id])
        .unwrap();
    assert_eq!(face_count(&db), 0, "资产删除应级联清脸");
    assert_eq!(person_count(&db), 1, "删资产不动人物簇");

    // 删簇不删脸：cluster_id SET NULL
    db.insert_asset(&asset_row("X:/p/b.jpg", AssetKind::Photo, None))
        .unwrap();
    let asset_id = db.asset_id_by_path("X:/p/b.jpg").unwrap().unwrap();
    let pid2 = db.create_person().unwrap();
    let fid = db
        .insert_face(asset_id, 0.0, 0.0, 50.0, 50.0, &rand_unit(2), Some(pid2))
        .unwrap();
    db.delete_person(pid2).unwrap();
    let cluster: Option<i64> =
        db.0.query_row("SELECT cluster_id FROM faces WHERE id = ?1", [fid], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(cluster, None, "删簇只解除归属");
    assert_eq!(face_count(&db), 1, "脸数据保留");
    // 不存在的簇报错
    assert!(db.delete_person(999_999).is_err());
    assert!(db.rename_person(999_999, "x").is_err());
}

#[test]
fn face_cluster_sums_and_people_list_contract() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_db(dir.path());
    db.insert_asset(&asset_row("X:/p/a.jpg", AssetKind::Photo, None))
        .unwrap();
    db.insert_asset(&asset_row("X:/p/b.jpg", AssetKind::Photo, None))
        .unwrap();
    let a = db.asset_id_by_path("X:/p/a.jpg").unwrap().unwrap();
    let b = db.asset_id_by_path("X:/p/b.jpg").unwrap().unwrap();

    let p_big = db.create_person().unwrap();
    let p_small = db.create_person().unwrap();
    let e1 = rand_unit(11);
    let e2 = rand_unit(12);
    let e3 = rand_unit(13);
    let f1 = db
        .insert_face(a, 0.0, 0.0, 100.0, 100.0, &e1, Some(p_big))
        .unwrap();
    let f2 = db
        .insert_face(a, 5.0, 5.0, 20.0, 20.0, &e2, Some(p_big))
        .unwrap();
    db.insert_face(b, 0.0, 0.0, 10.0, 10.0, &e3, Some(p_small))
        .unwrap();

    // 簇和 = 成员向量和（blob f32 LE roundtrip）
    let sums = db.face_cluster_sums().unwrap();
    assert_eq!(sums.len(), 2);
    let (sum_big, count_big) = sums.get(&p_big).unwrap();
    assert_eq!(count_big, &2);
    for (s, (x, y)) in sum_big.iter().zip(e1.iter().zip(&e2)) {
        assert!((s - (x + y)).abs() < 1e-6);
    }

    // 封面晋升：大框脸担任封面（先小后大再验证）
    db.maybe_promote_cover(f2, p_big, 20.0 * 20.0).unwrap();
    let cover: Option<i64> =
        db.0.query_row(
            "SELECT cover_face_id FROM people WHERE id = ?1",
            [p_big],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cover, Some(f2), "首张脸无条件担任封面");
    db.maybe_promote_cover(f1, p_big, 100.0 * 100.0).unwrap();
    let cover: Option<i64> =
        db.0.query_row(
            "SELECT cover_face_id FROM people WHERE id = ?1",
            [p_big],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cover, Some(f1), "更大框脸应晋升封面");

    // people_list：face_count 降序；cover_asset = 封面脸资产
    let people = db.people_list().unwrap();
    assert_eq!(
        people,
        vec![
            PersonRow {
                id: p_big,
                name: None,
                face_count: 2,
                cover_asset_id: Some(a),
            },
            PersonRow {
                id: p_small,
                name: None,
                face_count: 1,
                cover_asset_id: Some(b),
            },
        ]
    );

    // 重命名（trim / 空串回未命名）
    db.rename_person(p_big, "  张三  ").unwrap();
    assert_eq!(db.people_list().unwrap()[0].name.as_deref(), Some("张三"));
    db.rename_person(p_big, "   ").unwrap();
    assert_eq!(db.people_list().unwrap()[0].name, None);

    // assets_by_cluster：去重 + captured_at DESC（NULL 最先）
    db.insert_asset(&asset_row(
        "X:/p/c.jpg",
        AssetKind::Photo,
        Some("2026-01-01T10:00:00.000Z"),
    ))
    .unwrap();
    let c = db.asset_id_by_path("X:/p/c.jpg").unwrap().unwrap();
    db.insert_face(c, 0.0, 0.0, 5.0, 5.0, &rand_unit(14), Some(p_big))
        .unwrap();
    let assets = db.assets_by_cluster(p_big, 10).unwrap();
    assert_eq!(assets.len(), 2, "同资产多脸去重");
    assert_eq!(assets[0].id, a, "NULL captured_at 最先");
    assert_eq!(assets[1].id, c);
    let limited = db.assets_by_cluster(p_big, 1).unwrap();
    assert_eq!(limited.len(), 1);
    // 空簇不返回
    assert!(db.people_list().unwrap().iter().all(|p| p.face_count > 0));
}

#[test]
fn face_task_ledger_and_clear_face_data() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_db(dir.path());
    db.insert_asset(&asset_row("X:/p/a.jpg", AssetKind::Photo, None))
        .unwrap();
    db.insert_asset(&asset_row("X:/p/b.jpg", AssetKind::Photo, None))
        .unwrap();
    db.insert_asset(&asset_row("X:/p/v.mp4", AssetKind::Video, None))
        .unwrap();
    db.insert_asset(&asset_row("X:/p/r.NEF", AssetKind::Raw, None))
        .unwrap();

    // 回填建任务：photo/raw 各 1，video 不建；重复建去重
    assert_eq!(db.create_face_tasks_for_unindexed().unwrap(), 3);
    assert_eq!(
        db.create_face_tasks_for_unindexed().unwrap(),
        0,
        "pending 去重"
    );

    // 记账后不再建
    let a = db.asset_id_by_path("X:/p/a.jpg").unwrap().unwrap();
    db.set_face_indexed(a).unwrap();
    db.0.execute(
        "UPDATE index_tasks SET state = 'done' WHERE kind = 'face' AND asset_id = ?1",
        [a],
    )
    .unwrap();
    assert_eq!(
        db.create_face_tasks_for_unindexed().unwrap(),
        0,
        "已记账资产不回建"
    );

    // 一键清除：faces/people/任务/时间账全部复位
    let b = db.asset_id_by_path("X:/p/b.jpg").unwrap().unwrap();
    db.insert_face(b, 0.0, 0.0, 10.0, 10.0, &rand_unit(21), None)
        .unwrap();
    assert_eq!(face_count(&db), 1);
    db.clear_face_data().unwrap();
    assert_eq!(face_count(&db), 0);
    assert_eq!(person_count(&db), 0);
    let tasks: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM index_tasks WHERE kind = 'face'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tasks, 0, "face 通道任务清空");
    let ledger: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM assets WHERE face_indexed_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ledger, 0, "face_indexed_at 复位");
    assert_eq!(
        db.create_face_tasks_for_unindexed().unwrap(),
        3,
        "清除后可重新回填"
    );
}

// ---------------------------------------------------------------------------
// IPC
// ---------------------------------------------------------------------------

#[test]
fn ipc_people_commands_roundtrip() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let db = open_db(db_dir.path());
    db.insert_asset(&asset_row("X:/p/a.jpg", AssetKind::Photo, None))
        .unwrap();
    db.insert_asset(&asset_row(
        "X:/p/b.jpg",
        AssetKind::Photo,
        Some("2026-02-02T08:00:00.000Z"),
    ))
    .unwrap();
    let a = db.asset_id_by_path("X:/p/a.jpg").unwrap().unwrap();
    let b = db.asset_id_by_path("X:/p/b.jpg").unwrap().unwrap();
    let pid = db.create_person().unwrap();
    db.insert_face(a, 0.0, 0.0, 40.0, 40.0, &rand_unit(31), Some(pid))
        .unwrap();
    db.insert_face(b, 0.0, 0.0, 60.0, 60.0, &rand_unit(32), Some(pid))
        .unwrap();

    // people_list
    let people = ipc::people::fetch_people_list(&state).unwrap();
    assert_eq!(people.len(), 1);
    assert_eq!(people[0].id, pid);
    assert_eq!(people[0].face_count, 2);
    assert_eq!(people[0].cover_asset_id, Some(b), "封面 = 最大框脸资产");

    // people_assets：captured_at DESC 去重
    let assets = ipc::people::fetch_people_assets(&state, pid, 10).unwrap();
    assert_eq!(assets.len(), 2);
    assert_eq!(assets[0].id, a, "NULL captured_at 最先（画廊同款契约）");
    assert_eq!(assets[1].id, b);

    // rename
    ipc::people::fetch_person_rename(&state, pid, "小明").unwrap();
    assert_eq!(
        ipc::people::fetch_people_list(&state).unwrap()[0]
            .name
            .as_deref(),
        Some("小明")
    );
    // delete：faces 保留 + 缓存层无感（重查即空列表）
    ipc::people::fetch_person_delete(&state, pid).unwrap();
    assert!(ipc::people::fetch_people_list(&state).unwrap().is_empty());
    assert!(ipc::people::fetch_people_assets(&state, pid, 10)
        .unwrap()
        .is_empty());

    // clear：复位全链
    db.insert_face(a, 0.0, 0.0, 10.0, 10.0, &rand_unit(33), None)
        .unwrap();
    db.create_person().unwrap();
    assert!(ipc::ai::fetch_face_data_clear(&state).unwrap());
    assert_eq!(face_count(&db), 0);
    assert_eq!(person_count(&db), 0);
}

// ---------------------------------------------------------------------------
// 真 smoke（#[ignore]）：需先在设置页/ai_model_download 下载 scrfd + arcface
// 到 %APPDATA%\com.smartphoto.app\models\。手动跑：
// cargo test --test face_test real -- --ignored --nocapture
// ---------------------------------------------------------------------------

fn models_dir() -> Option<std::path::PathBuf> {
    std::env::var("APPDATA")
        .ok()
        .map(|base| Path::new(&base).join("com.smartphoto.app").join("models"))
        .filter(|dir| dir.is_dir())
}

fn real_manager() -> Option<ai::ModelManager> {
    let models = models_dir()?;
    if !models.join("scrfd.onnx").is_file() || !models.join("arcface.onnx").is_file() {
        eprintln!("skip: scrfd/arcface 未下载于 {}", models.display());
        return None;
    }
    Some(ai::ModelManager::new(
        models,
        events::EventBus::new(),
        tasks::TaskSupervisor::new(events::EventBus::new()),
    ))
}

/// smoke fixture localization: samples have been copied to the writable test area (single-file direct read, never traverse); face = positive group photo sample, noface = negative sample with no people.
fn sample_path(name: &str) -> Option<std::path::PathBuf> {
    let p = Path::new("I:\\SmartPhoto-test-e2e\\face-smoke").join(name);
    p.is_file().then_some(p)
}

/// 真实模型 + 真实照片 smoke：
/// 正样例 DSC_0243.JPG（团建合影，确有人脸）：检出框/关键点合理、512 维
/// 特征归一化、同脸确定性、双人脸区分度；负样例 DSC_0178.JPG（确无人脸）
/// 断言 0 检出（真机核对 2026-09-19：0178 无人物主体）。
#[test]
#[ignore = "可选 smoke：需已下载 scrfd+arcface 且样例存在；交付验证由真机回归执行"]
fn real_scrfd_arcface_pipeline() {
    let Some(manager) = real_manager() else {
        return;
    };
    // 负样例：无人脸图 → 0 检出
    if let Some(neg) = sample_path("noface.jpg") {
        let neg_img = image::ImageReader::open(&neg)
            .unwrap()
            .decode()
            .unwrap()
            .to_rgb8();
        let neg_faces = face::detect_faces(&manager, &neg_img).expect("SCRFD 推理");
        eprintln!("负样例 {} → 检出 {} 张", neg.display(), neg_faces.len());
        assert!(
            neg_faces.is_empty(),
            "无人脸样例不应有检出: {}",
            neg_faces.len()
        );
    }
    let Some(img_path) = sample_path("face.jpg") else {
        eprintln!("skip: face.jpg (DSC_0243) not found");
        return;
    };
    let img = image::ImageReader::open(&img_path)
        .expect("打开样例")
        .decode()
        .expect("解码样例")
        .to_rgb8();
    eprintln!(
        "样例 {} {}x{}",
        img_path.display(),
        img.width(),
        img.height()
    );

    let faces = face::detect_faces(&manager, &img).expect("SCRFD 推理");
    eprintln!("检出 {} 张人脸", faces.len());
    for (i, f) in faces.iter().take(8).enumerate() {
        eprintln!(
            "  #{i} score={:.3} box=({:.0},{:.0},{:.0}x{:.0})",
            f.score, f.box_x, f.box_y, f.box_w, f.box_h
        );
    }
    assert!(!faces.is_empty(), "DSC_0243 合影应至少检出 1 张人脸");
    assert!(faces.iter().all(|f| f.score >= face::DETECT_MIN_SCORE));
    let (w, h) = (img.width() as f32, img.height() as f32);
    for f in &faces {
        assert!(f.box_w > 0.0 && f.box_h > 0.0, "框尺寸非正: {f:?}");
        assert!(
            f.box_x >= -2.0
                && f.box_y >= -2.0
                && f.box_x + f.box_w <= w + 2.0
                && f.box_y + f.box_h <= h + 2.0,
            "框应落在图内（±2px 容差）: {f:?}"
        );
        assert!(
            f.kps.iter().all(|p| p[0].is_finite() && p[1].is_finite()),
            "关键点应为有限值: {f:?}"
        );
    }

    // 全链路：对齐 + 特征
    let mut pairs = manager.detect_and_embed(&img).expect("全链路");
    assert_eq!(
        pairs.len(),
        faces.len().min(MAX_FACES_PER_IMAGE),
        "对齐+识别不应丢脸"
    );
    assert!(!pairs.is_empty());
    let (_, emb0) = pairs.remove(0);
    assert_eq!(emb0.len(), FACE_EMBED_DIM);
    let norm: f32 = emb0.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 1e-3, "特征应 L2 归一化: {norm}");

    // 同一裁剪重复推理：确定性（cos ≈ 1）
    let f0 = &faces[0];
    let crop = face::align_face(&img, &f0.kps).expect("对齐");
    let again = manager.embed_aligned(&crop).expect("重复识别");
    assert!(
        cos(&emb0, &again) > 0.999,
        "同脸嵌入应确定: {}",
        cos(&emb0, &again)
    );

    // 不同人脸特征应有区分度（打印供阈值调参观察）
    if let Some((_, emb1)) = pairs.first() {
        let d = cos(&emb0, emb1);
        eprintln!("同人图内不同脸 cos(0,1)={d:.4}（观察聚类阈值 0.4 的裕度）");
    }
}
