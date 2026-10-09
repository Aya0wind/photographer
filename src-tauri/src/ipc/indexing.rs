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
    pub image: IndexKindStatus,
    pub selection_ready: bool,
    pub thumb: IndexKindStatus,
    pub exif: IndexKindStatus,
    pub ai: IndexKindStatus,
    pub face: IndexKindStatus,
    pub phash: IndexKindStatus,
    pub hash: IndexKindStatus,
    pub eyes: IndexKindStatus,
    pub blur: IndexKindStatus,
}

const KINDS: [&str; 9] = [
    "image", "thumb", "exif", "ai", "face", "phash", "hash", "eyes", "blur",
];

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
        (&mut dto.phash, "phash"),
        (&mut dto.hash, "hash"),
        (&mut dto.eyes, "eyes"),
        (&mut dto.blur, "blur"),
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
    if dto.exif.pending == 0 && dto.exif.running == 0 {
        dto.exif.done = total_assets;
    }
    dto.image = image_index_status(&db)?;
    dto.selection_ready = state.ai.selection_eyes_ready();
    Ok(dto)
}

/// Count each photo once; completion requires every component. EXIF with no
/// task is the synchronous import path. Eyes/blur without task/result are pending.
fn image_index_status(db: &crate::db::Db) -> Result<IndexKindStatus, String> {
    let mut result = IndexKindStatus::default();
    let mut stmt = db.0.prepare("WITH parts AS (
        SELECT a.id, a.thumb_state,
          MAX(CASE WHEN t.state='running' THEN 1 ELSE 0 END) AS running,
          MAX(CASE WHEN t.state='failed' THEN 1 ELSE 0 END) AS failed,
          MAX(CASE WHEN t.state='pending' THEN 1 ELSE 0 END) AS pending,
          MAX(CASE WHEN t.kind='eyes' AND t.state='done' THEN 1 ELSE 0 END) AS eyes_done,
          MAX(CASE WHEN t.kind='blur' AND t.state='done' THEN 1 ELSE 0 END) AS blur_done
        FROM assets a LEFT JOIN index_tasks t ON t.asset_id=a.id AND t.kind IN ('thumb','exif','eyes','blur')
        WHERE a.kind IN ('photo','raw') GROUP BY a.id
      ) SELECT CASE WHEN running=1 THEN 'running' WHEN failed=1 THEN 'failed'
          WHEN pending=1 OR thumb_state=0 OR eyes_done=0 OR blur_done=0 THEN 'pending'
          ELSE 'done' END, COUNT(*) FROM parts GROUP BY 1").map_err(|e|e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (status, count) = row.map_err(|e| e.to_string())?;
        match status.as_str() {
            "running" => result.running = count,
            "failed" => result.failed = count,
            "pending" => result.pending = count,
            _ => result.done = count,
        }
        result.total += count;
    }
    Ok(result)
}

