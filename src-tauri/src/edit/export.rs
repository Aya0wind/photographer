//! 导出执行器（阶段 D）：库内任务 + folder/album 两种落位模式。
//!
//! 任务铁律：每次导出先在 `export_job` 表建 queued 行，再由 IPC 层经
//! TaskSupervisor 后台线程执行 [`run_export_job`]——命令立刻返回任务 DTO，
//! 进度/收尾走 EventBus（前端 `app://event` 既有任务事件通道）。进程重启后
//! 表里仍是可查的历史/终态；遗留的 queued/running 行（进程中断）在下一次
//! `export_run` 时对照内存活跃集合收尸为 error（单文件导出无续传语义）。
//!
//! **folder 模式**：写 `outputDir/fileName.part` → 校验目标不存在 → 原子
//! rename；目标已存在报错（绝不覆盖）。
//!
//! **album 模式**：复用**导入引擎**的相册落位与登记路径，不平行造轮子——
//! 目录段走 [`crate::db::Db::album_item_home_rel`] 统一公式
//! （`{创建YYYY}/{创建MM}/{dir_name}[/{子组}]`，外层=相册创建时间年月
//! （UTC 口径）、相册内平铺——唯一例外 = 子组段（0022 物理化）——
//! 2026-09-28 布局定案；拍摄日分组在应用 UI），整体过
//! [`crate::import::templates::render_dir`] 净化；文件名 `{源stem}_edit.jpg`，
//! 冲突走导入引擎的
//! [`crate::import::templates::unique_path`]（`_1`/`_2` 可追踪后缀）；登记
//! 用 [`crate::db::Db::insert_asset_with_album`]（与导入引擎同一函数：资产
//! 行 + 缩略图/分析任务 + album_item（含 subgroup）同事务）。导出件就是
//! 普通资产进相册——无原片/成片关系语义（用户定案已移除该概念）。

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use xxhash_rust::xxh64::Xxh64;

use super::meta::{self, MetaOptions};
use super::recipe::EditRecipe;
use super::render;
use crate::db::{AssetRow, Db, ExportJobRow};
use crate::events::{AppEvent, AssetKind, EventBus};
use crate::import::templates::{render_dir, sanitize_component, unique_path, RenderCtx};

/// 导出落位模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportMode {
    Folder,
    Album,
}

impl ExportMode {
    /// 模式名（export_job.mode 列值）。
    pub fn as_str(self) -> &'static str {
        match self {
            ExportMode::Folder => "folder",
            ExportMode::Album => "album",
        }
    }
}

/// folder 模式目标（mode=folder 必填）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportFolderTarget {
    pub output_dir: String,
    pub file_name: String,
}

/// album 模式目标（mode=album 必填；subgroup 可空 = 相册根）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportAlbumTarget {
    pub album_id: String,
    pub subgroup: Option<String>,
}

/// 导出选项（IPC 契约，camelCase）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOptions {
    pub mode: ExportMode,
    #[serde(default)]
    pub folder: Option<ExportFolderTarget>,
    #[serde(default)]
    pub album: Option<ExportAlbumTarget>,
    /// null = 未指定（用配方 output.longEdge；都没有 = 源尺寸）。只缩不放。
    #[serde(default)]
    pub long_edge: Option<u32>,
    /// 未指定时用配方 output.quality（再缺省 90）。
    #[serde(default)]
    pub quality: Option<u8>,
    #[serde(default)]
    pub remove_gps: bool,
    #[serde(default)]
    pub copyright: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
}

/// 导出产物四元组（+ album 模式的新资产 id）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResultDto {
    pub output_path: String,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    /// album 模式：登记后的新资产 id；folder 模式 null。
    pub asset_id: Option<i64>,
}

/// 导出任务 DTO（export_run 返回值 / export_job 行投影）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportTaskDto {
    pub id: i64,
    pub asset_id: i64,
    pub mode: String,
    /// queued | running | done | error
    pub status: String,
    pub result: Option<ExportResultDto>,
    pub error: Option<String>,
}

