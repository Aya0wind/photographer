//! 选片（Culling）会话命令（0024，V1）：会话持久化 + 决定读写 + 进度派生
//! + 收尾映射（旗标/星级/拒绝——v1 不做子组动作）。
//!
//! V2/V3 增量：open items 连拍组字段（对比视图数据源）+ `cull_ai_prescan`
//! AI 挑图预扫规则引擎（纯 DB 零推理，只建议不自动决定）。
//!
//! - 全部 async + run_blocking（UI 零阻塞铁律）。
//! - 决定/进度语义见 [`crate::db::culling`]：未定 = 无行，计数纯 SQL 派生。
//! - 收尾映射走既有命令内核（[`super::rating::fetch_asset_flag_set`] /
//!   [`super::rating::fetch_asset_rating_set`] /
//!   [`super::selection::fetch_asset_reject_set`]）——与主库单资产操作同一
//!   路径，含 XMP 即时投影（星级/拒绝写边车，LR 可识别）；先映射后落
//!   finished_at（映射中途失败 → 会话仍未收尾，可重试）。
//! - 幂等：已收尾会话拒绝决定写入与重复收尾。

use tauri::State;

use super::{run_blocking, SharedState};
use crate::db::culling::{CullCounts, CullScope, CullSessionRow};

/// 会话 DTO（IPC 契约 camelCase；计数全部 SQL 派生）。`ignored` 仅
/// cull_decision_apply 返回体携带（不在快照内被忽略的决定条数——契约要求
/// 「忽略并计数返回」，以可选字段最小偏移承载，其余命令不占载荷）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CullSessionDto {
    pub id: i64,
    pub name: String,
    pub scope: CullScope,
    pub total: u64,
    pub accepted: u64,
    pub rejected: u64,
    pub undecided: u64,
    pub created_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignored: Option<u64>,
}

/// 决定条目（cull_decision_apply 输入）：decision null = 回未定；
/// origin 可缺省（默认 manual；V3 AI 预标记写 ai）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CullDecisionInput {
    pub asset_id: i64,
    pub decision: Option<String>,
    #[serde(default)]
    pub origin: Option<String>,
}

/// 快照条目（open 返回；快照序）。**不含资产元数据**——前端经既有
/// asset_detail / 缩略图管线按 assetId 自取。V2 追加连拍组字段：
/// burstId（assets.burst_id，无组 null）+ burstSize（该组**会话快照内**
/// 成员数，无组 = 1）——V2 对比视图默认取同连拍组的数据源。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CullItemDto {
    pub asset_id: i64,
    /// "accepted" | "rejected" | null（未定）。
    pub decision: Option<String>,
    /// "manual" | "ai"；未定时 null。
    pub origin: Option<String>,
    pub burst_id: Option<i64>,
    pub burst_size: u32,
}

/// open 返回体：会话（含派生计数）+ 全量快照条目（V1 全量返回，万张内
/// 可接受——V2 按 (sessionId, seq) keyset 分页）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CullSessionDetailDto {
    pub session: CullSessionDto,
    pub items: Vec<CullItemDto>,
}

/// 收尾映射开关（v1：旗标/星级/拒绝三选任意组合；子组动作 V2）。
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CullFinishApply {
    pub accepted_flag: bool,
    /// Some(1-5)：已选资产写星级；None：不动星级。
    pub accepted_rating: Option<u8>,
    pub reject_rejected: bool,
}

/// 收尾结果（各动作实际作用的资产数；开关关闭/None 对应 0/null）。
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CullFinishResultDto {
    pub applied_flag: u64,
    pub applied_rating: Option<u64>,
    pub rejected: u64,
}

/// 行 + 计数 → DTO（scope JSON 反序列化；scope 是创建时来源记录——query
/// 的 assetIds 可能因资产删除与当前快照漂移，属预期语义）。
fn session_dto(row: &CullSessionRow, counts: CullCounts) -> Result<CullSessionDto, String> {
    let scope: CullScope =
        serde_json::from_str(&row.scope_json).map_err(|e| format!("scope JSON 损坏: {e}"))?;
    Ok(CullSessionDto {
        id: row.id,
        name: row.name.clone(),
        scope,
        total: counts.total,
        accepted: counts.accepted,
        rejected: counts.rejected,
        undecided: counts.undecided(),
        created_at: row.created_at.clone(),
        updated_at: row.updated_at.clone(),
        finished_at: row.finished_at.clone(),
        ignored: None,
    })
}

