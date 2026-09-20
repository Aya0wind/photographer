//! index 命令（真机修复 2026-09-19：语义回填触发链缺口）：手动触发索引与
//! 通道状态查询。根因：kick_semantic_if_ready 只挂在「下载完成 watcher /
//! 导入收尾」，存量资产在模型就位前导入则无人补触发——本模块提供 UI 主动
//! 触发入口（index_kick_now）与进度可视化数据（index_status）。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tauri::State;

use super::{run_blocking, SharedState};

/// 单通道任务状态计数（camelCase）。total = 可索引资产数（photo/raw，
/// 供 UI 显示 done/total 进度条；thumb/face 通道同口径）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexKindStatus {
    pub pending: u64,
    pub running: u64,
    pub done: u64,
    pub failed: u64,
    pub total: u64,
}

/// 全通道状态（index_status 载荷）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatusDto {
    pub thumb: IndexKindStatus,
    pub exif: IndexKindStatus,
    pub ai: IndexKindStatus,
    pub face: IndexKindStatus,
}

const KINDS: [&str; 4] = ["thumb", "exif", "ai", "face"];

/// 状态聚合核：index_tasks 按 (kind, state) 计数 + assets 可索引总数。
pub fn fetch_index_status(state: &super::AppState) -> Result<IndexStatusDto, String> {
    let db = super::active_library_db(state)?;
    let counts = db.index_task_state_counts().map_err(|e| e.to_string())?;
    let (total_assets, thumb_done, ai_done, face_done) =
        db.0.query_row(
            "SELECT COUNT(*), \
                    COUNT(CASE WHEN thumb_state != 0 THEN 1 END), \
                    COUNT(CASE WHEN ai_indexed_at IS NOT NULL THEN 1 END), \
                    COUNT(CASE WHEN face_indexed_at IS NOT NULL THEN 1 END) \
             FROM assets WHERE kind IN ('photo', 'raw')",
            [],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )
        .map_err(|e| e.to_string())?;
    let total_assets = total_assets as u64;
    let mut by_kind: HashMap<String, [u64; 4]> = HashMap::new();
    for (kind, state_name, count) in counts {
        let slot = match state_name.as_str() {
            "pending" => 0,
            "running" => 1,
            "done" => 2,
            _ => 3,
        };
        by_kind.entry(kind).or_default()[slot] += count;
    }
    let mut dto = IndexStatusDto::default();
    for (field, kind) in [
        (&mut dto.thumb, "thumb"),
        (&mut dto.exif, "exif"),
        (&mut dto.ai, "ai"),
        (&mut dto.face, "face"),
    ] {
        let slots = by_kind.get(kind).copied().unwrap_or_default();
        *field = IndexKindStatus {
            pending: slots[0],
            running: slots[1],
            done: slots[2],
            failed: slots[3],
            total: total_assets,
        };
    }
    // 缩略图完成态以 assets.thumb_state 为持久真值；旧版本可能清理 done
    // 任务行，直接数 index_tasks 会把已经生成的 119 张误报成 0。
    dto.thumb.done = thumb_done as u64;
    // AI 与人脸也以资产级完成标记为真值。按钮是否可点只取决于是否仍有
    // 未索引资产，不受历史任务行是否保留、失败行是否清理影响。
    dto.ai.done = ai_done as u64;
    dto.face.done = face_done as u64;
    // EXIF 在导入流读取首段时已同步提取并随资产一起落库，不会另建
    // index_tasks 行。把这些资产计为已完成，避免 UI 误报 0 / total；若未来
    // 存在显式 EXIF 任务，则仍以任务账为准。
    if dto.exif.pending == 0 && dto.exif.running == 0 && dto.exif.done == 0 && dto.exif.failed == 0
    {
        dto.exif.done = total_assets;
    }
    Ok(dto)
}

