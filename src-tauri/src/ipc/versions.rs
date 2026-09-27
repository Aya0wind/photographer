//! 版本关系命令（阶段 B2）：LR 成片导回扫描/入库 + 资产版本查询。
//!
//! `lr_export_scan`：扫描 LR 导出目录新增 JPEG（size+xxhash 判库内已有，
//! 排除重复），对每个候选给出候选原片列表——匹配依据三通道：①文件名
//! 模板解析（`DSC_1234_edit_v1.jpg` → 基名 `DSC_1234` 匹配库内同名
//! RAW/JPEG）②EXIF 拍摄时间 ±2s 窗口 ③pHash 汉明距离（相似桶同源算法，
//! ≤6 近重复 / ≤10 连拍同景）。score = 各依据加分封顶 100，basis 记
//! JSON（filename/exif_time/phash_score 组合）。无候选进「待关联」。
//!
//! `lr_export_import`：复制成片入库（kind=photo 走正常缩略图/索引管线）+
//! asset_relation(derived_from, confirmed=1) + photo_group 归组（原片组
//! 追加 derived；无组则建）+ 可选挂相册；幂等（同文件重复导入按已有资产
//! 跳过）。落盘位置 = `{photoRoot}/{importSubdir}/{原片stem}/`（应用写入
//! 区内、与原片同 stem 子目录，重命名不丢关系——关系落库为准）。
//!
//! `asset_versions`：详情页版本切换数据源（组员 raw→sooc→derived 序）。
//!
//! 全部走 active_library_db + run_blocking（铁律：磁盘 IO / 大结果集 DB
//! 查询不上主线程）。

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::State;

use super::{run_blocking, SharedState};

/// pHash 匹配的汉明上限（近重复 ≤6 满分档；≤10 连拍同景降分档）。
const PHASH_HAMMING_MAX: u32 = 10;
/// EXIF 拍摄时间匹配窗口（±2s，roadmap §5）。
const EXIF_WINDOW_SECS: i64 = 2;
/// 单个导出文件的候选原片数上限。
const CANDIDATE_CAP: usize = 5;

/// 候选原片（含匹配依据与得分）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LrExportMatchDto {
    pub asset_id: i64,
    /// 库内原片文件名。
    pub name: String,
    /// 0-100（各依据加分封顶）。
    pub score: u32,
    /// 匹配依据 JSON（bases: filename/exif_time/phash_score 组合，供
    /// match_basis 落库与前端展示）。
    pub basis: String,
}

/// 导出文件候选（candidates 空 = 「待关联」）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LrExportCandidateDto {
    /// 导出目录内文件的绝对路径。
    pub path: String,
    pub size: u64,
    pub candidates: Vec<LrExportMatchDto>,
}

/// 导入单条匹配（path = 扫描结果里的候选文件；basis 透传扫描给出的依据
/// JSON 落 match_basis；缺省 None）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LrExportImportItem {
    pub path: String,
    pub source_asset_id: i64,
    #[serde(default)]
    pub basis: Option<String>,
}

/// 导入结果汇总。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LrImportResultDto {
    pub imported: u64,
    /// 已在库（size+xxhash 命中）跳过数——幂等重放的正常出口。
    pub skipped: u64,
    pub failed: Vec<LrImportFailureDto>,
}

/// 单条失败（路径 + 原因；不整批回滚）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LrImportFailureDto {
    pub path: String,
    pub error: String,
}

/// 版本组成员（详情页版本切换条目）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionMemberDto {
    pub asset_id: i64,
    /// raw | sooc | derived（未入组资产 None）。
    pub role: Option<String>,
    pub name: String,
    /// 缩略图就绪（thumb_state==1）。
    pub thumb_ready: bool,
}

/// 版本查询载荷：asset_id 未入组时 group_id=None 且 members 只有自己。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetVersionsDto {
    pub group_id: Option<i64>,
    pub members: Vec<VersionMemberDto>,
}

/// SQL LIKE 转义（ESCAPE '\'）：模式里的 \ % _ 按字面匹配。
fn like_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// stem → 基名前缀候选（按分隔符切分，最长优先；至少 4 字符防空泛前缀）。
/// 例：`DSC_1234_edit_v1` → [`DSC_1234_edit_v1`, `DSC_1234_edit`, `DSC_1234`]。
fn base_stem_candidates(stem: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![stem.to_string()];
    let bytes: Vec<char> = stem.chars().collect();
    for i in (0..bytes.len()).rev() {
        if matches!(bytes[i], '_' | '-' | ' ' | '(' | '[') && i >= 4 {
            out.push(bytes[..i].iter().collect());
        }
    }
    out.into_iter().take(3).collect()
}

