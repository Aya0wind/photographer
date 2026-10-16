//! Non-AI multi-photo jobs, using upstream registration/HDR/blending in a killable child.
use photocraft_engine::{doc::Document, Session};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub path: PathBuf,
    pub length: u64,
    pub modified: std::time::SystemTime,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub inputs: Vec<Input>,
    pub kind: String,
    pub layout: String,
    pub bracket_size: usize,
    pub remove_ghosts: bool,
    pub preview: bool,
}
impl Operation {
    pub(super) fn check_sources(&self) -> Result<(), String> {
        for input in &self.inputs {
            let m = std::fs::metadata(&input.path).map_err(|e| format!("合成原片不可用：{e}"))?;
            if m.len() != input.length || m.modified().ok() != Some(input.modified) {
                return Err("合成期间原片已改变，请重试".into());
            }
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Start {
    pub asset_ids: Vec<i64>,
    pub library_id: String,
    pub kind: String,
    pub layout: String,
    pub bracket_size: usize,
    pub remove_ghosts: bool,
    pub preview: bool,
    pub long_edge: Option<u32>,
    pub album: Option<super::export::ExportAlbumTarget>,
}
#[tauri::command]
pub async fn edit_composition_start(
    state: tauri::State<'_, crate::ipc::SharedState>,
    request: Start,
) -> Result<super::export::ExportTaskDto, String> {
    crate::ipc::run_blocking(state.inner().clone(), move |state| {
        let mut unique = std::collections::HashSet::new();
        if !(2..=12).contains(&request.asset_ids.len())
            || request.asset_ids.iter().any(|id| !unique.insert(*id))
        {
            return Err("请选择 2–12 张不同照片，按拍摄顺序排列".into());
        }
        if !["hdr", "panorama", "hdrPanorama"].contains(&request.kind.as_str())
            || photocraft_algo::panorama::Layout::parse(&request.layout).is_none()
        {
            return Err("合成类型或投影无效".into());
        }
        if request.kind == "hdrPanorama"
            && (!(2..=6).contains(&request.bracket_size)
                || request.asset_ids.len() % request.bracket_size != 0
                || request.asset_ids.len() / request.bracket_size < 2)
        {
            return Err("HDR 全景至少两组，每组 2–6 张包围曝光，按组连续排列".into());
        }
        if request
            .long_edge
            .is_some_and(|v| !(256..=30000).contains(&v))
        {
            return Err("合成长边需为 256–30000 像素，或保留原尺寸".into());
        }
        let mut inputs = Vec::new();
        for id in &request.asset_ids {
            let (_, asset) = super::project::resolve(state, &request.library_id, &id.to_string())?;
            if !matches!(
                asset.kind,
                crate::events::AssetKind::Photo | crate::events::AssetKind::Raw
            ) {
                return Err("合成仅接受照片或 RAW".into());
            }
            let path = PathBuf::from(asset.path);
            let meta = std::fs::metadata(&path).map_err(|e| format!("合成原片不可用：{e}"))?;
            inputs.push(Input {
                path,
                length: meta.len(),
                modified: meta.modified().map_err(|e| e.to_string())?,
            });
        }
        let operation = Operation {
            inputs,
            kind: request.kind,
            layout: request.layout,
            bracket_size: request.bracket_size,
            remove_ghosts: request.remove_ghosts,
            preview: request.preview,
        };
        let preview_dir = crate::ipc::app_database_dir(state)?.join("merge-previews");
        clean_previews(&preview_dir);
        let options = super::export::ExportOptions {
            file_name: None,
            finish: Default::default(),
            format: if request.preview {
                super::export::ExportFormat::Png
            } else {
                super::export::ExportFormat::Tiff
            },
            color_space: Default::default(),
            mode: if request.preview {
                super::export::ExportMode::Folder
            } else {
                super::export::ExportMode::Album
            },
            folder: request.preview.then(|| super::export::ExportFolderTarget {
                output_dir: preview_dir.to_string_lossy().into_owned(),
                file_name: format!("{}.png", uuid::Uuid::new_v4()),
            }),
            album: request.album,
            long_edge: if request.preview {
                Some(1600)
            } else {
                request.long_edge
            },
            quality: None,
            remove_gps: false,
            copyright: None,
            author: None,
            keywords: vec![],
        };
        let recipe = json!({"version":1,"renderer":"photocraft"});
        super::ipc::fetch_export_operation(
            state,
            request.asset_ids[0],
            &recipe,
            &options,
            Some(&request.library_id),
            Some(operation),
        )
    })
    .await
}
/// Load originals in a common display transfer without applying creative base curves.
fn load(path: &Path, edge: Option<u32>) -> Result<Document, String> {
    let source = if super::rawlab::is_raw_path(path) {
        super::rawlab::open(path, edge, &std::sync::atomic::AtomicBool::new(false))?
    } else {
        super::source::open(path)?
    };
    let resized = super::photocraft::resize(&source.document, edge)?;
    let image = photocraft_io::document_to_image(&resized, &mut Vec::new())
        .map_err(|e| e.to_string())?
        .convert(
            photocraft_codecs::ChannelLayout::Rgba,
            photocraft_codecs::SampleType::F32,
        );
    let mut doc = super::source::from_image("Merge input", &image)?;
    photocraft_engine::color_cmds::convert_document(
        &mut doc,
        photocraft_cms::Builtin::Srgb.profile(),
        photocraft_cms::Intent::RelativeColorimetric,
        true,
    )
    .map_err(|e| e.to_string())?;
    if let Some(exif) = super::meta::source_exif(path) {
        doc.metadata.exif = Some(std::sync::Arc::new(exif));
    }
    Ok(doc)
}
fn execute(
    session: &mut Session,
    command: &str,
    params: serde_json::Value,
    progress: &dyn Fn(&str),
) -> Result<serde_json::Value, String> {
    use photocraft_engine::jobs::Started;
    match session.start(command, params).map_err(|e| e.to_string())? {
        Started::Done(result) => Ok(result),
        Started::Job(id) => loop {
            session.poll_jobs();
            let info = session.job(id).ok_or("合成作业状态丢失")?;
            if info.state == "done" {
                return info.result.ok_or("合成未返回结果".into());
            }
            if info.state == "failed" || info.state == "cancelled" {
                return Err(info.error.unwrap_or_else(|| "合成已取消".into()));
            }
            progress(if info.message.contains("warp") {
                "mergeWarp"
            } else if info.message.contains("blend") {
                "mergeBlend"
            } else {
                "mergeAlign"
            });
            std::thread::sleep(std::time::Duration::from_millis(120));
        },
    }
}
fn hdr(docs: Vec<Document>, ghosts: bool, progress: &dyn Fn(&str)) -> Result<Document, String> {
    let mut session = Session::new();
    for doc in docs {
        session.add_document(doc, None);
    }
    progress("mergeHdr");
    execute(
        &mut session,
        "file.automate.mergeToHdrPro",
        json!({"useOpenDocuments":true,"align":true,"removeGhosts":ghosts,"mode":"32"}),
        progress,
    )?;
    super::photocraft::active_document(&session)
}
fn panorama(
    docs: Vec<Document>,
    layout: &str,
    progress: &dyn Fn(&str),
) -> Result<Document, String> {
    progress("mergeAlign");
    check_panorama_canvas(&docs, layout)?;
    let mut session = Session::new();
    for doc in docs {
        session.add_document(doc, None);
    }
    let result = execute(
        &mut session,
        "file.automate.photomerge",
        json!({"useOpenDocuments":true,"layout":layout,"blend":true,"contentAwareFill":false}),
        progress,
    )?;
    if result["failed"]
        .as_array()
        .is_some_and(|rows| !rows.is_empty())
    {
        return Err("部分照片无法对齐，未生成不完整全景；请检查重叠区域、顺序或投影方式".into());
    }
    super::photocraft::active_document(&session)
}
/// Match upstream's 1200 px registration pass, but reject oversized canvases before
/// Photomerge allocates warped images. The pinned engine exposes alignment, not a canvas limit.
fn check_panorama_canvas(docs: &[Document], layout: &str) -> Result<(), String> {
    use photocraft_algo::panorama::{self, AlignOptions, Layout};
    let side = docs
        .iter()
        .map(|d| d.size.width.max(d.size.height) as usize)
        .max()
        .unwrap_or(1);
    let k = side.div_ceil(1200).max(1);
    let mut prepared = Vec::new();
    for doc in docs {
        let (w, h) = (doc.size.width as usize, doc.size.height as usize);
        let (sw, sh) = (w.div_ceil(k).max(1), h.div_ceil(k).max(1));
        let mut luma = vec![0.0f32; sw * sh];
        let mut alpha = vec![0.0f32; sw * sh];
        let mut count = vec![0.0f32; sw * sh];
        let flat = photocraft_compose::flatten(doc);
        for (index, pixel) in flat.px.iter().enumerate() {
            let i = (index / w / k) * sw + index % w / k;
            luma[i] += (0.2126 * pixel[0] + 0.7152 * pixel[1] + 0.0722 * pixel[2]) * pixel[3];
            alpha[i] += pixel[3];
            count[i] += 1.0;
        }
        for i in 0..luma.len() {
            luma[i] = if alpha[i] > 0.0 {
                luma[i] / alpha[i]
            } else {
                0.0
            };
            alpha[i] /= count[i].max(1.0);
        }
        let valid = alpha
            .iter()
            .any(|a| *a < 0.5)
            .then(|| alpha.iter().map(|a| *a > 0.5).collect());
        prepared.push((sw, sh, luma, valid));
    }
    let features = panorama::detect_many(&prepared, 800);
    let matches = panorama::match_pairs(&features);
    let mut focals: Vec<_> = docs
        .iter()
        .filter_map(|doc| doc.metadata.exif.as_ref())
        .filter_map(|exif| photocraft_algo::exif::read(exif).focal_length_35mm)
        .collect();
    focals.sort_by(f64::total_cmp);
    let focal = focals
        .get(focals.len() / 2)
        .map(|f| f / 36.0 * side as f64 / k as f64);
    let alignment = panorama::align(
        &features,
        &matches,
        &AlignOptions {
            layout: Layout::parse(layout).ok_or("无效的全景投影")?,
            focal,
            reference: None,
            geometric: false,
        },
    )
    .ok_or("无法找到足够的重叠细节，请调整照片或投影")?;
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (doc, placement) in docs.iter().zip(&alignment.placements) {
        let placement = placement
            .as_ref()
            .ok_or("部分照片无法对齐，请检查重叠区域或顺序")?
            .scaled(k as f64);
        let b = placement.bounds(doc.size.width as f64, doc.size.height as f64);
        if b.iter().any(|v| !v.is_finite()) {
            return Err("全景边界无效，请尝试其他投影".into());
        }
        x0 = x0.min(b[0]);
        y0 = y0.min(b[1]);
        x1 = x1.max(b[2]);
        y1 = y1.max(b[3]);
    }
    let (w, h) = ((x1 - x0).ceil(), (y1 - y0).ceil());
    if w <= 0.0 || h <= 0.0 || w > 30000.0 || h > 30000.0 || w * h > 128_000_000.0 {
        return Err("全景画布过大，请减小输出长边或尝试其他投影".into());
    }
    Ok(())
}
fn tone_map(doc: Document) -> Result<Document, String> {
    let mut flat = photocraft_compose::flatten(&doc);
    let (w, h) = (doc.size.width, doc.size.height);
    photocraft_algo::hdr::tone_map(
        &mut flat.px,
        w as usize,
        h as usize,
        &photocraft_algo::hdr::ToneMethod::HighlightCompression,
    );
    let mut bytes = Vec::with_capacity(flat.px.len() * 16);
    for p in flat.px {
        for v in &p {
            bytes.extend(v.to_ne_bytes());
        }
    }
    let image = photocraft_codecs::Image::from_raw(
        w,
        h,
        photocraft_codecs::ChannelLayout::Rgba,
        photocraft_codecs::SampleType::F32,
        bytes,
    )
    .map_err(|e| e.to_string())?;
    let mut doc = super::source::from_image("HDR composite", &image)?;
    doc.icc_profile = Some(photocraft_cms::Builtin::Srgb.profile().to_bytes());
    Ok(doc)
}
pub(super) fn worker(
    operation: &Operation,
    options: &super::export::ValidatedOptions,
    progress: &dyn Fn(&str),
) -> Result<(Vec<u8>, u32, u32), String> {
    operation.check_sources()?;
    let mut docs = Vec::new();
    let mut pixels = 0u64;
    for input in &operation.inputs {
        progress("mergeDecode");
        let doc = load(&input.path, options.long_edge)?;
        pixels += u64::from(doc.size.width) * u64::from(doc.size.height);
        if pixels > 192_000_000 {
            return Err("输入照片合计超过 1.92 亿像素，请设置较小的输出长边".into());
        }
        docs.push(doc);
    }
    let first_metadata = docs[0].metadata.clone();
    let doc = match operation.kind.as_str() {
        "hdr" => tone_map(hdr(docs, operation.remove_ghosts, progress)?)?,
        "panorama" => panorama(docs, &operation.layout, progress)?,
        _ => {
            let mut merged = Vec::new();
            let mut input = docs.into_iter();
            loop {
                let group: Vec<_> = input.by_ref().take(operation.bracket_size).collect();
                if group.is_empty() {
                    break;
                }
                let doc = hdr(group, operation.remove_ghosts, progress)?;
                merged.push(doc);
            }
            let doc = panorama(merged, &operation.layout, progress)?;
            tone_map(doc)?
        }
    };
    let mut doc = super::photocraft::resize(&doc, options.long_edge)?;
    doc.metadata = first_metadata;
    if u64::from(doc.size.width) * u64::from(doc.size.height) > 128_000_000 {
        return Err("全景输出超过 1.28 亿像素，请减小输出尺寸".into());
    }
    operation.check_sources()?;
    progress("encode");
    let bytes = super::photocraft::lossless_with_options(
        &doc,
        options.format.extension(),
        doc.metadata.exif.as_deref().map(Vec::as_slice),
        &super::meta::MetaOptions::default(),
        options.color_space.profile(),
        &options.finish,
    )?;
    Ok((bytes, doc.size.width, doc.size.height))
}

fn clean_previews(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            (m.is_file() && e.path().extension().is_some_and(|s| s == "png")).then_some((
                m.modified().ok()?,
                m.len(),
                e.path(),
            ))
        })
        .collect();
    files.sort_by_key(|(time, _, _)| *time);
    let mut bytes: u64 = files.iter().map(|(_, size, _)| size).sum();
    for (time, size, path) in files {
        if bytes <= 64 * 1024 * 1024
            && time
                .elapsed()
                .is_ok_and(|age| age < std::time::Duration::from_secs(86400))
        {
            continue;
        }
        match std::fs::remove_file(path) {
            Ok(()) => bytes = bytes.saturating_sub(size),
            Err(error) => crate::devices::diagnostics::record(format!(
                "merge preview cleanup failed: {error}"
            )),
        }
    }
}
