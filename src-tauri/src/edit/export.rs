//! 导出执行器（阶段 D）：库内任务 + folder/album 两种落位模式。
//!
//! 任务铁律：每次导出先在 `export_job` 表建 queued 行，再由 IPC 层经
//! TaskSupervisor 后台线程执行 [`run_export_job`]——命令立刻返回任务 DTO，
//! 进度/收尾走 EventBus（前端 `app://event` 既有任务事件通道）。进程重启后
//! 表里仍是可查的历史/终态；遗留的 queued/running 行（进程中断）在下一次
//! `export_run` 时对照内存活跃集合收尸为 error（单文件导出无续传语义）。
//!
//! **folder 模式**：同目录唯一暂存文件写满后，以不覆盖方式落位；
//! 即使目标在写入期间出现也报错，避免竞争覆盖。
//!
//! **album 模式**：复用**导入引擎**的落位与登记路径，不平行造轮子——
//! 2026-10-09 纯时间布局定案（§三）：导出件落**源资产所属照片库**
//! `{库root}/{拍摄年}/{拍摄月}/{源stem}_edit.jpg`（拍摄时间与导入引擎同
//! 口径：EXIF 缺失回退 mtime）；冲突走导入引擎的
//! [`crate::db::organization::unique_path`]（`_1`/`_2` 可追踪后缀）；登记
//! 用 [`crate::db::Db::insert_asset_with_album`]（与导入引擎同一函数：资产
//! 行 + 缩略图/分析任务 + album_item（含 subgroup）同事务；library_id 随
//! 源资产）。导出件就是普通资产——无原片/成片关系语义（用户定案已移除
//! 该概念）；album/subgroup 只是逻辑引用，不影响落位（§一）。

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use xxhash_rust::xxh64::Xxh64;

use super::meta::{self, MetaOptions};
use super::recipe::EditRecipe;
use crate::db::{AssetRow, Db, ExportJobRow};
use crate::events::{AppEvent, AssetKind, EventBus};
use crate::db::organization::{sanitize_component, unique_path};

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