/// 流式读文件：产出 (head ≤1MB, 全文件 xxh64)。
fn hash_file(path: &Path) -> Result<(Vec<u8>, u64), String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("打开失败: {e}"))?;
    let mut head = Vec::with_capacity(1024 * 1024);
    let mut hasher = xxhash_rust::xxh64::Xxh64::new(0);
    let mut chunk = vec![0u8; 8 * 1024 * 1024];
    loop {
        let n = file
            .read(&mut chunk)
            .map_err(|e| format!("读取失败: {e}"))?;
        if n == 0 {
            break;
        }
        if head.len() < 1024 * 1024 {
            let take = n.min(1024 * 1024 - head.len());
            head.extend_from_slice(&chunk[..take]);
        }
        hasher.update(&chunk[..n]);
    }
    Ok((head, hasher.digest()))
}

/// 文件名 stem（无扩展名 = 原名）。
fn stem_of(filename: &str) -> String {
    match filename.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => stem.to_string(),
        _ => filename.to_string(),
    }
}

/// 扫描核：目录 → 新增 JPEG 候选 + 候选原片列表。
pub fn lr_export_scan_core(
    db: &crate::db::Db,
    dir: &str,
) -> Result<Vec<LrExportCandidateDto>, String> {
    let root = PathBuf::from(dir);
    if !root.is_dir() {
        return Err(format!("目录不存在: {dir}"));
    }
    let mut out = Vec::new();
    let mut paths: Vec<PathBuf> = walkdir::WalkDir::new(&root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| {
            p.extension().is_some_and(|ext| {
                matches!(
                    ext.to_ascii_lowercase().to_str(),
                    Some("jpg") | Some("jpeg")
                )
            })
        })
        .collect();
    paths.sort();
    for path in paths {
        let display = path.to_string_lossy().into_owned();
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let size = meta.len();
        let (head, xxhash) = match hash_file(&path) {
            Ok(v) => v,
            Err(error) => {
                eprintln!("[lr_export_scan] 跳过不可读文件 {display}: {error}");
                continue;
            }
        };
        // 库内已有（size+xxhash 命中）→ 非新增，整文件跳过
        let known = db
            .find_asset_by_size_xxh(size, xxhash)
            .map_err(|e| e.to_string())?;
        if known.is_some() {
            continue;
        }
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let candidates = match_export_candidates(db, &path, &stem_of(&file_name), size, &head)
            .map_err(|e| e.to_string())?;
        out.push(LrExportCandidateDto {
            path: display,
            size,
            candidates,
        });
    }
    Ok(out)
}