/// 决定值校验：None | "accepted" | "rejected"，其余拒绝。
fn parse_decision(raw: &Option<String>) -> Result<Option<&'static str>, String> {
    match raw.as_deref() {
        None => Ok(None),
        Some("accepted") => Ok(Some("accepted")),
        Some("rejected") => Ok(Some("rejected")),
        Some(other) => Err(format!("非法决定值: {other}（accepted/rejected/null）")),
    }
}

/// origin 校验：缺省 manual；只认 manual/ai。
fn parse_origin(raw: &Option<String>) -> Result<&'static str, String> {
    match raw.as_deref() {
        None | Some("manual") => Ok("manual"),
        Some("ai") => Ok("ai"),
        Some(other) => Err(format!("非法 origin: {other}（manual/ai）")),
    }
}

/// 建会话核：
/// - album：相册必须存在；快照 = 相册时间线全量（含子组过滤，拍摄时间序）。
/// - query：assetIds 非空；去重保序 + 只留库内现存资产（FK 约束前置过滤；
///   含回收站——「传入序」逐字面语义）。
/// - 默认名 `{来源描述} · 初选/复选N`（同来源第 1 个 = 初选，之后 = 复选N；
///   query 的来源池 = 全部 query 会话）。同名时自动「·2」后缀（表无唯一
///   约束，仅防 UI 含混）。
pub fn fetch_cull_session_create(
    state: &super::AppState,
    scope: CullScope,
) -> Result<CullSessionDto, String> {
    let db = super::active_library_db(state)?;
    let (asset_ids, source_desc) = match &scope {
        CullScope::Album { album_id, subgroup } => {
            let album_name: String =
                db.0.query_row("SELECT name FROM album WHERE id = ?1", [*album_id], |r| {
                    r.get(0)
                })
                .map_err(|_| "相册不存在".to_string())?;
            let ids = db
                .cull_album_asset_ids(*album_id, subgroup.as_deref())
                .map_err(|e| e.to_string())?;
            let desc = match subgroup {
                Some(sub) => format!("相册「{album_name}」/{sub}"),
                None => format!("相册「{album_name}」"),
            };
            (ids, desc)
        }
        CullScope::Query { asset_ids } => {
            if asset_ids.is_empty() {
                return Err("筛选快照为空（assetIds 空），无法创建选片会话".into());
            }
            // 去重保序 + 存在性过滤（分块 IN，防超 SQLite 变量上限）
            let mut existing = std::collections::HashSet::new();
            for chunk in asset_ids.chunks(500) {
                let slots = (0..chunk.len())
                    .map(|i| format!("?{}", i + 1))
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut stmt =
                    db.0.prepare(&format!("SELECT id FROM assets WHERE id IN ({slots})"))
                        .map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
                        r.get::<_, i64>(0)
                    })
                    .map_err(|e| e.to_string())?;
                for row in rows {
                    existing.insert(row.map_err(|e| e.to_string())?);
                }
            }
            let mut seen = std::collections::HashSet::new();
            let ids: Vec<i64> = asset_ids
                .iter()
                .copied()
                .filter(|id| existing.contains(id) && seen.insert(*id))
                .collect();
            if ids.is_empty() {
                return Err("筛选快照内无库内现存资产，无法创建选片会话".into());
            }
            (ids, format!("筛选快照 {} 张", asset_ids.len()))
        }
    };
    // 默认名轮次（同来源计数 + 1）
    let round = db.cull_scope_sessions(&scope).map_err(|e| e.to_string())? + 1;
    let base = if round == 1 {
        format!("{source_desc} · 初选")
    } else {
        format!("{source_desc} · 复选{round}")
    };
    // 同名自动「·N」后缀（轮次撞名/与用户改名的既有会话撞名的兜底）
    let mut name = base.clone();
    let mut n = 2;
    while db.cull_name_taken(&name).map_err(|e| e.to_string())? {
        name = format!("{base}·{n}");
        n += 1;
    }
    let scope_json = serde_json::to_string(&scope).map_err(|e| e.to_string())?;
    let row = db
        .cull_session_create(&name, &scope_json, &asset_ids)
        .map_err(|e| e.to_string())?;
    let dto = session_dto(
        &row,
        CullCounts {
            total: asset_ids.len() as u64,
            accepted: 0,
            rejected: 0,
        },
    )?;
    Ok(dto)
}