/// 手动触发核：thumb/exif → index::kick（全核跑待办）；ai → 语义回填
/// （模型未就绪 → 明确错误）；face → 人脸回填（同门槛）。幂等：任务生成
/// 侧自带去重（NOT EXISTS pending/running + ai_indexed_at/face_indexed_at
/// 时间账）。
pub fn fetch_index_kick_now(state: &super::AppState, kind: &str) -> Result<(), String> {
    let (enable_clip, enable_face) = {
        let settings = state.settings.lock().expect("settings mutex poisoned");
        (settings.ai.enable_clip, settings.ai.enable_face)
    };
    let library = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .active_library()
        .cloned()
        .ok_or("尚未创建库")?;
    let db_dir = std::path::PathBuf::from(&library.db_dir);
    let db = super::open_library_db(&db_dir)?;
    let supervisor = std::sync::Arc::clone(&state.supervisor);
    match kind {
        "thumb" | "exif" => {
            if kind == "thumb" {
                db.create_thumb_tasks_for_unindexed()
                    .map_err(|e| format!("创建缩略图任务失败: {e}"))?;
            }
            db.retry_failed_index_tasks(kind)
                .map_err(|e| format!("重试失败索引任务失败: {e}"))?;
            crate::index::kick(db_dir, &supervisor);
            Ok(())
        }
        "ai" => {
            if !state.ai.semantic_ready() {
                return Err("语义检索模型未下载，请先在设置中下载模型（siglip2-visual / siglip2-text / siglip2-tokenizer）".into());
            }
            if !enable_clip {
                return Err("语义索引未开启（设置 → AI → 语义检索）".into());
            }
            db.retry_failed_index_tasks("ai")
                .map_err(|e| format!("重试失败语义任务失败: {e}"))?;
            crate::ai::semantic::kick_semantic_if_ready(db_dir, &state.ai, &state.bus, &supervisor);
            Ok(())
        }
        "face" => {
            if !state.ai.face_models_ready() {
                return Err("人脸识别模型未下载，请先在设置中下载模型（scrfd / arcface）".into());
            }
            if !enable_face {
                return Err("人脸识别未开启（设置 → AI → 人脸识别）".into());
            }
            db.retry_failed_index_tasks("face")
                .map_err(|e| format!("重试失败人脸任务失败: {e}"))?;
            crate::ai::face::kick_face_if_ready(db_dir, &state.ai, &state.bus, &supervisor);
            Ok(())
        }
        other => Err(format!("未知索引通道: {other}（可选 {KINDS:?}）")),
    }
}

/// 手动触发索引（kind = "thumb" | "exif" | "ai" | "face"；幂等）。
#[tauri::command]
pub async fn index_kick_now(state: State<'_, SharedState>, kind: String) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| fetch_index_kick_now(state, &kind)).await
}

// ---------------------------------------------------------------------------
// 索引重建（用户 2026-09-20：参数可配 + 改配置重建按钮）
// ---------------------------------------------------------------------------

/// 重建通道名（IPC kind）。
const REBUILD_KINDS: [&str; 4] = ["semantic", "face", "thumb", "exif"];

/// 单通道重建核（同步、可测）：清理该通道全部持久化产物 + 时间账/任务账
/// 重排。返回重排后的待办数。不 kick worker（调用方决定：手动 IPC 后台
/// kick / 自动重建逐通道 kick）。
pub fn rebuild_channel_core(
    db: &crate::db::Db,
    db_dir: &std::path::Path,
    kind: &str,
) -> Result<u64, String> {
    match kind {
        "semantic" => {
            // 向量真值（usearch 文件）+ 时间账 + 任务账；池内 mmap 旧视图逐出
            let vectors = db_dir.join("vectors.usearch");
            if vectors.exists() {
                std::fs::remove_file(&vectors).map_err(|e| format!("删除向量索引失败: {e}"))?;
            }
            crate::ai::semantic::invalidate_index(db_dir);
            db.0
                .execute_batch(
                    "UPDATE assets SET ai_indexed_at = NULL;                      DELETE FROM index_tasks WHERE kind = 'ai';",
                )
                .map_err(|e| e.to_string())?;
            db.create_ai_tasks_for_unindexed()
                .map_err(|e| format!("重排语义任务失败: {e}"))
        }
        "face" => {
            // faces/people 全清（同 ai_face_data_clear）+ 时间账 + 任务账
            db.clear_face_data().map_err(|e| e.to_string())?;
            crate::ai::face::invalidate_cluster_cache(db_dir);
            db.create_face_tasks_for_unindexed()
                .map_err(|e| format!("重排人脸任务失败: {e}"))
        }
        "thumb" => {
            let thumbs = db_dir.join("thumbs");
            if thumbs.exists() {
                std::fs::remove_dir_all(&thumbs).map_err(|e| format!("删除缩略图目录失败: {e}"))?;
            }
            db.reset_thumb_states().map_err(|e| e.to_string())?;
            db.requeue_thumb_tasks_for_all()
                .map_err(|e| format!("重排缩略图任务失败: {e}"))
        }
        "exif" => {
            db.clear_exif_columns().map_err(|e| e.to_string())?;
            db.requeue_exif_tasks_for_all()
                .map_err(|e| format!("重排 EXIF 任务失败: {e}"))
        }
        other => Err(format!("未知重建通道: {other}（可选 {REBUILD_KINDS:?}）")),
    }
}

