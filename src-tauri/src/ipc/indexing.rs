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
    /// 选片索引（eyes+blur 合并完成度；2026-10-12 拆分为独立索引——图片
    /// 索引不再含这两通道，选片索引单独成行/单独触发/单独消费池）。
    pub selection: IndexKindStatus,
    pub selection_ready: bool,
    pub thumb: IndexKindStatus,
    pub exif: IndexKindStatus,
    pub ai: IndexKindStatus,
    pub face: IndexKindStatus,
    pub phash: IndexKindStatus,
    pub hash: IndexKindStatus,
    pub eyes: IndexKindStatus,
    pub blur: IndexKindStatus,
    /// AI 选片通道（eyes/blur）是否启用——「等待模型」提示只在开启且模型
    /// 未就绪时成立（2026-10-12：AI 关闭的库选片索引不建任务不计数）。
    pub selection_enabled: bool,
}

const KINDS: [&str; 10] = [
    "image", "selection", "thumb", "exif", "ai", "face", "phash", "hash", "eyes", "blur",
];

/// 状态聚合核：index_tasks 按 (kind, state) 计数 + assets 可索引总数。
pub fn fetch_index_status(state: &super::AppState) -> Result<IndexStatusDto, String> {
    // 引导期宽容(2026-10-09 多数据库修正):尚无激活数据库返回全空闲——
    // TaskDrawer 的 1.5s 轮询在 onboarding 期间不打错误日志。
    let no_database = state
        .settings
        .lock()
        .map(|s| s.active_database_id.is_none())
        .unwrap_or(true);
    if no_database {
        return Ok(IndexStatusDto::default());
    }
    let db = super::app_database_db(state)?;
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
    let selection_enabled = {
        let settings = state.settings.lock().expect("settings mutex poisoned");
        settings.ai.enable_clip || settings.ai.enable_face
    };
    dto.image = image_index_status(&db)?;
    dto.selection = selection_index_status(&db);
    dto.selection_ready = state.ai.selection_eyes_ready();
    dto.selection_enabled = selection_enabled;
    Ok(dto)
}

/// Count each photo once; completion requires every component. EXIF with no
/// task is the synchronous import path. **图片索引口径 = thumb + exif**
/// （2026-10-12 拆分：eyes/blur 归选片索引，不再参与图片完成度——图片
/// 索引不被选片模型的就绪状态拖住）。
fn image_index_status(db: &crate::db::Db) -> Result<IndexKindStatus, String> {
    let mut result = IndexKindStatus::default();
    let sql = "\
        WITH parts AS (
        SELECT a.id, a.thumb_state,
          MAX(CASE WHEN t.state='running' THEN 1 ELSE 0 END) AS running,
          MAX(CASE WHEN t.state='failed' THEN 1 ELSE 0 END) AS failed,
          MAX(CASE WHEN t.state='pending' THEN 1 ELSE 0 END) AS pending
        FROM assets a LEFT JOIN index_tasks t ON t.asset_id=a.id AND t.kind IN ('thumb','exif')
        WHERE a.kind IN ('photo','raw') GROUP BY a.id
      ) SELECT CASE WHEN running=1 THEN 'running' WHEN failed=1 THEN 'failed'
          WHEN pending=1 OR thumb_state=0 THEN 'pending'
          ELSE 'done' END, COUNT(*) FROM parts GROUP BY 1";
    per_asset_status(db, sql, &mut result)?;
    Ok(result)
}

/// 选片索引完成度（eyes+blur 合并，每张照片两通道都完成才计 done；
/// 与图片索引同构的 per-asset 聚合）。任务行不存在 = pending——AI 关闭
/// 的库无任务行，此口径全 pending，前端按 selectionEnabled 隐藏该行。
fn selection_index_status(db: &crate::db::Db) -> IndexKindStatus {
    let mut result = IndexKindStatus::default();
    let sql = "\
        WITH parts AS (
        SELECT a.id,
          MAX(CASE WHEN t.state='running' THEN 1 ELSE 0 END) AS running,
          MAX(CASE WHEN t.state='failed' THEN 1 ELSE 0 END) AS failed,
          MAX(CASE WHEN t.state='pending' THEN 1 ELSE 0 END) AS pending,
          MAX(CASE WHEN t.kind='eyes' AND t.state='done' THEN 1 ELSE 0 END) AS eyes_done,
          MAX(CASE WHEN t.kind='blur' AND t.state='done' THEN 1 ELSE 0 END) AS blur_done
        FROM assets a LEFT JOIN index_tasks t ON t.asset_id=a.id AND t.kind IN ('eyes','blur')
        WHERE a.kind IN ('photo','raw') GROUP BY a.id
      ) SELECT CASE WHEN running=1 THEN 'running' WHEN failed=1 THEN 'failed'
          WHEN pending=1 OR eyes_done=0 OR blur_done=0 THEN 'pending'
          ELSE 'done' END, COUNT(*) FROM parts GROUP BY 1";
    let _ = per_asset_status(db, sql, &mut result);
    result
}