/// 会话列表核（updated_at DESC；进行中在前）。
pub fn fetch_cull_session_list(state: &super::AppState) -> Result<Vec<CullSessionDto>, String> {
    let db = super::active_library_db(state)?;
    let rows = db.cull_session_list().map_err(|e| e.to_string())?;
    let mut dtos = Vec::with_capacity(rows.len());
    for row in &rows {
        let counts = db
            .cull_session_counts(row.id)
            .map_err(|e| e.to_string())?
            .ok_or("选片会话已不存在")?;
        dtos.push(session_dto(row, counts)?);
    }
    Ok(dtos)
}

/// 打开会话核（快照序全量条目；资产元数据前端按 id 自取）。
pub fn fetch_cull_session_open(
    state: &super::AppState,
    session_id: i64,
) -> Result<CullSessionDetailDto, String> {
    let db = super::active_library_db(state)?;
    let row = db
        .cull_session_get(session_id)
        .map_err(|e| e.to_string())?
        .ok_or("选片会话不存在")?;
    let counts = db
        .cull_session_counts(session_id)
        .map_err(|e| e.to_string())?
        .ok_or("选片会话不存在")?;
    let items = db
        .cull_session_items(session_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|item| CullItemDto {
            asset_id: item.asset_id,
            decision: item.decision,
            origin: item.origin,
            burst_id: item.burst_id,
            burst_size: item.burst_size,
        })
        .collect();
    Ok(CullSessionDetailDto {
        session: session_dto(&row, counts)?,
        items,
    })
}

/// 批量决定核：校验 → 快照外 id 忽略（计数随 DTO 返回）→ 事务 upsert。
/// decision=null 删行回未定；origin 缺省 manual。已收尾会话拒绝写入。
pub fn fetch_cull_decision_apply(
    state: &super::AppState,
    session_id: i64,
    decisions: &[CullDecisionInput],
) -> Result<CullSessionDto, String> {
    // 先全量校验再落库（单条非法不产生半截写入）
    let mut parsed = Vec::with_capacity(decisions.len());
    for d in decisions {
        parsed.push(crate::db::culling::CullDecisionRow {
            asset_id: d.asset_id,
            decision: parse_decision(&d.decision)?,
            origin: parse_origin(&d.origin)?,
        });
    }
    let db = super::active_library_db(state)?;
    let row = db
        .cull_session_get(session_id)
        .map_err(|e| e.to_string())?
        .ok_or("选片会话不存在")?;
    if row.finished_at.is_some() {
        return Err(format!("选片会话「{}」已收尾，不能再标记", row.name));
    }
    let ignored = db
        .cull_decision_apply(session_id, &parsed)
        .map_err(|e| e.to_string())?;
    // 重取行（updated_at 已被 apply 刷新）+ 派生计数
    let fresh = db
        .cull_session_get(session_id)
        .map_err(|e| e.to_string())?
        .ok_or("选片会话不存在")?;
    let counts = db
        .cull_session_counts(session_id)
        .map_err(|e| e.to_string())?
        .ok_or("选片会话不存在")?;
    let mut dto = session_dto(&fresh, counts)?;
    dto.ignored = Some(ignored);
    Ok(dto)
}

/// 会话改名核（trim 非空；不存在/已收尾报错——收尾后定稿）。
pub fn fetch_cull_session_rename(
    state: &super::AppState,
    session_id: i64,
    name: &str,
) -> Result<CullSessionDto, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("会话名不能为空".into());
    }
    let db = super::active_library_db(state)?;
    let row = db
        .cull_session_get(session_id)
        .map_err(|e| e.to_string())?
        .ok_or("选片会话不存在")?;
    if row.finished_at.is_some() {
        return Err(format!("选片会话「{}」已收尾，不能改名", row.name));
    }
    db.cull_session_rename(session_id, name)
        .map_err(|e| e.to_string())?
        .then_some(())
        .ok_or("选片会话不存在")?;
    let fresh = db
        .cull_session_get(session_id)
        .map_err(|e| e.to_string())?
        .ok_or("选片会话不存在")?;
    let counts = db
        .cull_session_counts(session_id)
        .map_err(|e| e.to_string())?
        .ok_or("选片会话不存在")?;
    session_dto(&fresh, counts)
}