/// 三通道候选匹配（文件名模板 / EXIF 时间窗口 / pHash 汉明）。
fn match_export_candidates(
    db: &crate::db::Db,
    path: &Path,
    stem: &str,
    _size: u64,
    head: &[u8],
) -> rusqlite::Result<Vec<LrExportMatchDto>> {
    use std::collections::HashMap;
    // asset_id → (name, score, bases, extras)
    struct Cand {
        name: String,
        score: u32,
        bases: Vec<&'static str>,
        time_delta_ms: Option<i64>,
        hamming: Option<u32>,
    }
    let mut cands: HashMap<i64, Cand> = HashMap::new();
    fn push(
        cands: &mut HashMap<i64, Cand>,
        id: i64,
        name: String,
        score: u32,
        basis: &'static str,
        time_delta_ms: Option<i64>,
        hamming: Option<u32>,
    ) {
        let entry = cands.entry(id).or_insert(Cand {
            name,
            score: 0,
            bases: Vec::new(),
            time_delta_ms: None,
            hamming: None,
        });
        entry.score = (entry.score + score).min(100);
        if !entry.bases.contains(&basis) {
            entry.bases.push(basis);
        }
        if time_delta_ms.is_some() {
            entry.time_delta_ms = time_delta_ms;
        }
        if hamming.is_some() {
            entry.hamming = hamming;
        }
    }

    // ① 文件名模板解析：基名前缀逐级匹配库内同名 stem（任意扩展名）
    for (i, prefix) in base_stem_candidates(stem).into_iter().enumerate() {
        let pattern = format!("{}.%", like_escape(&prefix).to_lowercase());
        let mut stmt = db.0.prepare(
            "SELECT id, filename FROM assets \
             WHERE kind IN ('photo', 'raw') AND in_trash = 0 \
               AND lower(filename) LIKE ?1 ESCAPE '\\' LIMIT 8",
        )?;
        let rows = stmt.query_map(rusqlite::params![pattern], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        // 全 stem 相等（i==0）100 分；后缀模板剥离（i>0）70 分
        let score = if i == 0 { 100 } else { 70 };
        for row in rows {
            let (id, name) = row?;
            push(&mut cands, id, name, score, "filename", None, None);
        }
    }

    // ② EXIF 拍摄时间 ±2s 窗口
    let meta = crate::metadata::exif_lite::parse(head);
    if let Some(captured) = meta.captured_at {
        let from = (captured - chrono::Duration::seconds(EXIF_WINDOW_SECS))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let to = (captured + chrono::Duration::seconds(EXIF_WINDOW_SECS))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let mut stmt = db.0.prepare(
            "SELECT id, filename, captured_at FROM assets \
             WHERE kind IN ('photo', 'raw') AND in_trash = 0 \
               AND captured_at IS NOT NULL AND captured_at >= ?1 AND captured_at <= ?2 \
             LIMIT 32",
        )?;
        let rows = stmt.query_map(rusqlite::params![from, to], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (id, name, at) = row?;
            let delta = chrono::DateTime::parse_from_rfc3339(&at).ok().map(|t| {
                (t.with_timezone(&chrono::Utc) - captured)
                    .num_milliseconds()
                    .abs()
            });
            push(&mut cands, id, name, 60, "exif_time", delta, None);
        }
    }

    // ③ pHash 汉明：导出 JPEG 解码出指纹；对候选精确过滤；零候选时全库寻近邻
    let export_phash: Option<u64> = image::ImageReader::open(path)
        .ok()
        .and_then(|r| r.decode().ok())
        .map(|img| crate::metadata::phash::phash_of_gray(&img.to_luma8()));
    if let Some(export_h) = export_phash {
        let hamming_of = |phash: u64| crate::metadata::phash::hamming(export_h, phash);
        let score_of = |d: u32| -> u32 {
            if d <= 6 {
                50 - 2 * d
            } else {
                30 - (d - 7) * 3
            }
        };
        // 候选内精确过滤（有指纹者）
        let ids: Vec<i64> = cands.keys().copied().collect();
        if !ids.is_empty() {
            let slots = (0..ids.len())
                .map(|i| format!("?{}", i + 1))
                .collect::<Vec<_>>()
                .join(", ");
            let sql =
                format!("SELECT id, phash FROM assets WHERE id IN ({slots}) AND phash IS NOT NULL");
            let mut stmt = db.0.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (id, phash) = row?;
                let d = hamming_of(phash as u64);
                if d <= PHASH_HAMMING_MAX {
                    let (name, _, _, delta, _) = {
                        let c = &cands[&id];
                        (
                            c.name.clone(),
                            c.score,
                            c.bases.clone(),
                            c.time_delta_ms,
                            c.hamming,
                        )
                    };
                    push(
                        &mut cands,
                        id,
                        name,
                        score_of(d),
                        "phash_score",
                        delta,
                        Some(d),
                    );
                }
            }
        }
        // 零候选 → 全库近邻兜底（20 万行 MB 级单查询，后台线程可承受）
        if cands.is_empty() {
            let mut stmt = db.0.prepare(
                "SELECT id, filename, phash FROM assets \
                 WHERE kind IN ('photo', 'raw') AND in_trash = 0 AND phash IS NOT NULL",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?;
            let mut near: Vec<(i64, String, u32)> = rows
                .filter_map(Result::ok)
                .map(|(id, name, phash)| (id, name, hamming_of(phash as u64)))
                .filter(|(_, _, d)| *d <= PHASH_HAMMING_MAX)
                .collect();
            near.sort_by_key(|(_, _, d)| *d);
            for (id, name, d) in near.into_iter().take(CANDIDATE_CAP) {
                push(
                    &mut cands,
                    id,
                    name,
                    score_of(d),
                    "phash_score",
                    None,
                    Some(d),
                );
            }
        }
    }

    // 装配：score 降序、id tiebreak；basis = 组合 JSON
    let mut list: Vec<(i64, Cand)> = cands.into_iter().collect();
    list.sort_by(|a, b| b.1.score.cmp(&a.1.score).then(a.0.cmp(&b.0)));
    Ok(list
        .into_iter()
        .take(CANDIDATE_CAP)
        .map(|(id, c)| {
            let mut basis = serde_json::Map::new();
            basis.insert(
                "bases".into(),
                serde_json::Value::Array(
                    c.bases
                        .iter()
                        .map(|b| serde_json::Value::String((*b).into()))
                        .collect(),
                ),
            );
            if let Some(delta) = c.time_delta_ms {
                basis.insert("time_delta_ms".into(), serde_json::Value::from(delta));
            }
            if let Some(d) = c.hamming {
                basis.insert("phash_hamming".into(), serde_json::Value::from(d));
            }
            LrExportMatchDto {
                asset_id: id,
                name: c.name,
                score: c.score,
                basis: serde_json::Value::Object(basis).to_string(),
            }
        })
        .collect())
}

/// 成片导回归一目录：`{photoRoot}/{importSubdir}/{原片stem}/`，原名冲突
/// 递增 ` (n)` 后缀（沿用导入引擎「原名保留+后缀」约定）。
fn render_derived_dst(
    photo_root: &str,
    import_subdir: &str,
    source_stem: &str,
    filename: &str,
) -> PathBuf {
    let dir = Path::new(photo_root).join(import_subdir).join(source_stem);
    let (stem, ext) = match filename.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), e.to_string()),
        _ => (filename.to_string(), String::new()),
    };
    let mut candidate = dir.join(filename);
    let mut n = 2;
    while candidate.exists() {
        let name = if ext.is_empty() {
            format!("{stem} ({n})")
        } else {
            format!("{stem} ({n}).{ext}")
        };
        candidate = dir.join(name);
        n += 1;
    }
    candidate
}