impl From<ExportJobRow> for ExportTaskDto {
    fn from(row: ExportJobRow) -> Self {
        let is_done = row.status == "done";
        Self {
            id: row.id,
            asset_id: row.asset_id,
            mode: row.mode,
            status: row.status,
            result: row
                .output_path
                .as_deref()
                .filter(|_| is_done)
                .map(|path| ExportResultDto {
                    output_path: path.to_string(),
                    width: row.width.unwrap_or(0),
                    height: row.height.unwrap_or(0),
                    bytes: row.bytes.unwrap_or(0),
                    asset_id: row.new_asset_id,
                }),
            error: row.error,
        }
    }
}

/// 选项语义校验（结构由 serde 保证；这里管跨字段与目标合法性）。
pub fn validate_options(db: &Db, options: &ExportOptions) -> Result<ValidatedOptions, String> {
    let mode = options.mode;
    let (folder, album) = match mode {
        ExportMode::Folder => {
            let folder = options
                .folder
                .as_ref()
                .ok_or("folder 模式必须提供输出目录与文件名")?;
            if folder.output_dir.trim().is_empty() {
                return Err("输出目录不能为空".into());
            }
            let name = folder.file_name.trim();
            if name.is_empty() {
                return Err("输出文件名不能为空".into());
            }
            if name.contains('/') || name.contains('\\') {
                return Err("输出文件名不能包含路径分隔符".into());
            }
            (Some((folder.output_dir.clone(), name.to_string())), None)
        }
        ExportMode::Album => {
            let album = options.album.as_ref().ok_or("album 模式必须提供相册")?;
            let album_id: i64 = album
                .album_id
                .trim()
                .parse()
                .map_err(|_| format!("相册 id 非法: {}", album.album_id))?;
            if !db.album_exists(album_id).map_err(|e| e.to_string())? {
                return Err(format!("相册 {album_id} 不存在"));
            }
            let subgroup = album
                .subgroup
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            (None, Some((album_id, subgroup)))
        }
    };
    Ok(ValidatedOptions {
        mode,
        folder,
        album,
        long_edge: options.long_edge,
        quality: options.quality,
        remove_gps: options.remove_gps,
        copyright: options.copyright.clone(),
        author: options.author.clone(),
        keywords: options.keywords.clone(),
    })
}

/// 校验后的选项（folder/album 二选一已定）。
#[derive(Debug, Clone)]
pub struct ValidatedOptions {
    pub mode: ExportMode,
    pub folder: Option<(String, String)>,
    pub album: Option<(i64, Option<String>)>,
    pub long_edge: Option<u32>,
    pub quality: Option<u8>,
    pub remove_gps: bool,
    pub copyright: Option<String>,
    pub author: Option<String>,
    pub keywords: Vec<String>,
}

// ---------------------------------------------------------------------------
// 内存活跃任务集合（孤儿收尸对照；guard 保证 panic 也能摘除）
// ---------------------------------------------------------------------------

/// 进程级活跃任务集合（孤儿收尸对照；guard 保证 panic 也能摘除）。
/// 以任务 id 为键——单活跃库前提下 id 唯一（多库并行导出不在 v1 语义内）。
fn live_jobs() -> &'static Mutex<HashSet<i64>> {
    static LIVE: OnceLock<Mutex<HashSet<i64>>> = OnceLock::new();
    LIVE.get_or_init(|| Mutex::new(HashSet::new()))
}

/// 当前活跃（本进程内正在跑）的导出任务 id 快照。
pub fn live_job_ids() -> Vec<i64> {
    live_jobs()
        .lock()
        .expect("export live jobs mutex poisoned")
        .iter()
        .copied()
        .collect()
}