/// 保存回照片库时可选的相册引用；subgroup 可空。
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
    #[serde(default)]
    pub file_name:Option<String>,
    #[serde(default)]
    pub finish:super::output_finish::Options,
    #[serde(default)]
    pub format:ExportFormat,
    #[serde(default)]
    pub color_space:ExportColorSpace,
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
#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize,Default)]
#[serde(rename_all="lowercase")]
pub enum ExportFormat {#[default]Jpeg,Png,Tiff,Pcraft}
impl ExportFormat {pub fn extension(self)->&'static str{match self{Self::Jpeg=>"jpg",Self::Png=>"png",Self::Tiff=>"tif",Self::Pcraft=>"pcraft"}}}
#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize,Default)]
#[serde(rename_all="camelCase")]
pub enum ExportColorSpace {#[default]Srgb,DisplayP3,AdobeRgb,ProPhotoRgb}
impl ExportColorSpace {
    pub fn profile(self)->&'static photocraft_cms::Profile {
        use photocraft_cms::Builtin;
        match self{Self::Srgb=>Builtin::Srgb,Self::DisplayP3=>Builtin::DisplayP3,Self::AdobeRgb=>Builtin::AdobeRgbCompat,Self::ProPhotoRgb=>Builtin::ProPhotoCompat}.profile()
    }
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
    pub phase: Option<String>,
}

impl From<ExportJobRow> for ExportTaskDto {
    fn from(row: ExportJobRow) -> Self {
        let is_done = row.status == "done";
        Self {
            id: row.id,
            asset_id: row.asset_id,
            mode: row.mode,
            status: if row.status == "error" && row.error.as_deref() == Some("导出已取消") { "cancelled".into() } else { row.status },
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
            phase: job_phase(row.id),
            error: row.error,
        }
    }
}

/// 选项语义校验（结构由 serde 保证；这里管跨字段与目标合法性）。
pub fn validate_options(db: &Db, options: &ExportOptions) -> Result<ValidatedOptions, String> {
    super::output_finish::validate(&options.finish)?;
    if options.format==ExportFormat::Pcraft&&options.mode!=ExportMode::Folder{return Err("可编辑文档请导出到文件夹，不能作为照片加入相册".into());}
    if let Some(name)=&options.file_name { validate_file_name(name.trim(), options.format)?; }
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
            validate_file_name(name, options.format)?;
            (Some((folder.output_dir.clone(), name.to_string())), None)
        }
        ExportMode::Album => {
            let target = options.album.as_ref().map(|album| -> Result<_, String> {
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
                Ok((album_id, subgroup))
            }).transpose()?;
            (None, target)
        }
    };
    Ok(ValidatedOptions {
        file_name:options.file_name.as_ref().map(|s|s.trim().to_owned()),
        finish:options.finish.clone(),
        format:options.format,
        color_space:options.color_space,
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

fn validate_file_name(name:&str,format:ExportFormat)->Result<(),String>{
    if name.is_empty() || name.len()>240 || name.chars().any(|c|c.is_control()||"<>:\"/\\|?*".contains(c)) || (name.ends_with(' ') || name.ends_with('.')) {
        return Err("输出文件名无效".into());
    }
    let extension=Path::new(name).extension().and_then(|s|s.to_str()).unwrap_or("").to_ascii_lowercase();
    let matches=match format{ExportFormat::Jpeg=>["jpg","jpeg"].contains(&extension.as_str()),ExportFormat::Png=>extension=="png",ExportFormat::Tiff=>["tif","tiff"].contains(&extension.as_str()),ExportFormat::Pcraft=>extension=="pcraft"};
    if !matches{return Err("文件扩展名与输出格式不一致".into());}
    Ok(())
}

/// 校验后的选项（folder/album 二选一已定）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatedOptions {
    pub file_name:Option<String>,
    pub finish:super::output_finish::Options,
    pub format:ExportFormat,
    pub color_space:ExportColorSpace,
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
#[derive(Default)]
struct ExportControl { cancelled: bool, committing: bool, phase: String }
fn live_jobs() -> &'static Mutex<HashMap<i64, Arc<Mutex<ExportControl>>>> {
    static LIVE: OnceLock<Mutex<HashMap<i64, Arc<Mutex<ExportControl>>>>> = OnceLock::new();
    LIVE.get_or_init(|| Mutex::new(HashMap::new()))
}
pub fn cancel_job(id: i64) -> Result<(), String> {
    let jobs = live_jobs().lock().map_err(|_| "导出任务锁不可用")?;
    let control = jobs.get(&id).ok_or("导出任务已结束")?;
    let mut control = control.lock().map_err(|_| "导出任务锁不可用")?;
    if control.committing { return Err("正在完成文件登记，请稍候".into()); }
    control.cancelled = true;
    Ok(())
}
pub fn job_phase(id: i64) -> Option<String> {
    let control = live_jobs().lock().ok()?.get(&id)?.clone();
    let phase = control.lock().ok()?.phase.clone();
    Some(phase)
}
/// 当前活跃（本进程内正在跑）的导出任务 id 快照。
pub fn live_job_ids() -> Vec<i64> {
    live_jobs()
        .lock()
        .expect("export live jobs mutex poisoned")
        .keys()
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
pub(crate) struct LiveGuard(i64);

impl LiveGuard {
    /// 构造即登记（防忘记插入的旁路构造）。
    #[allow(dead_code)] // 仅供 run_export_job 与测试构造
    pub(crate) fn new(id: i64) -> Self {
        live_jobs()
            .lock()
            .expect("export live jobs mutex poisoned")
            .entry(id).or_insert_with(|| Arc::new(Mutex::new(ExportControl::default())));
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
    pub composition:Option<super::composition::Operation>,
    pub job_id: i64,
    pub asset_id: i64,
    /// 源资产行快照（启动时读取；文件路径以行为准）。
    pub asset: AssetRow,
    pub recipe: EditRecipe,
    pub options: ValidatedOptions,
    /// 源资产所属照片库 root（album 模式纯时间布局落位锚点；folder 模式
    /// 不用）。
    pub library_root: PathBuf,
}

/// 执行一个导出任务至终态（running → done|error + 事件）。阻塞；调用方
/// 决定线程（IPC 层 supervisor / 集成测试直调）。
pub fn run_export_job(db: Db, bus: &EventBus, request: ExportJobRequest) {
    let _live = LiveGuard::new(request.job_id);
    let control = live_jobs().lock().expect("export jobs lock").get(&request.job_id).cloned().expect("registered export");
    let cancelled = || control.lock().map(|c| c.cancelled).unwrap_or(true);
    let asset_id = request.asset_id;
    let progress = |phase: &str| {
        if let Ok(mut value) = control.lock() { value.phase = phase.to_owned(); }
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
    match execute(&db, &request, &progress, &cancelled, &|| {
        let mut value = control.lock().map_err(|_| "导出任务锁不可用")?;
        if value.cancelled { return Err("导出已取消".into()); }
        value.committing = true;
        Ok(())
    }) {
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
    cancelled: &dyn Fn() -> bool,
    begin_commit: &dyn Fn() -> Result<(), String>,
) -> Result<ExportResultDto, String> {
    let asset = &request.asset;
    let src = PathBuf::from(&asset.path);
    if !src.is_file() {
        return Err(format!("源文件不在盘: {}", asset.path));
    }

    if cancelled() { return Err("导出已取消".into()); }
    if let Some(operation)=&request.composition{operation.check_sources()?;}
    let (jpeg, width, height) = super::export_worker::render(&src, &request.recipe, &request.options,request.composition.as_ref(), progress, cancelled)?;
    if let Some(operation)=&request.composition{operation.check_sources()?;}
    begin_commit()?;
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
            let (album_id, subgroup) = request.options.album.clone()
                .map(|(id, subgroup)| (Some(id), subgroup)).unwrap_or((None, None));
            let mut output_asset=asset.clone();
            if let Some(operation)=&request.composition{output_asset.filename=format!("{}_{}.tif",Path::new(&asset.filename).file_stem().unwrap_or_default().to_string_lossy(),operation.kind);}
            let (dst, new_asset_id) = place_in_album(
                db,
                &jpeg,
                &output_asset,
                &request.options,
                &request.library_root,
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

pub(super) fn encode_output(src: &Path, recipe: &EditRecipe, options: &ValidatedOptions, progress: &dyn Fn(&str)) -> Result<(Vec<u8>, u32, u32), String> {
    let effective=super::groups::effective(recipe);let recipe=effective.as_ref().unwrap_or(recipe);
    // ① 渲染（解码/转正/旋转/裁剪/标注/只缩不放）
    progress("decode");
    let long_edge = options
        .long_edge
        .or(recipe.output.long_edge);
    let quality = options
        .quality
        .unwrap_or(recipe.output.quality)
        .clamp(1, 100);
    let source_head = meta::source_exif(&src).or_else(||read_head(&src,2*1024*1024));
    let meta_opts = MetaOptions {remove_gps:options.remove_gps,copyright:options.copyright.clone(),author:options.author.clone(),keywords:options.keywords.clone()};
    let (jpeg, width, height) =
        if options.format==ExportFormat::Pcraft {
            if recipe.renderer!=Some(super::recipe::RenderEngine::Photocraft){return Err("可编辑文档需要高级编辑处理方案".into());}
            let doc=super::photocraft::prepare_export_document(&src,&recipe,None,progress)?;progress("encode");
            (super::photocraft::native_document_with_metadata(&doc,source_head.as_deref(),&meta_opts)?,doc.size.width,doc.size.height)
        }else if options.format!=ExportFormat::Jpeg {
            if recipe.renderer!=Some(super::recipe::RenderEngine::Photocraft){return Err("此输出格式需要高级编辑处理方案".into());}
            let doc=super::photocraft::prepare_export_document_16(&src,&recipe,long_edge,progress,true)?;
            let doc=super::output_finish::apply(doc,&options.finish)?;
            let doc=super::brand_frame::apply(doc,recipe.brand_frame.as_ref(),long_edge)?;progress("encode");
            (super::photocraft::lossless_with_options(&doc,options.format.extension(),source_head.as_deref(),&meta_opts,options.color_space.profile(),&options.finish)?,doc.size.width,doc.size.height)
        }else{
            let doc=super::photocraft::prepare_export_document(src,recipe,long_edge,progress)?;
            let doc=super::output_finish::apply(doc,&options.finish)?;
            let doc=super::brand_frame::apply(doc,recipe.brand_frame.as_ref(),long_edge)?;progress("encode");
            (super::photocraft::jpeg_with_profile(&doc,quality,options.color_space.profile())?.0,doc.size.width,doc.size.height)
        };

    // ③ 元数据（源 EXIF 尽量保留 + removeGps + 版权/作者/关键词）
    let jpeg = if options.format==ExportFormat::Jpeg{meta::apply_metadata_for_color_image(jpeg, source_head.as_deref(), &meta_opts,width,height,options.color_space==ExportColorSpace::Srgb)}else{jpeg};

    Ok((jpeg, width, height))
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

/// Folder output uses exclusive publication after writing a unique sibling file.
fn write_folder(jpeg: &[u8], dir: &str, name: &str) -> Result<PathBuf, String> {
    let dst = Path::new(dir).join(name);
    if dst.exists() {
        return Err(format!("目标文件已存在（不覆盖）: {}", dst.display()));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("创建输出目录失败: {e}"))?;
    atomic_write(&dst, jpeg)?;
    Ok(dst)
}

/// album 模式落位 + 登记（导入引擎同款路径）。
#[allow(clippy::too_many_arguments)]
pub(super) fn place_in_album(
    db: &Db,
    jpeg: &[u8],
    source: &AssetRow,
    options: &ValidatedOptions,
    library_root: &Path,
    width: u32,
    height: u32,
    album_id: Option<i64>,
    subgroup: Option<String>,
) -> Result<(PathBuf, i64), String> {
    let stem = source
        .filename
        .rsplit_once('.')
        .map(|(s, _)| s.to_string())
        .unwrap_or_else(|| source.filename.clone());
    let captured_at = captured_or_fallback(source);
    // 与导入引擎同一纯时间布局公式（§三：`{拍摄年}/{拍摄月}/`，EXIF 缺失
    // 回退 mtime——captured_or_fallback 同口径）；相册/子组只是逻辑引用
    //（§一），不影响落位。同一资产的导出件与导入件同月份目录平铺。
    let name = options.file_name.clone().unwrap_or_else(||format!("{}_edit.{}", sanitize_component(&stem),options.format.extension()));
    let libraries=db.photos_library_list().map_err(|e|e.to_string())?;
    let template=libraries.iter().find(|library|Path::new(&library.root_path)==library_root).map(|library|library.organization_template.as_str()).unwrap_or(crate::db::organization::DEFAULT_ORGANIZATION_TEMPLATE);
    let relative=crate::db::organization::organized_rel(template,captured_at,&name,source.camera.as_deref())?;
    let intended=library_root.join(relative);let dir=intended.parent().ok_or("组织模板路径无效")?;let name=intended.file_name().ok_or("组织模板文件名无效")?.to_string_lossy().into_owned();
    let dst = unique_path(&dir, &name); // 导入引擎命名冲突规则（_1/_2）
    atomic_write(&dst, jpeg)?;

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
        // 导出件随源资产归属同一照片库（§一：library_id 静态归属属性）
        library_id: source.library_id.clone(),
        missing: 0,
        xmp_dirty: 0,
        volume_serial: None,
        file_id: None,
    };
    db.insert_asset_with_album(&row, album_id, subgroup.as_deref())
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

/// Unique sibling temporary file, published with an exclusive/no-clobber
/// operation. Drop cleans failed writes without touching another job's staging.
fn atomic_write(dst: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent=dst.parent().ok_or("目标目录无效")?;
    std::fs::create_dir_all(parent).map_err(|e|format!("创建目标目录失败: {e}"))?;
    let mut file=tempfile::Builder::new().prefix(".photo-export-").suffix(".part").tempfile_in(parent).map_err(|e|format!("创建导出暂存失败: {e}"))?;
    file.write_all(bytes).and_then(|_|file.as_file().sync_all()).map_err(|e|format!("写导出文件失败: {e}"))?;
    file.persist_noclobber(dst).map_err(|e|format!("导出文件落位失败（不会覆盖已有文件）: {}",e.error))?;
    Ok(())
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
#[cfg(test)]
mod atomic_output_tests {
    use super::*;
    #[test]
    fn native_document_task_keeps_source_resolution_and_rejects_album_registration(){
        let dir=tempfile::tempdir().unwrap();let db=Db::open(&dir.path().join("data.sqlite")).unwrap();let path=dir.path().join("source.tif");image::ImageBuffer::<image::Rgb<u16>,Vec<u16>>::from_pixel(120,80,image::Rgb([16384,32768,49152])).save(&path).unwrap();let original=std::fs::read(&path).unwrap();
        let asset:AssetRow=serde_json::from_value(serde_json::json!({"path":path.to_string_lossy(),"filename":"source.tif","size":original.len(),"mtime":"2026-09-01T00:00:00Z","xxhash":0,"kind":"photo","source":"test","createdAt":"2026-09-01T00:00:00Z"})).unwrap();db.insert_asset(&asset).unwrap();let id=db.asset_id_by_path(&path.to_string_lossy()).unwrap().unwrap();
        let mut options:ExportOptions=serde_json::from_value(serde_json::json!({"format":"pcraft","colorSpace":"displayP3","mode":"folder","folder":{"outputDir":dir.path().join("outputs"),"fileName":"edited.pcraft"},"longEdge":16,"quality":1})).unwrap();
        let recipe=super::super::recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","output":{"longEdge":16,"quality":1},"advanced":{"exposure":0.2}})).unwrap();
        let request=ExportJobRequest{composition:None,job_id:0,asset_id:id,asset:asset.clone(),recipe,options:validate_options(&db,&options).unwrap(),library_root:dir.path().join("photos")};let result=execute(&db,&request,&|_|{},&||false,&||Ok(())).unwrap();assert_eq!((result.width,result.height),(120,80));assert!(result.asset_id.is_none());
        let document=photocraft_io::import(&result.output_path,&std::fs::read(&result.output_path).unwrap()).unwrap().document;assert_eq!(document.depth,photocraft_engine::doc::SampleType::U16);assert!(!document.layers.is_empty());// 显影类(cameraRaw)调整烘焙进像素层(文档模型无非破坏性载体,且线性光数学与 Exposure 图层不等价——改图层会与预览/JPEG 分叉);曲线/蒙版等才追加图层,多图层语义由 photocraft::native_document_roundtrip 测试覆盖。assert_eq!(std::fs::read(&path).unwrap(),original);
        options.mode=ExportMode::Album;assert!(validate_options(&db,&options).unwrap_err().contains("不能作为照片"));
    }
    #[test]
    fn historical_output_options_default_to_jpeg_and_srgb_and_reject_unknown_spaces(){
        let mut value=serde_json::json!({"mode":"folder","folder":{"outputDir":"output","fileName":"photo.jpg"}});
        let old:ExportOptions=serde_json::from_value(value.clone()).unwrap();assert_eq!(old.format,ExportFormat::Jpeg);assert_eq!(old.color_space,ExportColorSpace::Srgb);
        value["colorSpace"]=serde_json::json!("adobeRgb");assert_eq!(serde_json::from_value::<ExportOptions>(value.clone()).unwrap().color_space,ExportColorSpace::AdobeRgb);
        value["colorSpace"]=serde_json::json!("unknown");assert!(serde_json::from_value::<ExportOptions>(value).is_err());
    }
    #[test]
    fn lossless_formats_execute_through_folder_and_album_destinations(){
        let dir=tempfile::tempdir().unwrap();let db=Db::open(&dir.path().join("data.sqlite")).unwrap();let path=dir.path().join("source.jpg");image::RgbImage::from_pixel(24,16,image::Rgb([80,120,160])).save(&path).unwrap();let original=std::fs::read(&path).unwrap();
        let asset:AssetRow=serde_json::from_value(serde_json::json!({"path":path.to_string_lossy(),"filename":"source.jpg","size":original.len(),"mtime":"2026-09-01T00:00:00Z","xxhash":0,"kind":"photo","source":"test","createdAt":"2026-09-01T00:00:00Z"})).unwrap();db.insert_asset(&asset).unwrap();let id=db.asset_id_by_path(&path.to_string_lossy()).unwrap().unwrap();let album=db.album_create("Output").unwrap();
        let recipe=super::super::recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","advanced":{"exposure":0.2}})).unwrap();
        for format in [ExportFormat::Png,ExportFormat::Tiff]{for mode in [ExportMode::Folder,ExportMode::Album]{
            let color_space=if format==ExportFormat::Png{ExportColorSpace::DisplayP3}else{ExportColorSpace::ProPhotoRgb};
            let options=ExportOptions{file_name:None,finish:Default::default(),format,color_space,mode,folder:Some(ExportFolderTarget{output_dir:dir.path().join("outputs").to_string_lossy().into_owned(),file_name:format!("edited.{}",format.extension())}),album:Some(ExportAlbumTarget{album_id:album.id.to_string(),subgroup:None}),long_edge:Some(12),quality:None,remove_gps:true,copyright:Some("Rights".into()),author:Some("Artist".into()),keywords:vec!["Photo".into()]};
            let request=ExportJobRequest{composition:None,job_id:0,asset_id:id,asset:asset.clone(),recipe:recipe.clone(),options:validate_options(&db,&options).unwrap(),library_root:dir.path().join("photos")};let result=execute(&db,&request,&|_|{},&||false,&||Ok(())).unwrap();assert!(result.output_path.ends_with(format.extension()));let decoded=image::open(&result.output_path).unwrap();assert_eq!((decoded.width(),decoded.height()),(12,8));assert!(matches!(decoded.color(),image::ColorType::Rgb16|image::ColorType::Rgba16));if mode==ExportMode::Album{assert!(result.asset_id.is_some());}
            let bytes=std::fs::read(&result.output_path).unwrap();let output=photocraft_io::import(&result.output_path,&bytes).unwrap();assert_eq!(output.document.icc_profile.unwrap(),color_space.profile().to_bytes());
        }}assert_eq!(std::fs::read(path).unwrap(),original);
    }
    #[test]
    fn export_refuses_existing_targets_and_never_truncates_foreign_staging(){
        let dir=tempfile::tempdir().unwrap();let dst=dir.path().join("photo.jpg");let part=dir.path().join("photo.part");
        std::fs::write(&dst,b"original photo").unwrap();std::fs::write(&part,b"other task").unwrap();
        assert!(atomic_write(&dst,b"edited photo").is_err());assert_eq!(std::fs::read(&dst).unwrap(),b"original photo");assert_eq!(std::fs::read(&part).unwrap(),b"other task");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(),2);
    }
    #[test]
    fn competing_exports_publish_exactly_one_complete_file(){
        let dir=tempfile::tempdir().unwrap();let dst=dir.path().join("photo.jpg");let barrier=std::sync::Arc::new(std::sync::Barrier::new(2));
        let jobs:Vec<_>=[b"first complete photo".as_slice(),b"second complete photo".as_slice()].into_iter().map(|bytes|{let dst=dst.clone();let barrier=barrier.clone();std::thread::spawn(move||{barrier.wait();atomic_write(&dst,bytes).is_ok()})}).collect();
        let successes=jobs.into_iter().filter_map(|job|job.join().ok()).filter(|ok|*ok).count();assert_eq!(successes,1);
        let actual=std::fs::read(dst).unwrap();assert!(actual==b"first complete photo"||actual==b"second complete photo");assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(),1);
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    #[test]
    fn queued_export_can_cancel_but_publication_cannot() {
        let id = -90001;
        let _live = LiveGuard::new(id);
        cancel_job(id).unwrap();
        let control = live_jobs().lock().unwrap().get(&id).unwrap().clone();
        assert!(control.lock().unwrap().cancelled);
        { let mut value = control.lock().unwrap(); value.cancelled = false; value.committing = true; value.phase = "register".into(); }
        assert!(cancel_job(id).is_err());
        assert_eq!(job_phase(id).as_deref(), Some("register"));
        assert!(!control.lock().unwrap().cancelled);
    }
}