/// 导入核：逐条复制入库 + 建关系 + 归组（+ 可选挂相册）。单条失败不整批
/// 回滚；同 (size, xxhash) 已在库 → skip 计数（幂等）。
pub fn lr_export_import_core(
    state: &super::AppState,
    db: &crate::db::Db,
    matches: &[LrExportImportItem],
    album_id: Option<i64>,
) -> Result<LrImportResultDto, String> {
    use crate::events::AssetKind;

    let (photo_root, import_subdir) = {
        let settings = state
            .settings
            .lock()
            .expect("settings mutex poisoned")
            .clone();
        let library = settings.active_library().ok_or("尚未创建库")?.clone();
        (library.photo_root, library.import_subdir)
    };

    let mut result = LrImportResultDto {
        imported: 0,
        skipped: 0,
        failed: Vec::new(),
    };
    for item in matches {
        let mut fail = |error: String| {
            result.failed.push(LrImportFailureDto {
                path: item.path.clone(),
                error,
            });
        };
        let src = PathBuf::from(&item.path);
        if !src.is_file() {
            fail(format!("源文件不存在: {}", item.path));
            continue;
        }
        // 原片必须存在且未入回收站
        let source = db
            .asset_by_id(item.source_asset_id)
            .map_err(|e| e.to_string())?;
        let Some(source) = source else {
            fail(format!("原片资产不存在: {}", item.source_asset_id));
            continue;
        };
        let trashed: i64 =
            db.0.query_row(
                "SELECT in_trash FROM assets WHERE id = ?1",
                [item.source_asset_id],
                |r| r.get(0),
            )
            .unwrap_or(1);
        if trashed != 0 {
            fail(format!("原片资产在回收站: {}", item.source_asset_id));
            continue;
        }

        // 流式复制 + head/xxhash（.part 原子落位，冲突后缀在最终 rename 前定）
        let (head, xxhash) = match hash_file(&src) {
            Ok(v) => v,
            Err(error) => {
                fail(error);
                continue;
            }
        };
        let size = src.metadata().map(|m| m.len()).unwrap_or(0);
        // 幂等：同指纹已在库 → 跳过（不重复入库、不重复建关系）
        let known = db
            .find_asset_by_size_xxh(size, xxhash)
            .map_err(|e| e.to_string())?;
        if known.is_some() {
            result.skipped += 1;
            continue;
        }
        let kind = crate::devices::classify(&item.path, &head);
        if kind != AssetKind::Photo {
            fail("仅支持 JPEG 成片入库（kind 识别非 photo）".into());
            continue;
        }
        let meta = crate::metadata::exif_lite::parse(&head);
        let dst = render_derived_dst(
            &photo_root,
            &import_subdir,
            &stem_of(&source.filename),
            &src.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        if let Some(parent) = dst.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                fail(format!("创建目录失败: {e}"));
                continue;
            }
        }
        let part = dst.with_extension("part");
        if let Err(e) = std::fs::copy(&src, &part).map(|_| std::fs::rename(&part, &dst)) {
            let _ = std::fs::remove_file(&part);
            fail(format!("复制失败: {e}"));
            continue;
        }
        let mtime = src
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .map(|t| {
                chrono::DateTime::<chrono::Utc>::from(t)
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            })
            .unwrap_or_else(|| {
                chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            });
        let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let filename = dst
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let row = crate::db::AssetRow {
            path: dst.to_string_lossy().into_owned(),
            filename,
            size,
            mtime,
            xxhash,
            kind: AssetKind::Photo,
            captured_at: meta
                .captured_at
                .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
            camera: meta.camera.clone(),
            source: "imported".into(),
            created_at,
            origin: "imported".into(),
            width: meta.width,
            height: meta.height,
            iso: meta.iso,
            f_number: meta.f_number.clone(),
            exposure_time: meta.exposure_time.clone(),
            focal_length: meta.focal_length.clone(),
            lens: meta.lens.clone(),
            pair_asset_id: None,
            thumb_state: 0,
            orientation: meta.deep.orientation,
            flash: meta.deep.flash.clone(),
            metering_mode: meta.deep.metering_mode.clone(),
            white_balance: meta.deep.white_balance.clone(),
            exposure_program: meta.deep.exposure_program.clone(),
            software: meta.deep.software.clone(),
            artist: meta.deep.artist.clone(),
            gps_lat: meta.deep.gps_lat,
            gps_lon: meta.deep.gps_lon,
            rating: 0,
            flagged: 0,
            color_label: None,
            rejected: 0,
        };
        if let Err(e) = db.insert_asset_with_album(&row, album_id) {
            let _ = std::fs::remove_file(&dst);
            fail(format!("入库失败: {e}"));
            continue;
        }
        let new_id = match db.asset_id_by_path(&row.path) {
            Ok(Some(id)) => id,
            _ => {
                fail("入库后定位资产失败".into());
                continue;
            }
        };
        // 关系 + 归组（原片组追加 derived；无组则建）
        if let Err(e) = db.group_link_derived(new_id, item.source_asset_id) {
            fail(format!("归组失败: {e}"));
            continue;
        }
        if let Err(e) = db.asset_relation_add(
            new_id,
            item.source_asset_id,
            Some("lr_export"),
            item.basis.as_deref(),
        ) {
            fail(format!("建关系失败: {e}"));
            continue;
        }
        let _ = db.append_log(
            "info",
            None,
            &format!(
                "LR 成片导回：{} ← 原片 {}（derived_from）",
                row.path, source.filename
            ),
        );
        result.imported += 1;
    }
    Ok(result)
}

