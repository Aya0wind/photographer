//! 编辑器临时预览会话；只读源文件，内存文档有数量/有效期限制，不写照片库存储。
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use base64::Engine;
use photocraft_engine::doc::Document;
use serde::Serialize;
use tauri::State;

use super::{photocraft, recipe, render};
use crate::ipc::{run_blocking, SharedState};

const PREVIEW_EDGE: u32 = 1600;
const MAX_SESSIONS: usize = 4;
const IDLE_TTL: Duration = Duration::from_secs(30 * 60);

struct Preview {
    document: Document,
    path: PathBuf,
    modified: Option<SystemTime>,
    length: u64,
    touched: Instant,
}

fn sessions() -> &'static Mutex<HashMap<String, Arc<Mutex<Preview>>>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, Arc<Mutex<Preview>>>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn validate_source(
    session_id: &str,
    expected_path: &std::path::Path,
) -> Result<(), String> {
    let preview = sessions()
        .lock()
        .map_err(|_| "预览会话锁不可用")?
        .get(session_id)
        .cloned()
        .ok_or("预览会话已关闭，请重新准备照片")?;
    let p = preview.lock().map_err(|_| "预览会话锁不可用")?;
    let meta = std::fs::metadata(expected_path).map_err(|e| format!("源照片不可用: {e}"))?;
    if p.path != expected_path || p.length != meta.len() || p.modified != meta.modified().ok() {
        return Err("源照片已改变，请重新准备预览后再生成成片".into());
    }
    Ok(())
}

fn jpeg_url(image: &image::RgbImage) -> Result<String, String> {
    let jpeg = render::encode_jpeg(image, 90)?;
    Ok(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(jpeg)
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewOpened {
    session_id: String,
    source_url: String,
    width: u32,
    height: u32,
}

#[tauri::command]
pub async fn edit_preview_open(
    state: State<'_, SharedState>,
    library_id: String,
    asset_id: String,
) -> Result<PreviewOpened, String> {
    run_blocking(state.inner().clone(), move |state| {
        let (_, asset) = super::project::resolve(state, &library_id, &asset_id)?;
        let path = PathBuf::from(asset.path);
        let metadata = std::fs::metadata(&path).map_err(|e| format!("源文件不可用: {e}"))?;
        let upright = render::decode_upright(&path)?;
        let (width, height) = upright.dimensions();
        let proxy = render::resize_long_edge(upright, Some(PREVIEW_EDGE));
        let source_url = jpeg_url(&proxy)?;
        let document = photocraft::document(&proxy)?;
        let session_id = uuid::Uuid::new_v4().to_string();
        let mut cache = sessions().lock().map_err(|_| "预览会话锁不可用")?;
        cache.retain(|_, value| {
            value
                .lock()
                .map(|p| p.touched.elapsed() < IDLE_TTL)
                .unwrap_or(false)
        });
        // 不淘汰仍在编辑的会话；前端关闭编辑器时主动释放。
        if cache.len() >= MAX_SESSIONS {
            return Err("同时打开的编辑预览过多，请先关闭其他编辑器".into());
        }
        cache.insert(
            session_id.clone(),
            Arc::new(Mutex::new(Preview {
                document,
                path,
                modified: metadata.modified().ok(),
                length: metadata.len(),
                touched: Instant::now(),
            })),
        );
        Ok(PreviewOpened {
            session_id,
            source_url,
            width,
            height,
        })
    })
    .await
}

#[tauri::command]
pub async fn edit_preview_render(
    state: State<'_, SharedState>,
    session_id: String,
    recipe: serde_json::Value,
) -> Result<String, String> {
    // 在进入后台前只校验 JSON；源图和合成工作都在阻塞线程。
    let recipe = recipe::parse_recipe(&recipe)?;
    run_blocking(state.inner().clone(), move |_| {
        let preview = sessions()
            .lock()
            .map_err(|_| "预览会话锁不可用")?
            .get(&session_id)
            .cloned()
            .ok_or("预览会话已关闭或过期，请重新打开编辑器")?;
        let base = {
            let mut p = preview.lock().map_err(|_| "预览会话锁不可用")?;
            if p.touched.elapsed() >= IDLE_TTL {
                return Err("预览会话已过期，请重新打开编辑器".into());
            }
            let meta = std::fs::metadata(&p.path).map_err(|e| format!("源文件不可用: {e}"))?;
            if meta.len() != p.length || meta.modified().ok() != p.modified {
                return Err("源文件已改变，请重新打开编辑器".into());
            }
            p.touched = Instant::now();
            p.document.clone()
        };
        // 几何和可交互标注由前端覆盖层显示；底图调整与导出共用同一上游算法。
        let doc = photocraft::adjusted_document(&base, &recipe)?;
        jpeg_url(&photocraft::composite(&doc)?)
    })
    .await
}

#[tauri::command]
pub fn edit_preview_close(session_id: String) -> Result<(), String> {
    sessions()
        .lock()
        .map_err(|_| "预览会话锁不可用")?
        .remove(&session_id);
    Ok(())
}
