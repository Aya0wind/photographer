//! 选片会话仓储（0024，V1）：会话/快照/决定三表 + 派生计数。
//!
//! 语义（docs/plans/2026-09-28-culling-proposal.md §1，用户已批准）：
//! - **快照不变式**：创建时解析出有序 asset id 列表落 cull_session_asset
//!   （album = 相册时间线序含子组过滤；query = 传入序）。此后库内增删
//!   **不再**改变快照（防新导入扰动）——资产被删时行级联消失、total 收敛。
//! - **进度纯派生**：accepted/rejected 从 cull_decision 计数、undecided =
//!   total − 已决定；不存任何汇总列（切页/重启不丢、无对账）。
//! - scope 列存创建时的来源 JSON（含 query 的原始 assetIds——来源记录，
//!   非当前真值；当前真值 = cull_session_asset）。同来源轮次（初选/复选
//!   命名）按 scope JSON 的 kind+albumId+subgroup 计数，用 json_extract。

use rusqlite::{params, Result};

use super::{now_rfc3339, Db, CAPTURED_NULL_HIGH};
use serde::{Deserialize, Serialize};

/// 会话来源（IPC 契约 `CullScope`；serde 输出 camelCase + kind 判别）：
/// `{ kind: "album", albumId, subgroup } | { kind: "query", assetIds }`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum CullScope {
    /// 相册/子组入口：创建时按相册时间线（含子组过滤）快照。
    #[serde(rename = "album", rename_all = "camelCase")]
    Album {
        album_id: i64,
        /// None = 整册（根 + 全部子组）。
        subgroup: Option<String>,
    },
    /// 画廊筛选快照入口：创建时的 asset id 列表（传入序）。
    #[serde(rename = "query", rename_all = "camelCase")]
    Query { asset_ids: Vec<i64> },
}

/// cull_session 行（计数不在行内——由 [`Db::cull_session_counts`] 派生）。
#[derive(Debug, Clone, PartialEq)]
pub struct CullSessionRow {
    pub id: i64,
    pub name: String,
    pub scope_json: String,
    pub created_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
}

/// 派生进度计数（total = 当前快照行数，随资产删除收敛）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CullCounts {
    pub total: u64,
    pub accepted: u64,
    pub rejected: u64,
}

impl CullCounts {
    /// 未定 = 快照内无决定行的资产。
    pub fn undecided(self) -> u64 {
        self.total - self.accepted - self.rejected
    }
}

/// 决定条目（批量 upsert 输入；decision None = 回未定删行）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CullDecisionRow {
    pub asset_id: i64,
    /// 'accepted' | 'rejected' | None（回未定）。
    pub decision: Option<&'static str>,
    /// 'manual' | 'ai'（V1 写入恒 manual；校验在 IPC 层）。
    pub origin: &'static str,
}

/// 快照条目（open 数据源：seq 序 + 决定 LEFT JOIN + burst 归属）。
#[derive(Debug, Clone, PartialEq)]
pub struct CullItemRow {
    pub asset_id: i64,
    /// None = 未定。
    pub decision: Option<String>,
    /// 无决定时 None。
    pub origin: Option<String>,
    /// assets.burst_id（无连拍组 None；V2 对比视图默认取同组）。
    pub burst_id: Option<i64>,
    /// 该 burst 在**会话快照内**的成员数（无组 = 1；与 bursts.asset_count
    /// 不同——快照可能只含组的一部分，UI「组 N 张」徽标按快照口径）。
    pub burst_size: u32,
}

/// 会话快照内 burst 成员数聚合子查询（?1 = session_id；open items 与 AI
/// 预扫共用——单条 SQL 内联聚合，绝无逐条 N+1 查询）。
const SNAPSHOT_BURST_SIZE_JOIN: &str = "LEFT JOIN ( \
     SELECT a2.burst_id AS bid, COUNT(*) AS cnt \
     FROM cull_session_asset s2 JOIN assets a2 ON a2.id = s2.asset_id \
     WHERE s2.session_id = ?1 AND a2.burst_id IS NOT NULL \
     GROUP BY a2.burst_id \
 ) bz ON bz.bid = a.burst_id";