/// 手动触发核：thumb/exif → index::kick（全核跑待办）；ai → 语义回填
/// （模型未就绪 → 明确错误）；face → 人脸回填（同门槛）。幂等：任务生成
/// 侧自带去重（NOT EXISTS pending/running + ai_indexed_at/face_indexed_at
/// 时间账）。
pub fn fetch_index_kick_now(state: &super::AppState, kind: &str) -> Result<(), String> {
    super::ensure_no_import_running(state)?;
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
        "image" => {
            state.supervisor.resume_kind("index");
            db.create_thumb_tasks_for_unindexed()
                .map_err(|e| e.to_string())?;
            db.create_eyes_tasks_for_unindexed()
                .map_err(|e| e.to_string())?;
            db.create_blur_tasks_for_unindexed()
                .map_err(|e| e.to_string())?;
            for component in ["thumb", "exif", "eyes", "blur"] {
                db.retry_failed_index_tasks(component)
                    .map_err(|e| e.to_string())?;
            }
            crate::index::kick(db_dir.clone(), &supervisor);
            crate::ai::selection::kick_eyes_if_ready(db_dir, &state.ai, &state.bus, &supervisor);
            Ok(())
        }
        "thumb" | "exif" | "phash" | "hash" | "eyes" | "blur" => {
            match kind {
                "thumb" => {
                    db.create_thumb_tasks_for_unindexed()
                        .map_err(|e| format!("创建缩略图任务失败: {e}"))?;
                }
                "phash" => {
                    db.create_phash_tasks_for_unindexed()
                        .map_err(|e| format!("创建 pHash 任务失败: {e}"))?;
                }
                // 0021 选片分析通道：eyes 依赖闭眼模型 facemesh（未下载 →
                // 明确错误，任务账可先建档）；blur 无模型依赖始终可用
                "eyes" => {
                    if !state.ai.selection_eyes_ready() {
                        return Err("闭眼检测模型未下载，请先在设置中下载模型（facemesh）".into());
                    }
                    db.create_eyes_tasks_for_unindexed()
                        .map_err(|e| format!("创建闭眼任务失败: {e}"))?;
                }
                "blur" => {
                    db.create_blur_tasks_for_unindexed()
                        .map_err(|e| format!("创建失焦任务失败: {e}"))?;
                }
                // M8-②：xxhash=0 哨兵（rename 快道/历史遗留）手动补算入口
                "hash" => {
                    db.create_hash_tasks_for_unhashed()
                        .map_err(|e| format!("创建哈希补算任务失败: {e}"))?;
                }
                _ => {}
            }
            db.retry_failed_index_tasks(kind)
                .map_err(|e| format!("重试失败索引任务失败: {e}"))?;
            // eyes 任务不被 index worker 池认领（step 白名单外，模型会话
            // 在闭眼回填 worker）→ 走专属回填入口；其余通道走通用池
            if kind == "eyes" {
                crate::ai::selection::kick_eyes_if_ready(
                    db_dir.clone(),
                    &state.ai,
                    &state.bus,
                    &supervisor,
                );
            } else {
                crate::index::kick(db_dir, &supervisor);
            }
            Ok(())
        }
        "ai" => {
            if !state.ai.semantic_ready() {
                let [visual, text] =
                    crate::ai::semantic_model_ids(state.ai.ai_params().quality_tier);
                return Err(format!(
                    "语义检索模型未下载，请先在设置中下载模型（{visual} / {text} / siglip2-tokenizer）"
                ));
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
                let det = crate::ai::face_detect_model_id(state.ai.ai_params().quality_tier);
                return Err(format!(
                    "人脸识别模型未下载，请先在设置中下载模型（{det} / arcface）"
                ));
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

/// 手动触发索引（kind = "thumb" | "exif" | "ai" | "face" | "phash" | "hash"；幂等）。
#[tauri::command]
pub async fn index_kick_now(state: State<'_, SharedState>, kind: String) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| fetch_index_kick_now(state, &kind)).await
}

// ---------------------------------------------------------------------------
// 索引重建（用户 2026-09-20：参数可配 + 改配置重建按钮）
// ---------------------------------------------------------------------------

/// 重建通道名（IPC kind）。
const REBUILD_KINDS: [&str; 6] = ["image", "semantic", "face", "thumb", "exif", "selection"];

/// 单通道重建核（同步、可测）：清理该通道全部持久化产物 + 时间账/任务账
/// 重排。返回重排后的待办数。不 kick worker（调用方决定：手动 IPC 后台
/// kick / 自动重建逐通道 kick）。
pub fn rebuild_channel_core(
    db: &crate::db::Db,
    db_dir: &std::path::Path,
    kind: &str,
) -> Result<u64, String> {
    match kind {
        "image" => {
            let barrier = crate::index::image_index_lock(db_dir);
            let _guard = barrier.write().map_err(|e| e.to_string())?;
            // The target is the established cache folder below this library,
            // never the photo root or the library directory itself.
            let cache = db_dir.join("thumbs");
            if cache.exists() {
                std::fs::remove_dir_all(&cache).map_err(|e| e.to_string())?;
            }
            let transaction = db.0.unchecked_transaction().map_err(|e| e.to_string())?;
            db.reset_thumb_states().map_err(|e| e.to_string())?;
            db.clear_exif_columns().map_err(|e| e.to_string())?;
            db.0.execute_batch(
                "DELETE FROM ai_analysis WHERE kind IN ('eyes','blur');
                DELETE FROM index_tasks WHERE kind IN ('thumb','exif','eyes','blur');",
            )
            .map_err(|e| e.to_string())?;
            let pending = db
                .requeue_thumb_tasks_for_all()
                .map_err(|e| e.to_string())?
                + db.requeue_exif_tasks_for_all().map_err(|e| e.to_string())?
                + db.requeue_eyes_tasks_for_all().map_err(|e| e.to_string())?
                + db.requeue_blur_tasks_for_all().map_err(|e| e.to_string())?;
            transaction.commit().map_err(|e| e.to_string())?;
            Ok(pending)
        }
        "semantic" => {
            // 向量真值（usearch 文件）+ 时间账 + 任务账；先逐出双池再删文件
            // （真机事故 2026-09-20：先删后逐出时，重建+回填竞态下 worker 持
            // 旧 view 句柄 add → immutable 报错；先逐出则后续取柄必为新建可变）
            crate::ai::semantic::invalidate_index(db_dir);
            let vectors = db_dir.join("vectors.usearch");
            if vectors.exists() {
                std::fs::remove_file(&vectors).map_err(|e| format!("删除向量索引失败: {e}"))?;
            }
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
        "selection" => {
            // 0021 选片分析重建：清分析产物 + 两通道任务账重排（eyes 模型
            // 未收录时其任务保持 pending 空转跳过）
            db.0
                .execute_batch("DELETE FROM ai_analysis;                                DELETE FROM index_tasks WHERE kind IN ('eyes', 'blur');")
                .map_err(|e| e.to_string())?;
            let eyes = db.requeue_eyes_tasks_for_all().map_err(|e| e.to_string())?;
            let blur = db.requeue_blur_tasks_for_all().map_err(|e| e.to_string())?;
            Ok(eyes + blur)
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
        "image" | "selection" => {
            crate::index::kick(db_dir.clone(), &supervisor);
            crate::ai::selection::kick_eyes_if_ready(db_dir, &state.ai, &bus, &supervisor);
        }
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
    super::ensure_no_import_running(state)?;
    let (enable_clip, enable_face) = {
        let settings = state.settings.lock().expect("settings mutex poisoned");
        (settings.ai.enable_clip, settings.ai.enable_face)
    };
    match kind {
        "semantic" => {
            if !state.ai.semantic_ready() {
                let [visual, text] =
                    crate::ai::semantic_model_ids(state.ai.ai_params().quality_tier);
                return Err(format!(
                    "语义检索模型未下载，请先在设置中下载模型（{visual} / {text} / siglip2-tokenizer）"
                ));
            }
            if !enable_clip {
                return Err("语义索引未开启（设置 → AI → 语义检索）".into());
            }
        }
        "face" => {
            if !state.ai.face_models_ready() {
                let det = crate::ai::face_detect_model_id(state.ai.ai_params().quality_tier);
                return Err(format!(
                    "人脸识别模型未下载，请先在设置中下载模型（{det} / arcface）"
                ));
            }
            if !enable_face {
                return Err("人脸识别未开启（设置 → AI → 人脸识别）".into());
            }
        }
        "image" | "thumb" | "exif" | "selection" => {}
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
    supervisor.spawn_unique(
        "index",
        format!("rebuild-{kind}:{}", db_dir.display()),
        move |_| {
            run_rebuild(&shared, &db_dir, &kind);
        },
    );
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

/// 语义通道指纹（版本 + embed_input_size——嵌入随输入尺寸变化；2026-09-28
/// 三档画质：accurate 档追加 fp16 双塔**模型版本**折叠——切档换件即换指纹
/// → 自动重建语义索引。fast/normal 同用 int8 件 → 不追加 → 互相切换不动
/// 语义索引，且与升级前的旧指纹**逐位一致**（既有 marker 命中，不误重建））。
pub fn semantic_params_fingerprint(ai: &crate::settings::AiSettings) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for part in [
        ai.index_params_version as u64,
        u64::from(ai.embed_input_size),
    ] {
        hash ^= part;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    if let Some(tag) = semantic_tier_tag(ai) {
        hash ^= tag;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// 人脸通道指纹（版本 + 检测门槛 + 聚类阈值；f32 用 to_bits 保精确比较；
/// 三档画质：fast 追加**检测模型 id+版本**（scrfd-10g 换件）；normal 与
/// accurate 人脸同件同源（2026-09-28 用户定规：检测源三档统一缓存优先
/// [512,2048]，不再 2048 优先）→ 均不追加，与旧指纹逐位一致。
/// 联动矩阵：fast↔normal/fast↔accurate 重建 face；normal↔accurate
/// **只重建 semantic**（fp16 双塔），人脸零重算。
pub fn face_params_fingerprint(ai: &crate::settings::AiSettings) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for part in [
        crate::ai::face::FACE_INDEX_GENERATION,
        ai.index_params_version as u64,
        u64::from(ai.embed_input_size), // 版本语义变更兜底参与
        ai.face_detect_threshold.to_bits() as u64,
        ai.face_cluster_threshold.to_bits() as u64,
    ] {
        hash ^= part;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    if let Some(tag) = face_tier_tag(ai) {
        hash ^= tag;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// FNV-1a 字节串折叠（档位派生标签用）。
fn fnv_str(hash: u64, s: &str) -> u64 {
    let mut h = hash;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// 语义档位派生标签：Some(值) 参与 fingerprint 折叠，None = 与升级前
/// 旧指纹兼容（不追加项）。accurate 折叠 fp16 双塔的 catalog 版本串
/// （版本 pin 变化 → 指纹变 → 重建）。
fn semantic_tier_tag(ai: &crate::settings::AiSettings) -> Option<u64> {
    let tier = crate::ai::QualityTier::from_setting(&ai.quality_tier)?;
    if !matches!(tier, crate::ai::QualityTier::Accurate) {
        return None; // fast/normal 同用 int8 件：与旧指纹兼容
    }
    let version = |id: &str| {
        crate::ai::catalog()
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.version.as_str())
            .unwrap_or("")
    };
    let [visual, text] = crate::ai::semantic_model_ids(tier);
    let mut h = 0xcbf29ce484222325u64;
    h = fnv_str(h, version(visual));
    Some(fnv_str(h, version(text)))
}

/// 人脸档位派生标签（None = 与旧指纹兼容不触发重建）：仅 fast 追加
/// scrfd-10g 模型 id+版本（换检测件必须重算）；normal/accurate 人脸
/// 同件同源（2026-09-28 检测源统一，2048 优先策略退役）→ None。
fn face_tier_tag(ai: &crate::settings::AiSettings) -> Option<u64> {
    let tier = crate::ai::QualityTier::from_setting(&ai.quality_tier)?;
    match tier {
        crate::ai::QualityTier::Fast => {
            let id = crate::ai::face_detect_model_id(tier);
            let version = crate::ai::catalog()
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.version.as_str())
                .unwrap_or("");
            let mut h = 0xcbf29ce484222325u64;
            h = fnv_str(h, id);
            Some(fnv_str(h, version))
        }
        crate::ai::QualityTier::Normal | crate::ai::QualityTier::Accurate => None,
    }
}

/// 选片分析指纹（0021：blur 阈值/算法版本 + eyes EAR 阈值/模型版本；f32
/// 用 to_bits 保精确比较）。任一阈值变更 → 两通道任务重排（分析结果随
/// 阈值变）；eyes 算法版本（facemesh 换件/判据改动）同样折叠。
pub fn selection_params_fingerprint(ai: &crate::settings::AiSettings) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for part in [
        ai.index_params_version as u64,
        ai.blur_soft_threshold.to_bits() as u64,
        ai.eyes_ear_closed.to_bits() as u64,
        ai.eyes_ear_maybe.to_bits() as u64,
        u64::from(ai.eyes_include_single),
    ] {
        hash ^= part;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let mut h = hash;
    for tag in [
        ai.quality_tier.as_str(),
        crate::ai::selection::BLUR_ALGO_VERSION,
        crate::ai::selection::EYES_ALGO_VERSION,
    ] {
        h = fnv_str(h, tag);
    }
    h
}

/// 连拍分组指纹（版本 + gap/hamming/min——改参数只重组不重算 pHash）。
pub fn burst_params_fingerprint(ai: &crate::settings::AiSettings) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for part in [
        ai.index_params_version as u64,
        u64::from(ai.burst_gap_ms),
        u64::from(ai.burst_hamming_max),
        u64::from(ai.burst_min_size),
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
    let (want_sem, want_face, want_burst, want_sel) = (
        semantic_params_fingerprint(ai),
        face_params_fingerprint(ai),
        burst_params_fingerprint(ai),
        selection_params_fingerprint(ai),
    );
    let marker = db_dir.join(PARAMS_MARKER);
    // 首跑（marker 不存在）：视参数为「一直就是当前值」，只写标记不重建
    // （真机事故：首跑误判参数变更触发语义重建，与启动回填竞态致 71 条失败）
    if !marker.is_file() {
        std::fs::write(
            &marker,
            format!(
                "semantic={want_sem}
face={want_face}
burst={want_burst}
selection={want_sel}
"
            ),
        )
        .ok();
        return;
    }
    let content = std::fs::read_to_string(&marker).unwrap_or_default();
    let stored_sem = content
        .lines()
        .find_map(|l| l.strip_prefix("semantic="))
        .and_then(|v| v.parse::<u64>().ok());
    let stored_face = content
        .lines()
        .find_map(|l| l.strip_prefix("face="))
        .and_then(|v| v.parse::<u64>().ok());
    let stored_burst = content
        .lines()
        .find_map(|l| l.strip_prefix("burst="))
        .and_then(|v| v.parse::<u64>().ok());
    // 0021 前的旧 marker 无 selection 行 → None（首次对齐即重排两通道任务，
    // 分析结果随阈值产出，与 gen-1 建档幂等）
    let stored_sel = content
        .lines()
        .find_map(|l| l.strip_prefix("selection="))
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
    // 连拍参数变更：只重组不重算（pHash 与参数无关）。旧 marker 无 burst 行
    //（0012 前创建）视作「从未分组」——一次重组后对齐。
    // 0021 选片参数变更（或旧 marker 升级）：两通道任务重排（清分析产物
    // 不必须——upsert 覆盖；阈值回退时旧记录随重算覆写）
    if stored_sel != Some(want_sel) {
        eprintln!("[params] 选片参数变更（{stored_sel:?} → {want_sel}），重排 eyes/blur 任务");
        if let Ok(db) = super::open_library_db(db_dir) {
            let _ = db.requeue_eyes_tasks_for_all();
            let _ = db.requeue_blur_tasks_for_all();
            let pending = db.pending_index_task_count("blur").unwrap_or(0);
            if pending > 0 {
                crate::index::kick(
                    db_dir.to_path_buf(),
                    &std::sync::Arc::clone(&state.supervisor),
                );
            }
        }
    }
    let mut regrouped_burst = false;
    if stored_burst != Some(want_burst) {
        eprintln!("[params] 连拍参数变更（{stored_burst:?} → {want_burst}），重组");
        let params = crate::bursts::BurstParams::from_settings(ai);
        let bus = state.bus.clone();
        let supervisor = std::sync::Arc::clone(&state.supervisor);
        crate::bursts::regroup_kick(db_dir.to_path_buf(), params, &bus, &supervisor);
        regrouped_burst = true;
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
    let burst_line = format!(
        "burst={}",
        if regrouped_burst || stored_burst == Some(want_burst) {
            want_burst
        } else {
            stored_burst.unwrap_or(want_burst)
        }
    );
    let sel_line = format!("selection={want_sel}");
    let _ = std::fs::create_dir_all(db_dir);
    let _ = std::fs::write(
        &marker,
        format!(
            "{sem_line}
{face_line}
{burst_line}
{sel_line}
"
        ),
    );
}

/// 暂停索引类后台任务（kind="index"：thumb/exif/hash/phash/blur 池 + AI
/// 回填池）。软语义：worker 步进间轮询生效。接线 2026-09-29——此前前端
/// 按钮静默无效（命令未注册）。
#[tauri::command]
pub async fn index_task_pause(state: State<'_, SharedState>) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, |state| {
        let hit = state.supervisor.pause_kind("index");
        eprintln!("手动暂停索引：命中 {hit} 个任务");
        Ok(())
    })
    .await
}

/// 恢复索引类后台任务。导入让路闸开着时拒绝（用户定案 2026-09-29：
/// 导入完成前无法手动恢复——导入收尾会自动恢复并按需触发一轮）。
#[tauri::command]
pub async fn index_task_resume(state: State<'_, SharedState>) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, |state| {
        super::ensure_no_import_running(state)?;
        let hit = state.supervisor.resume_kind("index");
        eprintln!("手动恢复索引：命中 {hit} 个任务");
        Ok(())
    })
    .await
}
