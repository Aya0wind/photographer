//! 去重命令（M7 F8 两级去重）：exact = (size, xxhash) 组；similar = pHash
//! 汉明 ≤6 近重复组（多探针 4×16-bit 桶候选 + 精确过滤 + 并查集聚合，
//! 绝不 O(n²) 两两比）。RAW+JPG 孪生边排除（同拍摄不算重复）；连拍组内
//! **不排除**（连拍正是挑片场景）。

use serde::{Deserialize, Serialize};
use tauri::State;

use super::{run_blocking, SharedState};

/// 近重复判定汉明阈值（区别于连拍分组的 ≤10）。
pub const SIMILAR_HAMMING_MAX: u32 = 6;
/// 单桶成员上限：超过视为病态（同哈希海量堆），跳桶防候选对爆炸。
const BUCKET_MEMBER_CAP: usize = 512;

/// 重复组载荷：kind + 组内资产（created_at 升序，burst 字段同画廊契约）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroupDto {
    /// "exact" | "similar"
    pub kind: String,
    pub assets: Vec<super::assets::AssetDto>,
}

/// exact 组核：(size, xxhash) 分组 >1 → 每组取行（组内 created_at 升序）。
/// 组序 = 组大小降序（SQL 键序）；`after` 为上一页末组序号（0 起，省略/0
/// = 第一页），`limit` 限组数（上限 100）。
pub fn fetch_duplicates_exact(
    state: &super::AppState,
    after: usize,
    limit: usize,
) -> Result<Vec<DuplicateGroupDto>, String> {
    let db = super::active_library_db(state)?;
    let keys = db.exact_duplicate_keys().map_err(|e| e.to_string())?;
    let mut groups = Vec::new();
    for (size, xxhash) in keys.into_iter().skip(after).take(limit.min(100)) {
        let ids: Vec<i64> =
            db.0.prepare(
                "SELECT id FROM assets WHERE size = ?1 AND xxhash = ?2 \
                 ORDER BY created_at ASC, id ASC",
            )
            .and_then(|mut s| {
                s.query_map(rusqlite::params![size, xxhash as i64], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<i64>>>()
            })
            .map_err(|e| e.to_string())?;
        let rows = db.assets_by_ids_ordered(&ids).map_err(|e| e.to_string())?;
        let mut assets: Vec<_> = rows
            .into_iter()
            .map(super::assets::page_row_to_dto)
            .collect();
        super::assets::attach_burst_counts_pub(&db, &mut assets);
        groups.push(DuplicateGroupDto {
            kind: "exact".into(),
            assets,
        });
    }
    Ok(groups)
}

/// similar 组核：懒校验桶表 → 多探针候选 → 汉明 ≤6 过滤 → 并查集 → 组。
fn fetch_duplicates_similar_core(
    db: &crate::db::Db,
    after: usize,
    limit: usize,
) -> Result<Vec<Vec<i64>>, String> {
    db.ensure_similar_buckets()?;
    // (asset_id → phash, pair) 全量（20 万 × 16B ≈ 数 MB）
    let rows = db.similar_scan_rows().map_err(|e| e.to_string())?;
    let mut phash_of = std::collections::HashMap::with_capacity(rows.len());
    let mut pair_of = std::collections::HashMap::with_capacity(rows.len());
    for (id, phash, pair) in &rows {
        phash_of.insert(*id, *phash as u64);
        pair_of.insert(*id, *pair);
    }
    // 并查集
    let mut uf = UnionFind::new(rows.iter().map(|r| r.0).collect());
    for (_key, members) in db.similar_bucket_members().map_err(|e| e.to_string())? {
        if members.len() > BUCKET_MEMBER_CAP {
            eprintln!(
                "[duplicates] 桶 {:?} 成员 {} 超上限，跳过（病态哈希堆）",
                _key,
                members.len()
            );
            continue;
        }
        for i in 0..members.len() {
            for j in (i + 1)..members.len() {
                let (a, b) = (members[i], members[j]);
                // 孪生边排除（双向指向都算 RAW+JPG 同拍摄）
                if pair_of.get(&a) == Some(&Some(b)) || pair_of.get(&b) == Some(&Some(a)) {
                    continue;
                }
                let (Some(ha), Some(hb)) = (phash_of.get(&a), phash_of.get(&b)) else {
                    continue;
                };
                if crate::metadata::phash::hamming(*ha, *hb) <= SIMILAR_HAMMING_MAX {
                    uf.union(a, b);
                }
            }
        }
    }
    // 组装配：成员 ≥2，组大小降序稳定
    let mut groups: Vec<Vec<i64>> = uf.groups();
    groups.retain(|g| g.len() >= 2);
    for group in &mut groups {
        group.sort_unstable();
    }
    groups.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));
    Ok(groups
        .into_iter()
        .skip(after)
        .take(limit.min(100))
        .collect())
}