fn map_session(row: &rusqlite::Row<'_>) -> Result<CullSessionRow> {
    Ok(CullSessionRow {
        id: row.get(0)?,
        name: row.get(1)?,
        scope_json: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
        finished_at: row.get(5)?,
    })
}

impl Db {
    /// 相册时间线全量 asset id（快照解析用）：与 album_assets_page 完全同
    /// 语义——in_trash=0、kind photo/raw、subgroup 过滤（None = 整册）、
    /// 拍摄时间序（NULL 最先 + 时间降序 + id 降序 tiebreak）。
    pub fn cull_album_asset_ids(&self, album_id: i64, subgroup: Option<&str>) -> Result<Vec<i64>> {
        let sql = match subgroup {
            Some(_) => {
                "SELECT a.id FROM assets a \
                 WHERE a.in_trash = 0 AND a.kind IN ('photo', 'raw') \
                   AND EXISTS (SELECT 1 FROM album_item ai \
                        WHERE ai.asset_id = a.id AND ai.album_id = ?1 AND ai.subgroup = ?2) \
                 ORDER BY COALESCE(a.captured_at, ?3) DESC, a.id DESC"
            }
            None => {
                "SELECT a.id FROM assets a \
                 WHERE a.in_trash = 0 AND a.kind IN ('photo', 'raw') \
                   AND EXISTS (SELECT 1 FROM album_item ai \
                        WHERE ai.asset_id = a.id AND ai.album_id = ?1) \
                 ORDER BY COALESCE(a.captured_at, ?3) DESC, a.id DESC"
            }
        };
        let mut stmt = self.0.prepare(sql)?;
        let rows = stmt.query_map(params![album_id, subgroup, CAPTURED_NULL_HIGH], |r| {
            r.get(0)
        })?;
        rows.collect()
    }

    /// 同来源既有会话数（默认名轮次 = 返回值 + 1）：album 按 kind+albumId+
    /// subgroup 精确匹配（subgroup NULL 用 IS 判等）；query 只按 kind 计
    /// （筛选快照无稳定来源身份，所有 query 会话同轮次池）。
    pub fn cull_scope_sessions(&self, scope: &CullScope) -> Result<u64> {
        let n: i64 = match scope {
            CullScope::Album { album_id, subgroup } => self.0.query_row(
                "SELECT COUNT(*) FROM cull_session \
                     WHERE json_extract(scope, '$.kind') = 'album' \
                       AND json_extract(scope, '$.albumId') = ?1 \
                       AND json_extract(scope, '$.subgroup') IS ?2",
                params![album_id, subgroup],
                |r| r.get(0),
            )?,
            CullScope::Query { .. } => self.0.query_row(
                "SELECT COUNT(*) FROM cull_session \
                 WHERE json_extract(scope, '$.kind') = 'query'",
                [],
                |r| r.get(0),
            )?,
        };
        Ok(n as u64)
    }