/// 版本查询核：组员 raw→sooc→derived 序；未入组返回自己（role=None）。
pub fn asset_versions_core(db: &crate::db::Db, asset_id: i64) -> Result<AssetVersionsDto, String> {
    let group_id = db.asset_group_of(asset_id).map_err(|e| e.to_string())?;
    let Some(group_id) = group_id else {
        let member = db
            .asset_by_id(asset_id)
            .map_err(|e| e.to_string())?
            .map(|a| VersionMemberDto {
                asset_id,
                role: None,
                name: a.filename,
                thumb_ready: a.thumb_state == 1,
            });
        return Ok(AssetVersionsDto {
            group_id: None,
            members: member.into_iter().collect(),
        });
    };
    let members: Vec<VersionMemberDto> = db
        .group_members(group_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|(mid, role)| {
            let (name, thumb_state) =
                db.0.query_row(
                    "SELECT filename, thumb_state FROM assets WHERE id = ?1",
                    [mid],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                        ))
                    },
                )
                .map_err(|e| e.to_string())?;
            Ok(VersionMemberDto {
                asset_id: mid,
                role: Some(role),
                name,
                thumb_ready: thumb_state == 1,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(AssetVersionsDto {
        group_id: Some(group_id),
        members,
    })
}

// ---------------------------------------------------------------------------
// Tauri 命令壳（async + spawn_blocking）
// ---------------------------------------------------------------------------

/// 扫描 LR 导出目录（dir = 导出目录绝对路径）。
#[tauri::command]
pub async fn lr_export_scan(
    state: State<'_, SharedState>,
    dir: String,
) -> Result<Vec<LrExportCandidateDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        let db = super::active_library_db(state)?;
        lr_export_scan_core(&db, &dir)
    })
    .await
}

/// 成片导回入库（matches = 扫描结果确认后的 path→原片映射；album_id 可选）。
#[tauri::command]
pub async fn lr_export_import(
    state: State<'_, SharedState>,
    matches: Vec<LrExportImportItem>,
    album_id: Option<i64>,
) -> Result<LrImportResultDto, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        let db = super::active_library_db(state)?;
        lr_export_import_core(state, &db, &matches, album_id)
    })
    .await
}

/// 版本查询（详情页版本切换数据源）。
#[tauri::command]
pub async fn asset_versions(
    state: State<'_, SharedState>,
    asset_id: i64,
) -> Result<AssetVersionsDto, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        let db = super::active_library_db(state)?;
        asset_versions_core(&db, asset_id)
    })
    .await
}