/// 重建后 kick 对应 worker（通道→入口映射）。
fn kick_rebuilt_channel(
    state: &super::AppState,
    db_dir: std::path::PathBuf,
    kind: &str,
    pending: u64,
) {
    let bus = state.bus.clone();
    if pending > 0 {
        bus.publish(crate::events::AppEvent::IndexTaskResumed { pending });
    }
    let supervisor = std::sync::Arc::clone(&state.supervisor);
    match kind {
        "semantic" => {
            crate::ai::semantic::kick_semantic_if_ready(db_dir, &state.ai, &bus, &supervisor)
        }
        "face" => crate::ai::face::kick_face_if_ready(db_dir, &state.ai, &bus, &supervisor),
        _ => crate::index::kick(db_dir, &supervisor),
    }
}

/// 重建门槛校验（同步快路径）：语义/人脸需模型就绪 + 开关开启；
/// thumb/exif 无门槛。通过后调用方再后台执行清理+重排+kick。
pub fn rebuild_gates(state: &super::AppState, kind: &str) -> Result<(), String> {
    let (enable_clip, enable_face) = {
        let settings = state.settings.lock().expect("settings mutex poisoned");
        (settings.ai.enable_clip, settings.ai.enable_face)
    };
    match kind {
        "semantic" => {
            if !state.ai.semantic_ready() {
                return Err("语义检索模型未下载，请先在设置中下载模型（siglip2-visual / siglip2-text / siglip2-tokenizer）".into());
            }
            if !enable_clip {
                return Err("语义索引未开启（设置 → AI → 语义检索）".into());
            }
        }
        "face" => {
            if !state.ai.face_models_ready() {
                return Err("人脸识别模型未下载，请先在设置中下载模型（scrfd / arcface）".into());
            }
            if !enable_face {
                return Err("人脸识别未开启（设置 → AI → 人脸识别）".into());
            }
        }
        "thumb" | "exif" => {}
        other => return Err(format!("未知重建通道: {other}（可选 {REBUILD_KINDS:?}）")),
    }
    Ok(())
}

/// 活动库 dbDir（重建后台任务用；无库明确报错）。
pub fn active_db_dir(state: &super::AppState) -> Result<std::path::PathBuf, String> {
    state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .active_library()
        .map(|lib| std::path::PathBuf::from(&lib.db_dir))
        .ok_or_else(|| "尚未创建库".to_string())
}

/// 重建执行体（supervisor 线程 / 测试直调）：开库 → 清理+重排 → 事件 →
/// kick 对应 worker。
pub fn run_rebuild(state: &super::AppState, db_dir: &std::path::Path, kind: &str) {
    let Ok(db) = super::open_library_db(db_dir) else {
        eprintln!("[rebuild] 库不可用: {}", db_dir.display());
        return;
    };
    match rebuild_channel_core(&db, db_dir, kind) {
        Ok(pending) => kick_rebuilt_channel(state, db_dir.to_path_buf(), kind, pending),
        Err(error) => {
            eprintln!("[rebuild] {kind} 重建失败: {error}");
            state.bus.publish(crate::events::AppEvent::AppError {
                level: "error".into(),
                message: format!("索引重建失败（{kind}）：{error}"),
                recoverable: true,
            });
        }
    }
}

/// 重建索引（kind = "semantic" | "face" | "thumb" | "exif"）。清理该通道
/// 全部产物并重排任务，后台执行（IPC 立即返回；IndexTaskResumed 事件驱动
/// 任务抽屉即时反映）。
#[tauri::command]
pub async fn index_rebuild(state: State<'_, SharedState>, kind: String) -> Result<(), String> {
    let shared = state.inner().clone();
    // 门槛同步校验（快，立即把「模型未就绪/开关未开」回报给 UI）
    let kind_for_gates = kind.clone();
    run_blocking(shared.clone(), move |state| {
        rebuild_gates(state, &kind_for_gates)
    })
    .await?;
    let db_dir = run_blocking(shared.clone(), active_db_dir).await?;
    // 清理+重排+kick 后台执行（删大目录/批量 UPDATE 不阻塞 IPC）
    let supervisor = std::sync::Arc::clone(&shared.supervisor);
    supervisor.spawn("index", format!("rebuild-{kind}"), move |_| {
        run_rebuild(&shared, &db_dir, &kind);
    });
    Ok(())
}

