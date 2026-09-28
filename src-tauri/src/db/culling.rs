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

/// 快照条目（open 数据源：seq 序 + 决定 LEFT JOIN）。
#[derive(Debug, Clone, PartialEq)]
pub struct CullItemRow {
    pub asset_id: i64,
    /// None = 未定。
    pub decision: Option<String>,
    /// 无决定时 None。
    pub origin: Option<String>,
}

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
    pub fn cull_album_asset_ids(
        &self,
        album_id: i64,
        subgroup: Option<&str>,
    ) -> Result<Vec<i64>> {
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
        let rows = stmt.query_map(
            params![album_id, subgroup, CAPTURED_NULL_HIGH],
            |r| r.get(0),
        )?;
        rows.collect()
    }

    /// 同来源既有会话数（默认名轮次 = 返回值 + 1）：album 按 kind+albumId+
    /// subgroup 精确匹配（subgroup NULL 用 IS 判等）；query 只按 kind 计
    /// （筛选快照无稳定来源身份，所有 query 会话同轮次池）。
    pub fn cull_scope_sessions(&self, scope: &CullScope) -> Result<u64> {
        let n: i64 = match scope {
            CullScope::Album { album_id, subgroup } => {
                self.0.query_row(
                    "SELECT COUNT(*) FROM cull_session \
                     WHERE json_extract(scope, '$.kind') = 'album' \
                       AND json_extract(scope, '$.albumId') = ?1 \
                       AND json_extract(scope, '$.subgroup') IS ?2",
                    params![album_id, subgroup],
                    |r| r.get(0),
                )?
            }
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

    /// 快照条目（seq 升序 + 决定 LEFT JOIN；open 数据源）。
    /// **V1 全量返回**（万张内可接受）；V2 keyset 分页按 (session_id, seq)
    /// 游标即可——表结构已就绪。
    pub fn cull_session_items(&self, session_id: i64) -> Result<Vec<CullItemRow>> {
        let mut stmt = self.0.prepare(
            "SELECT s.asset_id, d.decision, d.origin \
             FROM cull_session_asset s \
             LEFT JOIN cull_decision d ON d.session_id = s.session_id AND d.asset_id = s.asset_id \
             WHERE s.session_id = ?1 \
             ORDER BY s.seq",
        )?;
        let rows = stmt.query_map(params![session_id], |r| {
            Ok(CullItemRow {
                asset_id: r.get(0)?,
                decision: r.get(1)?,
                origin: r.get(2)?,
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
            let mut clear = tx.prepare(
                "DELETE FROM cull_decision WHERE session_id = ?1 AND asset_id = ?2",
            )?;
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
        let n = self.0.execute("DELETE FROM cull_session WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    /// 同名会话存在性（默认名/改名去重用；表无唯一约束，重复名只是 UI 含混）。
    pub fn cull_name_taken(&self, name: &str) -> Result<bool> {
        let n: i64 = self
            .0
            .query_row(
                "SELECT COUNT(*) FROM cull_session WHERE name = ?1",
                params![name],
                |r| r.get(0),
            )?;
        Ok(n > 0)
    }
}