/// 进程重启孤儿收尸：queued/running 且不在活跃集合 → error（幂等）。
pub fn reap_orphan_jobs(db: &Db) {
    let live = live_job_ids();
    let Ok(stale) = db.export_job_stale_ids(&live) else {
        return;
    };
    for id in stale {
        let _ = db.export_job_fail(id, "进程重启中断");
    }
}

/// 活跃集合守卫（Drop 摘除——worker 正常/出错/panic 三态都收回）。
struct LiveGuard(i64);

impl LiveGuard {
    /// 构造即登记（防忘记插入的旁路构造）。
    #[allow(dead_code)] // 仅供 run_export_job 与测试构造
    fn new(id: i64) -> Self {
        live_jobs()
            .lock()
            .expect("export live jobs mutex poisoned")
            .insert(id);
        Self(id)
    }
}

impl Drop for LiveGuard {
    fn drop(&mut self) {
        live_jobs()
            .lock()
            .expect("export live jobs mutex poisoned")
            .remove(&self.0);
    }
}

// ---------------------------------------------------------------------------
// 任务执行
// ---------------------------------------------------------------------------

/// 单任务执行请求（IPC 层组装；worker 线程消费）。
pub struct ExportJobRequest {
    pub job_id: i64,
    pub asset_id: i64,
    /// 源资产行快照（启动时读取；文件路径以行为准）。
    pub asset: AssetRow,
    pub recipe: EditRecipe,
    pub options: ValidatedOptions,
    /// 照片根（album 模式落位锚点）。
    pub photo_root: PathBuf,
}

/// 执行一个导出任务至终态（running → done|error + 事件）。阻塞；调用方
/// 决定线程（IPC 层 supervisor / 集成测试直调）。
pub fn run_export_job(db: Db, bus: &EventBus, request: ExportJobRequest) {
    let _live = LiveGuard::new(request.job_id);
    let asset_id = request.asset_id;
    let progress = |phase: &str| {
        bus.publish(AppEvent::ExportTaskProgress {
            job_id: request.job_id,
            asset_id,
            phase: phase.to_string(),
        });
    };
    let finish = |ok: bool,
                  output_path: Option<String>,
                  new_asset_id: Option<i64>,
                  error: Option<String>| {
        bus.publish(AppEvent::ExportTaskFinished {
            job_id: request.job_id,
            asset_id,
            ok,
            output_path,
            new_asset_id,
            error,
        });
    };
    let _ = db.export_job_set_status(request.job_id, "running");
    match execute(&db, &request, &progress) {
        Ok(result) => {
            let _ = db.export_job_finish(
                request.job_id,
                &result.output_path,
                result.width,
                result.height,
                result.bytes,
                result.asset_id,
            );
            finish(true, Some(result.output_path), result.asset_id, None);
        }
        Err(error) => {
            let _ = db.export_job_fail(request.job_id, &error);
            finish(false, None, None, Some(error));
        }
    }
}