    /// 建会话：单事务写 session 行 + 快照行（seq 0 起，传入序即快照序；
    /// asset_ids 已由调用方去重/存在性过滤）。name 唯一性由调用方保证。
    pub fn cull_session_create(
        &self,
        name: &str,
        scope_json: &str,
        asset_ids: &[i64],
    ) -> Result<CullSessionRow> {
        let now = now_rfc3339();
        let tx = self.0.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO cull_session (name, scope, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?3)",
            params![name, scope_json, now],
        )?;
        let id = tx.last_insert_rowid();
        {
            let mut stmt = tx.prepare(
                "INSERT INTO cull_session_asset (session_id, seq, asset_id) VALUES (?1, ?2, ?3)",
            )?;
            for (seq, asset_id) in asset_ids.iter().enumerate() {
                stmt.execute(params![id, seq as i64, asset_id])?;
            }
        }
        tx.commit()?;
        Ok(CullSessionRow {
            id,
            name: name.to_string(),
            scope_json: scope_json.to_string(),
            created_at: now.clone(),
            updated_at: now,
            finished_at: None,
        })
    }

    /// 会话行（不存在 None）。
    pub fn cull_session_get(&self, id: i64) -> Result<Option<CullSessionRow>> {
        let mut stmt = self.0.prepare(
            "SELECT id, name, scope, created_at, updated_at, finished_at \
             FROM cull_session WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(map_session(row)?)),
            None => Ok(None),
        }
    }

    /// 会话列表（updated_at DESC；进行中在前——finished_at 非空排后）。
    pub fn cull_session_list(&self) -> Result<Vec<CullSessionRow>> {
        let mut stmt = self.0.prepare(
            "SELECT id, name, scope, created_at, updated_at, finished_at \
             FROM cull_session \
             ORDER BY (finished_at IS NOT NULL), updated_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], map_session)?;
        rows.collect()
    }

    /// 派生计数：total = 当前快照行数；accepted/rejected = 决定行计数。
    /// （决定行按构造只在快照内——apply 侧已过滤；跨资产删除后两侧同步
    /// 级联，不变式保持。）
    pub fn cull_session_counts(&self, session_id: i64) -> Result<Option<CullCounts>> {
        let exists: bool = self.0.query_row(
            "SELECT EXISTS(SELECT 1 FROM cull_session WHERE id = ?1)",
            params![session_id],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(None);
        }
        let (total, accepted, rejected) = self.0.query_row(
            "SELECT \
               (SELECT COUNT(*) FROM cull_session_asset WHERE session_id = ?1), \
               (SELECT COUNT(*) FROM cull_decision WHERE session_id = ?1 AND decision = 'accepted'), \
               (SELECT COUNT(*) FROM cull_decision WHERE session_id = ?1 AND decision = 'rejected')",
            params![session_id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)? as u64,
                    r.get::<_, i64>(1)? as u64,
                    r.get::<_, i64>(2)? as u64,
                ))
            },
        )?;
        Ok(Some(CullCounts {
            total,
            accepted,
            rejected,
        }))
    }

    /// 快照条目（seq 升序 + 决定 LEFT JOIN + burstId/burstSize 聚合；open
    /// 数据源）。**V1 全量返回**（万张内可接受）；V2 keyset 分页按
    /// (session_id, seq) 游标即可——表结构已就绪。
    pub fn cull_session_items(&self, session_id: i64) -> Result<Vec<CullItemRow>> {
        let mut stmt = self.0.prepare(&format!(
            "SELECT s.asset_id, d.decision, d.origin, a.burst_id, \
                    CASE WHEN a.burst_id IS NULL THEN 1 ELSE COALESCE(bz.cnt, 1) END \
             FROM cull_session_asset s \
             JOIN assets a ON a.id = s.asset_id \
             LEFT JOIN cull_decision d ON d.session_id = s.session_id AND d.asset_id = s.asset_id \
             {SNAPSHOT_BURST_SIZE_JOIN} \
             WHERE s.session_id = ?1 \
             ORDER BY s.seq"
        ))?;
        let rows = stmt.query_map(params![session_id], |r| {
            Ok(CullItemRow {
                asset_id: r.get(0)?,
                decision: r.get(1)?,
                origin: r.get(2)?,
                burst_id: r.get(3)?,
                burst_size: r.get::<_, i64>(4)? as u32,
            })
        })?;
        rows.collect()
    }

    /// 批量决定 upsert（单事务）：decision Some → 整行覆盖（decision/origin/
    /// decided_at）；None → 删行回未定。**不在快照内的 id 忽略**（返回计数；
    /// 陈旧 UI 值属预期）。调用方保证会话存在且未收尾。
    pub fn cull_decision_apply(
        &self,
        session_id: i64,
        decisions: &[CullDecisionRow],
    ) -> Result<u64> {
        let now = now_rfc3339();
        let tx = self.0.unchecked_transaction()?;
        let mut ignored = 0u64;
        {
            // 快照内 id 集（内存判重；万张内 HashSet 开销可忽略）
            let mut in_snapshot = std::collections::HashSet::new();
            {
                let mut stmt =
                    tx.prepare("SELECT asset_id FROM cull_session_asset WHERE session_id = ?1")?;
                let rows = stmt.query_map(params![session_id], |r| r.get::<_, i64>(0))?;
                for row in rows {
                    in_snapshot.insert(row?);
                }
            }
            let mut upsert = tx.prepare(
                "INSERT INTO cull_decision (session_id, asset_id, decision, origin, decided_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT (session_id, asset_id) DO UPDATE SET \
                   decision = excluded.decision, origin = excluded.origin, \
                   decided_at = excluded.decided_at",
            )?;
            let mut clear =
                tx.prepare("DELETE FROM cull_decision WHERE session_id = ?1 AND asset_id = ?2")?;
            for d in decisions {
                if !in_snapshot.contains(&d.asset_id) {
                    ignored += 1;
                    continue;
                }
                match d.decision {
                    Some(decision) => {
                        upsert.execute(params![session_id, d.asset_id, decision, d.origin, now])?;
                    }
                    None => {
                        clear.execute(params![session_id, d.asset_id])?;
                    }
                }
            }
        }
        if !decisions.is_empty() {
            tx.execute(
                "UPDATE cull_session SET updated_at = ?2 WHERE id = ?1",
                params![session_id, now],
            )?;
        }
        tx.commit()?;
        Ok(ignored)
    }

    /// 某决定的快照内 asset id 清单（收尾映射数据源；INNER JOIN 快照兜底
    /// ——任何绕过 apply 的杂散决定行不参与映射）。
    pub fn cull_decided_ids(&self, session_id: i64, decision: &str) -> Result<Vec<i64>> {
        let mut stmt = self.0.prepare(
            "SELECT d.asset_id FROM cull_decision d \
             JOIN cull_session_asset s \
               ON s.session_id = d.session_id AND s.asset_id = d.asset_id \
             WHERE d.session_id = ?1 AND d.decision = ?2",
        )?;
        let rows = stmt.query_map(params![session_id, decision], |r| r.get(0))?;
        rows.collect()
    }

    /// 会话改名（trim 后非空由 IPC 层校验）。返回是否存在。
    pub fn cull_session_rename(&self, id: i64, name: &str) -> Result<bool> {
        let n = self.0.execute(
            "UPDATE cull_session SET name = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, name, now_rfc3339()],
        )?;
        Ok(n > 0)
    }

    /// 收尾落库（finished_at + updated_at；只在未收尾时生效）。
    /// 返回 false = 会话不存在或已收尾（区分由调用方先行查行决定）。
    pub fn cull_session_finish_mark(&self, id: i64) -> Result<bool> {
        let n = self.0.execute(
            "UPDATE cull_session SET finished_at = ?2, updated_at = ?2 \
             WHERE id = ?1 AND finished_at IS NULL",
            params![id, now_rfc3339()],
        )?;
        Ok(n > 0)
    }

    /// 弃置会话：只删 session 行——快照/决定经 FK ON DELETE CASCADE 随灭，
    /// 主库资产/旗标/星级/拒绝态绝不动。
    pub fn cull_session_discard(&self, id: i64) -> Result<bool> {
        let n = self
            .0
            .execute("DELETE FROM cull_session WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    /// 同名会话存在性（默认名/改名去重用；表无唯一约束，重复名只是 UI 含混）。
    pub fn cull_name_taken(&self, name: &str) -> Result<bool> {
        let n: i64 = self.0.query_row(
            "SELECT COUNT(*) FROM cull_session WHERE name = ?1",
            params![name],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    // -----------------------------------------------------------------------
    // AI 挑图预扫（V3，proposal §3.3）：纯 DB 规则引擎——零推理，只读既有
    // ai_analysis / faces / bursts 数据给「预标记建议」，绝不自动定案。
    // -----------------------------------------------------------------------

    /// AI 预扫（会话不存在 None）。规则求值全程单条 JOIN 查询取数 + 内存
    /// 求值；**只读**——落库由调用方经 [`Db::cull_decision_apply`]
    /// （origin='ai'）完成。桶语义（四桶非互斥分区：豁免桶是 eyes 通道的
    /// 旁路记录，被豁免项仍可能被 blur/burst 规则建议剔除）：
    /// - suggested_accepted / suggested_rejected：最终建议（含 maxAccepted
    ///   封顶后转不动的 accepted **不**入桶）；
    /// - skipped_manual：已有决定（manual 或既有 ai）的项——规则整组跳过；
    /// - exempted_group：因合影豁免跳过 eyes 规则的未定项。
    pub fn cull_ai_prescan(
        &self,
        session_id: i64,
        rules: &CullPrescanRules,
    ) -> Result<Option<CullPrescanOutcome>> {
        let exists: bool = self.0.query_row(
            "SELECT EXISTS(SELECT 1 FROM cull_session WHERE id = ?1)",
            params![session_id],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(None);
        }
        // 单条查询取全量求值输入（快照序）：决定存在性 + eyes/blur 分析行
        // + faces 计数 + burst 归属与快照内组员数
        let mut stmt = self.0.prepare(&format!(
            "SELECT s.asset_id, d.decision IS NOT NULL, \
                    ee.value, ee.score, bb.value, bb.score, \
                    a.burst_id, \
                    CASE WHEN a.burst_id IS NULL THEN 1 ELSE COALESCE(bz.cnt, 1) END, \
                    COALESCE(fc.cnt, 0) \
             FROM cull_session_asset s \
             JOIN assets a ON a.id = s.asset_id \
             LEFT JOIN cull_decision d ON d.session_id = s.session_id AND d.asset_id = s.asset_id \
             LEFT JOIN ai_analysis ee ON ee.asset_id = s.asset_id AND ee.kind = 'eyes' \
             LEFT JOIN ai_analysis bb ON bb.asset_id = s.asset_id AND bb.kind = 'blur' \
             LEFT JOIN (SELECT asset_id AS aid, COUNT(*) AS cnt FROM faces GROUP BY asset_id) fc \
               ON fc.aid = s.asset_id \
             {SNAPSHOT_BURST_SIZE_JOIN} \
             WHERE s.session_id = ?1 \
             ORDER BY s.seq"
        ))?;
        let rows = stmt.query_map(params![session_id], |r| {
            Ok(PrescanRow {
                asset_id: r.get(0)?,
                decided: r.get(1)?,
                eyes_value: r.get(2)?,
                eyes_score: r.get(3)?,
                blur_value: r.get(4)?,
                blur_score: r.get(5)?,
                burst_id: r.get(6)?,
                burst_size: r.get::<_, i64>(7)? as u32,
                faces: r.get::<_, i64>(8)? as u32,
            })
        })?;
        let rows: Vec<PrescanRow> = rows.collect::<Result<_>>()?;

        // 组级预聚合：每组（burstSize≥2）blur score 最高的成员（最锐）。
        // **全组成员参与比拼**（已决定的也计入基准——用户已剔除最锐帧时，
        // 其余组员按「其余组员」整体建议剔除，不偷偷晋升次锐为 accepted）；
        // 同分取快照序先者；组内全无 blur 分 → 不入表（整组不动）。
        let mut sharpest: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
        for (idx, r) in rows.iter().enumerate() {
            if r.burst_size < 2 {
                continue;
            }
            if let (Some(bid), Some(score)) = (r.burst_id, r.blur_score) {
                match sharpest.get(&bid) {
                    Some(&best) if rows[best].blur_score.is_some_and(|s| s >= score) => {}
                    _ => {
                        sharpest.insert(bid, idx);
                    }
                }
            }
        }

        let mut outcome = CullPrescanOutcome::default();
        let mut accepted_count: u64 = 0;
        for (idx, r) in rows.iter().enumerate() {
            if r.decided {
                outcome.skipped_manual.push(r.asset_id);
                continue;
            }
            // suggestion：Some(true)=accepted / Some(false)=rejected / None=不动
            let mut suggestion: Option<bool> = None;

            // ① eyes：敏感度值集命中 → 剔除建议；合影豁免（>N 人不判闭眼；
            //    N=0 关）只旁路本规则，不拦后面规则
            if rules.eyes_enabled {
                let exempt = rules.group_exempt_faces > 0 && r.faces > rules.group_exempt_faces;
                if exempt {
                    outcome.exempted_group.push(r.asset_id);
                } else if eyes_hits(
                    rules.eyes_sensitivity,
                    r.eyes_value.as_deref(),
                    r.eyes_score,
                ) {
                    suggestion = Some(false);
                }
            }
            // ② blur：值集命中 → 剔除建议（weak 值集为空 = 不启用）
            if rules.blur_enabled && blur_hits(rules.blur_sensitivity, r.blur_value.as_deref()) {
                suggestion = Some(false);
            }
            // ③ burstKeepSharpest：最锐组员 → accepted（仅在未被 ①② 建议
            //    剔除时——冲突保守取剔除）；其余组员 → rejected
            if rules.burst_keep_sharpest && r.burst_size >= 2 {
                if let Some(bid) = r.burst_id {
                    match sharpest.get(&bid) {
                        Some(&best) if best == idx => {
                            if suggestion.is_none() {
                                suggestion = Some(true);
                            }
                        }
                        Some(_) => suggestion = Some(false),
                        // 组内全无 blur 分：整组不动
                        None => {}
                    }
                }
            }

            // ④ maxAccepted：accepted 建议按快照序先到先得，封顶后转不动；
            //    rejected 建议不受限
            match suggestion {
                Some(true) => {
                    if rules.max_accepted.is_some_and(|cap| accepted_count >= cap) {
                        continue;
                    }
                    accepted_count += 1;
                    outcome.suggested_accepted.push(r.asset_id);
                }
                Some(false) => outcome.suggested_rejected.push(r.asset_id),
                None => {}
            }
        }
        Ok(Some(outcome))
    }
}

/// 预扫求值输入行（[`Db::cull_ai_prescan`] 内部用）。
struct PrescanRow {
    asset_id: i64,
    /// 已有决定（manual 或既有 ai）——规则整组跳过。
    decided: bool,
    eyes_value: Option<String>,
    eyes_score: Option<f64>,
    blur_value: Option<String>,
    blur_score: Option<f64>,
    burst_id: Option<i64>,
    burst_size: u32,
    /// faces 行数（0 = 无人脸）。
    faces: u32,
}

/// eyes 敏感度 → ai_analysis('eyes').value 剔除值集：
/// - weak = {closed} **且额外要求 score ≥ 0.5**（score = 闭眼置信 0..1，
///   分析通道已恒写入——见 ai::selection；weak 档用它压误报）
/// - normal = {closed}（不看 score）
/// - strong = {closed, maybe}
///
/// unknown / 无记录永不命中。
fn eyes_hits(sensitivity: CullSensitivity, value: Option<&str>, score: Option<f64>) -> bool {
    match sensitivity {
        CullSensitivity::Weak => value == Some("closed") && score.is_some_and(|s| s >= 0.5),
        CullSensitivity::Normal => value == Some("closed"),
        CullSensitivity::Strong => matches!(value, Some("closed") | Some("maybe")),
    }
}

/// blur 敏感度 → ai_analysis('blur').value 剔除值集（现值域 sharp/soft/
/// unknown）：weak = {}（关）；normal = strong = {soft}。
fn blur_hits(sensitivity: CullSensitivity, value: Option<&str>) -> bool {
    match sensitivity {
        CullSensitivity::Weak => false,
        CullSensitivity::Normal | CullSensitivity::Strong => value == Some("soft"),
    }
}

/// 敏感度档（eyes/blur 规则共用；IPC 层字符串校验后转入）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CullSensitivity {
    Weak,
    Normal,
    Strong,
}

/// AI 预扫规则（V3，**已校验**形态——字符串敏感度在 IPC 层解析为本枚举）。
#[derive(Debug, Clone, PartialEq)]
pub struct CullPrescanRules {
    pub eyes_enabled: bool,
    pub eyes_sensitivity: CullSensitivity,
    pub blur_enabled: bool,
    pub blur_sensitivity: CullSensitivity,
    pub burst_keep_sharpest: bool,
    /// faces 行数 > N 的资产跳过 eyes 规则（合影豁免）；0 = 关。
    pub group_exempt_faces: u32,
    /// accepted 建议数封顶（快照序先到先得；None = 不限）。
    pub max_accepted: Option<u64>,
}

/// 预扫结果（四桶快照序 asset id 列表；计数由 DTO 层取 len）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CullPrescanOutcome {
    pub suggested_accepted: Vec<i64>,
    pub suggested_rejected: Vec<i64>,
    pub skipped_manual: Vec<i64>,
    pub exempted_group: Vec<i64>,
}