/// 弃置会话核：删会话 + 快照/决定（FK 级联），主库标记/资产绝不动。
pub fn fetch_cull_session_discard(state: &super::AppState, session_id: i64) -> Result<(), String> {
    let db = super::active_library_db(state)?;
    db.cull_session_discard(session_id)
        .map_err(|e| e.to_string())?
        .then_some(())
        .ok_or("选片会话不存在")?;
    Ok(())
}

/// 收尾核：已选/已剔除映射到主库（走既有命令内核——含 XMP 即时投影，
/// 与单资产操作行为一致），成功后 finished_at 落库。
/// - acceptedFlag → 旗标 P；acceptedRating（1-5）→ 星级；rejectRejected →
///   拒绝态 X（XMP 投影 -1，星级 DB 保留）。
/// - 幂等：已收尾拒绝重复映射（Err）；映射先于 finished_at 落库，中途失败
///   可整体重试（各映射本身幂等——同值重写）。
pub fn fetch_cull_session_finish(
    state: &super::AppState,
    session_id: i64,
    apply: CullFinishApply,
) -> Result<CullFinishResultDto, String> {
    if let Some(rating) = apply.accepted_rating {
        if !(1..=5).contains(&rating) {
            return Err(format!("收尾星级必须在 1-5：{rating}"));
        }
    }
    let db = super::active_library_db(state)?;
    let row = db
        .cull_session_get(session_id)
        .map_err(|e| e.to_string())?
        .ok_or("选片会话不存在")?;
    if row.finished_at.is_some() {
        return Err(format!("选片会话「{}」已收尾，拒绝重复映射", row.name));
    }
    let accepted = db
        .cull_decided_ids(session_id, "accepted")
        .map_err(|e| e.to_string())?;
    let rejected = db
        .cull_decided_ids(session_id, "rejected")
        .map_err(|e| e.to_string())?;
    drop(db);

    // —— 映射（既有命令内核直调：DB 写 + XMP 边车异步派发）——
    if apply.accepted_flag {
        for id in &accepted {
            super::rating::fetch_asset_flag_set(state, *id, true)?;
        }
    }
    if let Some(rating) = apply.accepted_rating {
        for id in &accepted {
            super::rating::fetch_asset_rating_set(state, *id, i64::from(rating))?;
        }
    }
    if apply.reject_rejected && !rejected.is_empty() {
        super::selection::fetch_asset_reject_set(state, &rejected, true)?;
    }

    // —— 映射全成 → 收尾落库（0 行 = 并发重复收尾，拒绝）——
    let db = super::active_library_db(state)?;
    if !db
        .cull_session_finish_mark(session_id)
        .map_err(|e| e.to_string())?
    {
        return Err(format!("选片会话「{}」已收尾，拒绝重复映射", row.name));
    }
    Ok(CullFinishResultDto {
        applied_flag: if apply.accepted_flag {
            accepted.len() as u64
        } else {
            0
        },
        applied_rating: apply.accepted_rating.map(|_| accepted.len() as u64),
        rejected: if apply.reject_rejected {
            rejected.len() as u64
        } else {
            0
        },
    })
}

// ---------------------------------------------------------------------------
// Tauri 命令壳（async + run_blocking；命令名 snake_case 契约固定）
// ---------------------------------------------------------------------------

/// 建选片会话（scope = 相册/子组 或 画廊筛选快照 assetIds）。
#[tauri::command]
pub async fn cull_session_create(
    state: State<'_, SharedState>,
    scope: CullScope,
) -> Result<CullSessionDto, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| fetch_cull_session_create(state, scope)).await
}