/// 任务执行核（一个任务一次调用；进度经回调上报）。
fn execute(
    db: &Db,
    request: &ExportJobRequest,
    progress: &dyn Fn(&str),
) -> Result<ExportResultDto, String> {
    let asset = &request.asset;
    let src = PathBuf::from(&asset.path);
    if !src.is_file() {
        return Err(format!("源文件不在盘: {}", asset.path));
    }

    // ① 渲染（解码/转正/旋转/裁剪/标注/只缩不放）
    progress("render");
    let long_edge = request
        .options
        .long_edge
        .or(request.recipe.output.long_edge);
    let rendered = render::render_recipe(&src, &request.recipe, long_edge)?;
    let (width, height) = (rendered.image.width(), rendered.image.height());

    // ② 编码（sRGB JPEG；质量 options → 配方 → 90）
    progress("encode");
    let quality = request
        .options
        .quality
        .unwrap_or(request.recipe.output.quality)
        .clamp(1, 100);
    let jpeg = render::encode_jpeg(&rendered.image, quality)?;

    // ③ 元数据（源 EXIF 尽量保留 + removeGps + 版权/作者/关键词）
    let source_head = read_head(&src, 2 * 1024 * 1024);
    let meta_opts = MetaOptions {
        remove_gps: request.options.remove_gps,
        copyright: request.options.copyright.clone(),
        author: request.options.author.clone(),
        keywords: request.options.keywords.clone(),
    };
    let jpeg = meta::apply_metadata(jpeg, source_head.as_deref(), &meta_opts);

    // ④ 落位 + 登记
    match request.options.mode {
        ExportMode::Folder => {
            progress("write");
            let (dir, name) = request.options.folder.as_ref().expect("validated");
            let dst = write_folder(&jpeg, dir, name)?;
            Ok(ExportResultDto {
                output_path: dst.to_string_lossy().into_owned(),
                width,
                height,
                bytes: jpeg.len() as u64,
                asset_id: None,
            })
        }
        ExportMode::Album => {
            progress("register");
            let (album_id, subgroup) = request.options.album.clone().expect("validated");
            let (dst, new_asset_id) = place_in_album(
                db,
                &jpeg,
                asset,
                &request.options,
                &request.photo_root,
                width,
                height,
                album_id,
                subgroup,
            )?;
            Ok(ExportResultDto {
                output_path: dst.to_string_lossy().into_owned(),
                width,
                height,
                bytes: jpeg.len() as u64,
                asset_id: Some(new_asset_id),
            })
        }
    }
}

/// 读文件头至多 `limit` 字节（EXIF 提取用；JPEG APP1 / TIFF IFD 链都在头
/// 部；越界值由 continue_on_error 容错丢弃）。
fn read_head(src: &Path, limit: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(src).ok()?;
    let mut buf = Vec::new();
    (&mut file).take(limit).read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// folder 模式落位：目标存在报错；.part 写满后原子改名。
fn write_folder(jpeg: &[u8], dir: &str, name: &str) -> Result<PathBuf, String> {
    let dst = Path::new(dir).join(name);
    if dst.exists() {
        return Err(format!("目标文件已存在（不覆盖）: {}", dst.display()));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("创建输出目录失败: {e}"))?;
    let part = dst.with_extension("part");
    atomic_write(&part, &dst, jpeg)?;
    Ok(dst)
}

/// album 模式落位 + 登记（导入引擎同款路径）。
#[allow(clippy::too_many_arguments)]
fn place_in_album(
    db: &Db,
    jpeg: &[u8],
    source: &AssetRow,
    options: &ValidatedOptions,
    photo_root: &Path,
    width: u32,
    height: u32,
    album_id: i64,
    subgroup: Option<String>,
) -> Result<(PathBuf, i64), String> {
    let home_rel = db
        .album_item_home_rel(album_id, subgroup.as_deref())
        .map_err(|e| e.to_string())?
        .ok_or("相册不存在")?;
    let stem = source
        .filename
        .rsplit_once('.')
        .map(|(s, _)| s.to_string())
        .unwrap_or_else(|| source.filename.clone());
    let captured_at = captured_or_fallback(source);
    // 与导入引擎同一布局公式（album_item_home_rel：`{创建YYYY}/{创建MM}/
    // {dir_name}[/{子组}]`——相册内平铺，唯一例外 = 子组段（0022 物理化），
    // subgroup Some 时导出件落进对应子文件夹）+ 同一渲染器过 sanitize——
    // 同一相册的导出件与导入件落进同一个相册目录（同子组同文件夹）。
    // 模板为纯字面量（无逐照片令牌），captured_at 上下文仅形态沿用。
    let ctx = RenderCtx {
        captured_at,
        camera: None,
        lens: None,
        original_stem: stem.clone(),
        ext: "jpg".to_string(),
    };
    let rel = render_dir(&home_rel, &ctx).map_err(|e| format!("导出目录渲染失败: {e}"))?;
    let dir = photo_root.join(rel);
    let name = format!("{}_edit.jpg", sanitize_component(&stem));
    let dst = unique_path(&dir, &name); // 导入引擎命名冲突规则（_1/_2）
    atomic_write(&dst.with_extension("part"), &dst, jpeg)?;

    // 登记：与导入引擎同一入库函数（资产行 + 索引任务 + album_item 同事务）。
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let row = AssetRow {
        path: dst.to_string_lossy().into_owned(),
        filename: dst
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| name.clone()),
        size: jpeg.len() as u64,
        mtime: now.clone(),
        xxhash: xxh64_of(jpeg),
        kind: AssetKind::Photo,
        captured_at: source.captured_at.clone(),
        camera: source.camera.clone(),
        source: "imported".into(),
        created_at: now,
        origin: "imported".into(),
        width: Some(width),
        height: Some(height),
        iso: source.iso,
        f_number: source.f_number.clone(),
        exposure_time: source.exposure_time.clone(),
        focal_length: source.focal_length.clone(),
        lens: source.lens.clone(),
        pair_asset_id: None,
        thumb_state: 0,
        // 导出件方向已烘焙转正；资产行口径与缩略图管线一致（存 EXIF 原值
        // 的是导入路径，这里产物是转正后的新文件）。
        orientation: Some(1),
        flash: source.flash.clone(),
        metering_mode: source.metering_mode.clone(),
        white_balance: source.white_balance.clone(),
        exposure_program: source.exposure_program.clone(),
        software: source.software.clone(),
        artist: options
            .author
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| source.artist.clone()),
        gps_lat: if options.remove_gps {
            None
        } else {
            source.gps_lat
        },
        gps_lon: if options.remove_gps {
            None
        } else {
            source.gps_lon
        },
        rating: 0,
        flagged: 0,
        color_label: None,
        rejected: 0,
    };
    db.insert_asset_with_album(&row, Some(album_id), subgroup.as_deref())
        .map_err(|e| format!("导出件登记失败: {e}"))?;
    let new_id = db
        .asset_id_by_path(&row.path)
        .map_err(|e| e.to_string())?
        .ok_or("导出件登记后无法读回资产 id")?;
    Ok((dst, new_id))
}