/// per-asset 完成度聚合查询核（image/selection 共用）。
fn per_asset_status(
    db: &crate::db::Db,
    sql: &str,
    result: &mut IndexKindStatus,
) -> Result<(), String> {
    let mut stmt = db.0.prepare(sql).map_err(|e| e.to_string())?;
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
    Ok(())
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
    let db_dir = super::app_database_dir(state)?;
    let db = super::open_library_db(&db_dir)?;
    let supervisor = std::sync::Arc::clone(&state.supervisor);
    match kind {
        "image" => {
            crate::index::scheduler::pause(&db_dir,false,false)?;
            db.create_thumb_tasks_for_unindexed()
                .map_err(|e| e.to_string())?;
            // 图片域消费的全部通道一起重试失败行（thumb/exif/phash 都在
            // 图片池——一键恢复暂态性批量失败，2026-10-13 真机案例）
            for component in ["thumb", "exif", "phash"] {
                db.retry_failed_index_tasks(component)
                    .map_err(|e| e.to_string())?;
            }
            super::super::index::pipeline::kick(
                db_dir.clone(),
                &supervisor,
                super::super::index::pipeline::Pools::IMAGE,
            );
            Ok(())
        }
        // 选片索引（2026-10-12 拆分）：eyes+blur 独立触发/独立消费池，
        // 与缩略图/详细信息索引不绑定。AI 关闭的库不建任务（明确报错，
        // 与语义/人脸通道同款契约）。
        "selection" => {
            if !(enable_clip || enable_face) {
                return Err("选片索引未开启（设置 → AI）".into());
            }
            crate::index::scheduler::pause(&db_dir,true,false)?;
            db.create_eyes_tasks_for_unindexed()
                .map_err(|e| format!("创建闭眼任务失败: {e}"))?;
            db.create_blur_tasks_for_unindexed()
                .map_err(|e| format!("创建失焦任务失败: {e}"))?;
            for component in ["eyes", "blur"] {
                db.retry_failed_index_tasks(component)
                    .map_err(|e| e.to_string())?;
            }
            // eyes 模型未就绪时任务保持 pending（等待模型提示在选片行）；
            // blur 无模型依赖，消费池直接跑
            super::super::index::pipeline::kick(
                db_dir.clone(),
                &supervisor,
                super::super::index::pipeline::Pools::SELECTION,
            );
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
                "hash" => return Err("新索引使用文件元数据去重，不再安排全文件哈希任务".into()),
                _ => {}
            }
            db.retry_failed_index_tasks(kind)
                .map_err(|e| format!("重试失败索引任务失败: {e}"))?;
            // 通道 → 消费池：eyes → 闭眼回填（模型会话）；blur → 选片
            // 消费池（2026-10-12 拆分后不再走图片索引池）；其余走通用池
            if kind == "eyes" {
                crate::ai::selection::kick_eyes_if_ready(
                    db_dir.clone(),
                    &state.ai,
                    &state.bus,
                    &supervisor,
                );
            } else if kind == "blur" {
                crate::ai::selection::kick_selection_index(db_dir, &supervisor);
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
    let barrier=crate::index::image_index_lock(db_dir);
    let _guard=barrier.write().map_err(|e| e.to_string())?;
    crate::index::artifacts::invalidate(db_dir);
    match kind {
        "image" => {
            // The target is the established cache folder below this library,
            // never the photo root or the library directory itself.
            let cache = db_dir.join("thumbs");
            if cache.exists() {
                std::fs::remove_dir_all(&cache).map_err(|e| e.to_string())?;
            }
            let transaction = db.0.unchecked_transaction().map_err(|e| e.to_string())?;
            db.reset_thumb_states().map_err(|e| e.to_string())?;
            db.clear_exif_columns().map_err(|e| e.to_string())?;
            // 2026-10-12 拆分：图片重建只动 thumb/exif（选片产物/任务归
            // "selection" 重建通道，不动）
            db.0.execute_batch(
                "DELETE FROM index_tasks WHERE kind IN ('thumb','exif');",
            )
            .map_err(|e| e.to_string())?;
            let pending = db
                .requeue_thumb_tasks_for_all()
                .map_err(|e| e.to_string())?
                + db.requeue_exif_tasks_for_all().map_err(|e| e.to_string())?;
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
            // 未收录时其任务保持 pending 空转跳过）。写闸与图片重建同款：
            // 在途选片 worker（持读闸）收尾后才开始清理——旧任务不能在
            // 清理后回写分析产物。
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
        "image" => {
            crate::index::kick(db_dir, &supervisor);
        }
        // 选片重建 → 选片消费池（blur）+ 闭眼回填（eyes）
        "selection" => {
            crate::ai::selection::kick_selection_index(db_dir.clone(), &supervisor);
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

/// 激活数据库目录（重建后台任务用；多数据库修正按 activeDatabaseId 解析）。
pub fn app_db_dir(state: &super::AppState) -> Result<std::path::PathBuf, String> {
    super::app_database_dir(state)
}

/// 重建执行体（supervisor 线程 / 测试直调）：开库 → 清理+重排 → 事件 →
/// kick 对应 worker。
pub fn run_rebuild(state: &super::AppState, db_dir: &std::path::Path, kind: &str) {
    let Ok(db) = super::open_library_db(db_dir) else {
        log::warn!("[rebuild] 库不可用: {}", db_dir.display());
        return;
    };
    match rebuild_channel_core(&db, db_dir, kind) {
        Ok(pending) => kick_rebuilt_channel(state, db_dir.to_path_buf(), kind, pending),
        Err(error) => {
            log::warn!("[rebuild] {kind} 重建失败: {error}");
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
    let db_dir = run_blocking(shared.clone(), app_db_dir).await?;
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
/// 2026-10-12：AI 功能开关（clip|face 任一）折入指纹——开启即重排建档
/// （含存量补建），关闭不建新任务（消费/计数侧同步按开关让路）。
pub fn selection_params_fingerprint(ai: &crate::settings::AiSettings) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for part in [
        ai.index_params_version as u64,
        ai.blur_soft_threshold.to_bits() as u64,
        ai.eyes_ear_closed.to_bits() as u64,
        ai.eyes_ear_maybe.to_bits() as u64,
        u64::from(ai.eyes_include_single),
        u64::from(ai.enable_clip || ai.enable_face),
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

/// Configure immutable per-library parameters and reconcile durable task versions.
pub fn check_params_and_rebuild(
    state: &super::AppState,
    db_dir: &std::path::Path,
    ai: &crate::settings::AiSettings,
) {
    crate::index::scheduler::configure(db_dir,&state.ai,&state.bus,ai);
    crate::index::scheduler::kick(db_dir.to_path_buf(),&state.supervisor);
}

/// 索引池名归一（'static——spawn_blocking 闭包需要）：None/未知 → None。
fn as_index_pool(pool: Option<&str>) -> Option<&'static str> {
    match pool {
        Some("selection") => Some("selection"),
        Some("index") | None => Some("index"),
        _ => None,
    }
}

/// 暂停索引类后台任务。pool="index"（thumb/exif/hash/phash 图片索引池）
/// 或 "selection"（eyes/blur 选片索引池，2026-10-12 拆分）；缺省 "index"。
/// 软语义：worker 步进间轮询生效。
#[tauri::command]
pub async fn index_task_pause(
    state: State<'_, SharedState>,
    pool: Option<String>,
) -> Result<(), String> {
    let Some(pool) = as_index_pool(pool.as_deref()) else {
        return Err(format!("未知索引池: {}", pool.unwrap_or_default()));
    };
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        crate::index::scheduler::pause(&super::app_database_dir(state)?, pool == "selection", true)?;
        Ok(())
    })
    .await
}

/// Resume persisted channel policy; committed photos may run during import.
#[tauri::command]
pub async fn index_task_resume(
    state: State<'_, SharedState>,
    pool: Option<String>,
) -> Result<(), String> {
    let Some(pool) = as_index_pool(pool.as_deref()) else {
        return Err(format!("未知索引池: {}", pool.unwrap_or_default()));
    };
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
            let path = super::app_database_dir(state)?;
        crate::index::scheduler::pause(&path, pool == "selection", false)?;
        crate::index::scheduler::kick(path, &state.supervisor);
        Ok(())
    })
    .await
}