/// 会话列表（updated_at DESC；进行中在前；计数 SQL 派生）。
#[tauri::command]
pub async fn cull_session_list(
    state: State<'_, SharedState>,
) -> Result<Vec<CullSessionDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, fetch_cull_session_list).await
}

/// 打开会话（快照序全量 items；资产元数据前端按 assetId 经既有管线自取）。
#[tauri::command]
pub async fn cull_session_open(
    state: State<'_, SharedState>,
    session_id: i64,
) -> Result<CullSessionDetailDto, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_cull_session_open(state, session_id)
    })
    .await
}

/// 批量决定 upsert（decision=null 回未定；快照外 id 忽略并计数返回）。
#[tauri::command]
pub async fn cull_decision_apply(
    state: State<'_, SharedState>,
    session_id: i64,
    decisions: Vec<CullDecisionInput>,
) -> Result<CullSessionDto, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_cull_decision_apply(state, session_id, &decisions)
    })
    .await
}

/// 会话改名。
#[tauri::command]
pub async fn cull_session_rename(
    state: State<'_, SharedState>,
    session_id: i64,
    name: String,
) -> Result<CullSessionDto, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_cull_session_rename(state, session_id, &name)
    })
    .await
}

/// 弃置会话（删会话+决定；不动主库）。
#[tauri::command]
pub async fn cull_session_discard(
    state: State<'_, SharedState>,
    session_id: i64,
) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_cull_session_discard(state, session_id)
    })
    .await
}

/// 收尾：已选/已剔除映射到旗标/星级/拒绝（含 XMP 投影）+ finished_at。
#[tauri::command]
pub async fn cull_session_finish(
    state: State<'_, SharedState>,
    session_id: i64,
    apply: CullFinishApply,
) -> Result<CullFinishResultDto, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_cull_session_finish(state, session_id, apply)
    })
    .await
}

// ---------------------------------------------------------------------------
// AI 挑图预扫（V3，proposal §3.3）：纯 DB 规则引擎（零推理）——读既有
// ai_analysis / faces / bursts 给「预标记建议」。AI 只建议纪律不变：
// apply=true 也只写会话内 origin='ai' 决定（用户过片可翻转），绝不直接
// 动主库旗标/星级/拒绝（那是收尾映射的事）。
// ---------------------------------------------------------------------------

/// 检测开关（eyes/blur 共用形状）：enabled + 三档敏感度字符串。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CullRuleSwitch {
    pub enabled: bool,
    /// "weak" | "normal" | "strong"（[`parse_sensitivity`] 校验）。
    pub sensitivity: String,
}

/// 预扫规则输入（camelCase 契约；rulesEcho 回显同形——缺省字段按生效值
/// 回显，前端弹窗「按这些规则跑了 N 张」的直接数据源）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CullPrescanRulesInput {
    pub eyes: CullRuleSwitch,
    pub blur: CullRuleSwitch,
    /// 连拍组留最锐（组内 blur score 最高成员 accepted、其余 rejected；
    /// 无 blur 分的组不动）。
    #[serde(default)]
    pub burst_keep_sharpest: bool,
    /// faces 行数 > N 的资产跳过 eyes 规则（合影豁免）；0 = 关。
    #[serde(default)]
    pub group_exempt_faces: u32,
    /// accepted 建议数封顶（快照序先到先得；null = 不限；负数非法）。
    #[serde(default)]
    pub max_accepted: Option<i64>,
}

/// 结果桶：计数 + 明细 asset_id 列表（快照序）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CullPrescanBucketDto {
    pub count: u64,
    pub asset_ids: Vec<i64>,
}

/// 预扫返回体：apply=false 纯只读预览；apply=true 落 origin='ai' 决定并
/// 额外携带 applied 计数（= 实际写入决定行数——与两建议桶计数之和一致，
/// 前端拿去出 toast）。四桶非互斥分区（豁免桶是 eyes 通道旁路记录）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CullPrescanDto {
    pub suggested_accepted: CullPrescanBucketDto,
    pub suggested_rejected: CullPrescanBucketDto,
    /// 已有决定（manual 或既有 ai）被跳过的项。
    pub skipped_manual: CullPrescanBucketDto,
    /// 因合影豁免跳过 eyes 规则的未定项。
    pub exempted_group: CullPrescanBucketDto,
    /// 生效规则回显（已校验/已补默认值形态）。
    pub rules_echo: CullPrescanRulesInput,
    /// 仅 apply=true 携带。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied: Option<u64>,
}

