//! thumb 命令：`thumb_get(path, size)`。
//!
//! 铁律（2026-09-18）：解码是 CPU 密集 + 磁盘 IO（A7R5 的 30-60MB JPG）→
//! async + spawn_blocking 后台执行。返回缓存文件绝对路径（前端经 asset
//! 协议加载该文件）；无法生成（RAW/视频/读取失败/无库/超时）返回 null。
//! 核心逻辑在 `crate::thumbs`（独立可测，不依赖 AppState）。

use std::path::{Path, PathBuf};

use tauri::State;

use super::{run_blocking, SharedState};

/// 取缩略图缓存文件路径（无法生成返回 null）。
/// size 就近归一到 256/512 两档（v1 前端用 256）；命中缓存不重解码；
/// 同 path+size 并发请求合并为一次解码；缓存落库 dbDir/thumbs。
/// 注：async+State 命令须返回 Result；`Ok(None)` 序列化为 null——错误路径
/// 一律归一为 null（契约 `String | null`，不向前端抛 reject）。
#[tauri::command]
pub async fn thumb_get(
    state: State<'_, SharedState>,
    path: String,
    size: u16,
) -> Result<Option<String>, String> {
    let shared = state.inner().clone();
    let thumb = run_blocking(shared, move |state| {
        let library = state
            .settings
            .lock()
            .expect("settings mutex poisoned")
            .active_library()
            .cloned();
        let thumb: Option<String> = library.and_then(|lib| {
            let db_dir = PathBuf::from(lib.db_dir);
            crate::thumbs::thumb_file(&db_dir, Path::new(&path), size)
        });
        Ok::<Option<String>, String>(thumb)
    })
    .await
    .ok()
    .flatten();
    Ok(thumb)
}