/// 索引通道状态（各通道 pending/running/done/failed + 可索引资产总数）。
#[tauri::command]
pub async fn index_status(state: State<'_, SharedState>) -> Result<IndexStatusDto, String> {
    let shared = state.inner().clone();
    run_blocking(shared, fetch_index_status).await
}

// ---------------------------------------------------------------------------
// AI 索引参数指纹（settings.ai ↔ dbDir/index-params.marker；改参数自动重建）
// ---------------------------------------------------------------------------

/// 语义通道指纹（版本 + embed_input_size——嵌入随输入尺寸变化）。
pub fn semantic_params_fingerprint(ai: &crate::settings::AiSettings) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for part in [
        ai.index_params_version as u64,
        u64::from(ai.embed_input_size),
    ] {
        hash ^= part;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// 人脸通道指纹（版本 + 检测门槛 + 聚类阈值；f32 用 to_bits 保精确比较）。
pub fn face_params_fingerprint(ai: &crate::settings::AiSettings) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for part in [
        ai.index_params_version as u64,
        u64::from(ai.embed_input_size), // 版本语义变更兜底参与
        ai.face_detect_threshold.to_bits() as u64,
        ai.face_cluster_threshold.to_bits() as u64,
    ] {
        hash ^= part;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// 参数指纹落盘名（dbDir 下）。
const PARAMS_MARKER: &str = "index-params.marker";

/// 启动 / settings_set 后调用：比对 settings.ai 参数指纹与库标记，不一致
/// 的通道自动重建（一次性；模型未就绪/开关未开的通道**不写标记**，下一轮
/// 满足条件时再重建）。手动重建按钮走 [`index_rebuild`]，与此互不冲突
/// （重建本身幂等：清产物 → 重排）。
pub fn check_params_and_rebuild(
    state: &super::AppState,
    db_dir: &std::path::Path,
    ai: &crate::settings::AiSettings,
) {
    let (want_sem, want_face) = (semantic_params_fingerprint(ai), face_params_fingerprint(ai));
    let marker = db_dir.join(PARAMS_MARKER);
    let content = std::fs::read_to_string(&marker).unwrap_or_default();
    let stored_sem = content
        .lines()
        .find_map(|l| l.strip_prefix("semantic="))
        .and_then(|v| v.parse::<u64>().ok());
    let stored_face = content
        .lines()
        .find_map(|l| l.strip_prefix("face="))
        .and_then(|v| v.parse::<u64>().ok());

    let mut rebuilt_sem = false;
    if stored_sem != Some(want_sem) && ai.enable_clip && state.ai.semantic_ready() {
        eprintln!("[params] 语义参数变更（{stored_sem:?} → {want_sem}），自动重建");
        run_rebuild(state, db_dir, "semantic");
        rebuilt_sem = true;
    }
    let mut rebuilt_face = false;
    if stored_face != Some(want_face) && ai.enable_face && state.ai.face_models_ready() {
        eprintln!("[params] 人脸参数变更（{stored_face:?} → {want_face}），自动重建");
        run_rebuild(state, db_dir, "face");
        rebuilt_face = true;
    }
    // 只把「已重建或已对齐」的通道写回标记；被门槛挡下的通道保留旧指纹
    let sem_line = format!(
        "semantic={}",
        if rebuilt_sem || stored_sem == Some(want_sem) {
            want_sem
        } else {
            stored_sem.unwrap_or(want_sem)
        }
    );
    let face_line = format!(
        "face={}",
        if rebuilt_face || stored_face == Some(want_face) {
            want_face
        } else {
            stored_face.unwrap_or(want_face)
        }
    );
    let _ = std::fs::create_dir_all(db_dir);
    let _ = std::fs::write(
        &marker,
        format!(
            "{sem_line}
{face_line}
"
        ),
    );
}