fn bucket(ids: Vec<i64>) -> CullPrescanBucketDto {
    CullPrescanBucketDto {
        count: ids.len() as u64,
        asset_ids: ids,
    }
}

/// 敏感度校验：weak/normal/strong 之外拒绝。
fn parse_sensitivity(raw: &str) -> Result<crate::db::culling::CullSensitivity, String> {
    match raw {
        "weak" => Ok(crate::db::culling::CullSensitivity::Weak),
        "normal" => Ok(crate::db::culling::CullSensitivity::Normal),
        "strong" => Ok(crate::db::culling::CullSensitivity::Strong),
        other => Err(format!("非法敏感度: {other}（weak/normal/strong）")),
    }
}

/// AI 预扫核：规则校验 → 引擎求值（只读）→ apply 时落 origin='ai' 决定。
/// 值集映射（详见 db::culling）：eyes weak={closed}+score≥0.5 /
/// normal={closed} / strong={closed,maybe}；blur weak={}（关）/
/// normal=strong={soft}；eyes 与 burst 建议冲突保守取剔除。
pub fn fetch_cull_ai_prescan(
    state: &super::AppState,
    session_id: i64,
    rules: CullPrescanRulesInput,
    apply: bool,
) -> Result<CullPrescanDto, String> {
    // 先全量校验（非法规则不碰库）
    let eyes_sensitivity = parse_sensitivity(&rules.eyes.sensitivity)?;
    let blur_sensitivity = parse_sensitivity(&rules.blur.sensitivity)?;
    if let Some(max) = rules.max_accepted {
        if max < 0 {
            return Err(format!("maxAccepted 不能为负：{max}"));
        }
    }
    let parsed = crate::db::culling::CullPrescanRules {
        eyes_enabled: rules.eyes.enabled,
        eyes_sensitivity,
        blur_enabled: rules.blur.enabled,
        blur_sensitivity,
        burst_keep_sharpest: rules.burst_keep_sharpest,
        group_exempt_faces: rules.group_exempt_faces,
        max_accepted: rules.max_accepted.map(|v| v as u64),
    };
    let db = super::active_library_db(state)?;
    let row = db
        .cull_session_get(session_id)
        .map_err(|e| e.to_string())?
        .ok_or("选片会话不存在")?;
    if row.finished_at.is_some() {
        return Err(format!("选片会话「{}」已收尾，不能再预扫", row.name));
    }
    let outcome = db
        .cull_ai_prescan(session_id, &parsed)
        .map_err(|e| e.to_string())?
        .ok_or("选片会话不存在")?;
    let mut applied = None;
    if apply {
        let decisions: Vec<crate::db::culling::CullDecisionRow> = outcome
            .suggested_accepted
            .iter()
            .map(|&asset_id| crate::db::culling::CullDecisionRow {
                asset_id,
                decision: Some("accepted"),
                origin: "ai",
            })
            .chain(outcome.suggested_rejected.iter().map(|&asset_id| {
                crate::db::culling::CullDecisionRow {
                    asset_id,
                    decision: Some("rejected"),
                    origin: "ai",
                }
            }))
            .collect();
        applied = Some(decisions.len() as u64);
        db.cull_decision_apply(session_id, &decisions)
            .map_err(|e| e.to_string())?;
    }
    Ok(CullPrescanDto {
        suggested_accepted: bucket(outcome.suggested_accepted),
        suggested_rejected: bucket(outcome.suggested_rejected),
        skipped_manual: bucket(outcome.skipped_manual),
        exempted_group: bucket(outcome.exempted_group),
        rules_echo: rules,
        applied,
    })
}

/// AI 挑图预扫（apply=false 只读预览 / true 落 origin='ai' 预标记）。
#[tauri::command]
pub async fn cull_ai_prescan(
    state: State<'_, SharedState>,
    session_id: i64,
    rules: CullPrescanRulesInput,
    apply: bool,
) -> Result<CullPrescanDto, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_cull_ai_prescan(state, session_id, rules, apply)
    })
    .await
}