/// similar 组核（IPC 面）：组 → DTO（组内 created_at 升序）。
pub fn fetch_duplicates_similar(
    state: &super::AppState,
    after: usize,
    limit: usize,
) -> Result<Vec<DuplicateGroupDto>, String> {
    let db = super::active_library_db(state)?;
    let groups = fetch_duplicates_similar_core(&db, after, limit)?;
    let mut out = Vec::with_capacity(groups.len());
    for ids in groups {
        let rows = db.assets_by_ids_ordered(&ids).map_err(|e| e.to_string())?;
        let mut assets: Vec<_> = rows
            .into_iter()
            .map(super::assets::page_row_to_dto)
            .collect();
        super::assets::attach_burst_counts_pub(&db, &mut assets);
        out.push(DuplicateGroupDto {
            kind: "similar".into(),
            assets,
        });
    }
    Ok(out)
}

/// duplicates_list 核。
pub fn fetch_duplicates_list(
    state: &super::AppState,
    kind: &str,
    after: usize,
    limit: usize,
) -> Result<Vec<DuplicateGroupDto>, String> {
    match kind {
        "exact" => fetch_duplicates_exact(state, after, limit),
        "similar" => fetch_duplicates_similar(state, after, limit),
        other => Err(format!("未知去重档位: {other}（可选 exact | similar）")),
    }
}

/// 删除资产核（duplicate_delete）：删库行（FK 级联清 index_tasks/faces/
/// view_history/similar_bucket）+ 磁盘文件（缺失不报错）+ 日志。缩略图缓存
/// 按 (path, mtime) 键成为孤儿——开发期容忍，整库重建可清。返回实际删除数。
pub fn fetch_duplicate_delete(state: &super::AppState, asset_ids: &[i64]) -> Result<u64, String> {
    let db = super::active_library_db(state)?;
    let mut deleted = 0u64;
    for id in asset_ids.iter().copied() {
        let path: Option<String> =
            db.0.query_row("SELECT path FROM assets WHERE id = ?1", [id], |r| r.get(0))
                .ok();
        let Some(path) = path else {
            continue; // 不存在：幂等跳过
        };
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                let _ = db.append_log(
                    "warn",
                    None,
                    &format!("重复删除：文件删除失败 {path}: {e}（库行仍清除）"),
                );
            }
        }
        let n =
            db.0.execute("DELETE FROM assets WHERE id = ?1", [id])
                .map_err(|e| e.to_string())?;
        deleted += n as u64;
        let _ = db.append_log("info", None, &format!("重复删除：{path}"));
    }
    Ok(deleted)
}

/// 两级去重组列表（kind = "exact" | "similar"；after = 上一页末组序号）。
#[tauri::command]
pub async fn duplicates_list(
    state: State<'_, SharedState>,
    kind: String,
    after: Option<usize>,
    limit: Option<usize>,
) -> Result<Vec<DuplicateGroupDto>, String> {
    let shared = state.inner().clone();
    let after = after.unwrap_or(0);
    let limit = limit.unwrap_or(50);
    run_blocking(shared, move |state| {
        fetch_duplicates_list(state, &kind, after, limit)
    })
    .await
}

/// 批量删除资产（文件 + 库行级联；重复清理页的确认动作）。
#[tauri::command]
pub async fn duplicate_delete(
    state: State<'_, SharedState>,
    asset_ids: Vec<i64>,
) -> Result<u64, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_duplicate_delete(state, &asset_ids)
    })
    .await
}

/// 并查集（近重复候选聚合；路径压缩 + 按秩合并）。
struct UnionFind {
    parent: std::collections::HashMap<i64, i64>,
    rank: std::collections::HashMap<i64, u32>,
}

impl UnionFind {
    fn new(ids: Vec<i64>) -> Self {
        let mut parent = std::collections::HashMap::with_capacity(ids.len());
        let mut rank = std::collections::HashMap::with_capacity(ids.len());
        for id in ids {
            parent.insert(id, id);
            rank.insert(id, 0);
        }
        Self { parent, rank }
    }

    fn find(&mut self, mut x: i64) -> i64 {
        let root = loop {
            let p = *self.parent.get(&x).expect("uf 成员必在");
            if p == x {
                break x;
            }
            // 路径压缩（两步跳）
            let g = *self.parent.get(&p).expect("uf 成员必在");
            self.parent.insert(x, g);
            if g == p {
                break p;
            }
            x = g;
        };
        root
    }

    fn union(&mut self, a: i64, b: i64) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        let (ranka, rankb) = (
            *self.rank.get(&ra).unwrap_or(&0),
            *self.rank.get(&rb).unwrap_or(&0),
        );
        let (child, root) = if ranka < rankb { (ra, rb) } else { (rb, ra) };
        self.parent.insert(child, root);
        if ranka == rankb {
            *self.rank.get_mut(&root).expect("uf 成员必在") += 1;
        }
    }

    /// 聚合为组（成员无序）。
    fn groups(self) -> Vec<Vec<i64>> {
        let Self { parent, .. } = self;
        let mut by_root: std::collections::HashMap<i64, Vec<i64>> = Default::default();
        for id in parent.keys().copied() {
            // 终态查找（无压缩，父链到根）
            let mut root = id;
            while let Some(&p) = parent.get(&root) {
                if p == root {
                    break;
                }
                root = p;
            }
            by_root.entry(root).or_default().push(id);
        }
        by_root.into_values().collect()
    }
}