/// 资产拍摄时间（UTC）：captured_at → mtime → now（与模板引擎
/// resolve_captured 的 EXIF ?? mtime 精神一致）。
fn captured_or_fallback(asset: &AssetRow) -> DateTime<Utc> {
    let parse = |text: &str| {
        DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|t| t.with_timezone(&Utc))
    };
    asset
        .captured_at
        .as_deref()
        .and_then(parse)
        .or_else(|| parse(&asset.mtime))
        .unwrap_or_else(Utc::now)
}

/// `.part` 写满 + 原子改名（导入/缩略图同款两段式；写失败清残留）。
fn atomic_write(part: &Path, dst: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目标目录失败: {e}"))?;
    }
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(part)?;
        file.write_all(bytes)?;
        file.flush()
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(part);
        return Err(format!("写导出文件失败: {e}"));
    }
    std::fs::rename(part, dst).map_err(|e| {
        let _ = std::fs::remove_file(part);
        format!("导出文件落位失败: {e}")
    })
}

fn xxh64_of(bytes: &[u8]) -> u64 {
    let mut hasher = Xxh64::new(0);
    hasher.update(bytes);
    hasher.digest()
}

/// 等待任务到达终态（测试辅助；超时返回 false）。
#[allow(dead_code)] // 集成测试（tests/edit_export_test.rs）引用
pub fn wait_terminal(db: &Db, job_id: i64, timeout: Duration) -> bool {
    let started = std::time::Instant::now();
    loop {
        let terminal = db
            .export_job_get(job_id)
            .ok()
            .flatten()
            .is_some_and(|row| matches!(row.status.as_str(), "done" | "error"));
        if terminal {
            return true;
        }
        if started.elapsed() > timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
