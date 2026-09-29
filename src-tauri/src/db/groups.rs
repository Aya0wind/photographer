//! 连拍、相似照片与版本分组仓储。

use super::*;

impl Db {
    /// 连拍扫描行：(id, phash, captured_at, kind, pair_asset_id)——
    /// 已算 pHash 的 photo/raw 按 (camera, captured_at, id) 升序。
    pub fn burst_scan_rows(&self) -> Result<Vec<BurstScanRow>> {
        let mut stmt = self.0.prepare(
            "SELECT id, phash, captured_at, kind, pair_asset_id FROM assets              WHERE phash IS NOT NULL AND kind IN ('photo', 'raw')              ORDER BY camera ASC, captured_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?;
        rows.collect()
    }

    /// 连拍组整体重写（事务：清旧组 + burst_id 复位 → 建组 + 回填归属）。
    /// members = 每组 (asset_id, captured_at) 列表（组内有序）。
    pub fn write_bursts(&self, groups: &[Vec<(i64, Option<String>)>]) -> Result<()> {
        let tx = self.0.unchecked_transaction()?;
        tx.execute(
            "UPDATE assets SET burst_id = NULL WHERE burst_id IS NOT NULL",
            [],
        )?;
        tx.execute("DELETE FROM bursts", [])?;
        for members in groups {
            let (started, ended) = (
                members.first().and_then(|m| m.1.clone()),
                members.last().and_then(|m| m.1.clone()),
            );
            tx.execute(
                "INSERT INTO bursts (asset_count, started_at, ended_at) VALUES (?1, ?2, ?3)",
                params![members.len() as i64, started, ended],
            )?;
            let burst_id = tx.last_insert_rowid();
            for (asset_id, _) in members {
                tx.execute(
                    "UPDATE assets SET burst_id = ?2 WHERE id = ?1",
                    params![asset_id, burst_id],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// 连拍统计（设置页/调试）：(组数, 入组资产数)。
    pub fn burst_stats(&self) -> Result<(u64, u64)> {
        let (groups, photos): (i64, i64) = self.0.query_row(
            "SELECT COUNT(*), COALESCE(SUM(asset_count), 0) FROM bursts",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok((groups as u64, photos as u64))
    }

    /// 批量取组员数（页内 burstId → burstCount 装配，免 N+1）。
    pub fn burst_counts(&self, burst_ids: &[i64]) -> Result<HashMap<i64, u32>> {
        let mut out = HashMap::new();
        let ids: Vec<i64> = {
            let mut seen = std::collections::HashSet::new();
            burst_ids
                .iter()
                .copied()
                .filter(|id| seen.insert(*id))
                .collect()
        };
        if ids.is_empty() {
            return Ok(out);
        }
        let slots = (0..ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("SELECT b.id, b.asset_count FROM bursts b WHERE b.id IN ({slots})");
        let mut stmt = self.0.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as u32))
        })?;
        for row in rows {
            let (id, count) = row?;
            out.insert(id, count);
        }
        Ok(out)
    }

    /// pHash 写入（worker 成果）。
    pub fn set_phash(&self, id: i64, phash: u64) -> Result<()> {
        self.0.execute(
            "UPDATE assets SET phash = ?2 WHERE id = ?1",
            params![id, phash as i64],
        )?;
        Ok(())
    }

    /// 为 pHash 回填建任务：phash IS NULL 的 photo/raw 且无未完成 phash 任务。
    /// RAW+JPG 孪生两边都算（分组时才跳孪生；查重也要用）。
    pub fn create_phash_tasks_for_unindexed(&self) -> Result<u64> {
        let now = now_rfc3339();
        let created = self.0.execute(
            "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at)              SELECT 'phash', a.id, 'pending', 0, ?1, ?1 FROM assets a              WHERE a.phash IS NULL AND a.kind IN ('photo', 'raw')                AND NOT EXISTS (SELECT 1 FROM index_tasks t                                WHERE t.kind = 'phash' AND t.asset_id = a.id                                AND t.state IN ('pending', 'running'))",
            params![now],
        )?;
        Ok(created as u64)
    }

    /// phash 通道代际重排（既有复位 pending + 无任务行新建）。
    pub fn requeue_phash_tasks_for_all(&self) -> Result<u64> {
        let now = now_rfc3339();
        self.0.execute(
            "UPDATE index_tasks SET state = 'pending', attempts = 0, updated_at = ?1              WHERE kind = 'phash' AND state != 'pending'",
            params![now],
        )?;
        self.0.execute(
            "INSERT OR IGNORE INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at)              SELECT 'phash', a.id, 'pending', 0, ?1, ?1 FROM assets a              WHERE a.kind IN ('photo', 'raw')                AND NOT EXISTS (SELECT 1 FROM index_tasks t                                WHERE t.kind = 'phash' AND t.asset_id = a.id)",
            params![now],
        )?;
        self.pending_index_task_count("phash")
    }

    /// xxhash 补算写入（hash worker 成果）。
    pub fn set_xxhash(&self, id: i64, xxhash: u64) -> Result<()> {
        self.0.execute(
            "UPDATE assets SET xxhash = ?2 WHERE id = ?1",
            params![id, xxhash as i64],
        )?;
        Ok(())
    }

    /// 为哈希补算建任务：xxhash = 0 哨兵（rename 快道遗留）且无未完成
    /// hash 任务的照片和 RAW 资产。
    pub fn create_hash_tasks_for_unhashed(&self) -> Result<u64> {
        let now = now_rfc3339();
        let created = self.0.execute(
            "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at)              SELECT 'hash', a.id, 'pending', 0, ?1, ?1 FROM assets a              WHERE a.xxhash = 0 AND a.kind IN ('photo', 'raw')                AND NOT EXISTS (SELECT 1 FROM index_tasks t                                WHERE t.kind = 'hash' AND t.asset_id = a.id                                AND t.state IN ('pending', 'running'))",
            params![now],
        )?;
        Ok(created as u64)
    }

    /// hash 通道代际重排（既有复位 + 无任务行新建；对齐 requeue_* 家族）。
    pub fn requeue_hash_tasks_for_all(&self) -> Result<u64> {
        let now = now_rfc3339();
        self.0.execute(
            "UPDATE index_tasks SET state = 'pending', attempts = 0, updated_at = ?1              WHERE kind = 'hash' AND state != 'pending'",
            params![now],
        )?;
        self.0.execute(
            "INSERT OR IGNORE INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at)              SELECT 'hash', a.id, 'pending', 0, ?1, ?1 FROM assets a              WHERE a.xxhash = 0 AND a.kind IN ('photo', 'raw')                AND NOT EXISTS (SELECT 1 FROM index_tasks t                                WHERE t.kind = 'hash' AND t.asset_id = a.id)",
            params![now],
        )?;
        self.pending_index_task_count("hash")
    }

    /// 单资产 hash 任务登记（rename 快道入库后即时建；幂等）。
    pub fn create_hash_task_for(&self, asset_id: i64) -> Result<()> {
        self.0.execute(
            "INSERT OR IGNORE INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at)              VALUES ('hash', ?1, 'pending', 0, ?2, ?2)",
            params![asset_id, now_rfc3339()],
        )?;
        Ok(())
    }

    /// pHash 指纹的 4 段 16-bit 分桶插入（phash 任务成功后增量维护）。
    pub fn insert_similar_buckets(&self, id: i64, phash: u64) -> Result<()> {
        let p = phash as i64;
        self.0.execute(
            "INSERT OR IGNORE INTO similar_bucket (segment, seg_val, asset_id) VALUES \
             (0, (?1 >> 48) & 0xFFFF, ?5), (1, (?1 >> 32) & 0xFFFF, ?5), \
             (2, (?1 >> 16) & 0xFFFF, ?5), (3, ?1 & 0xFFFF, ?5)",
            params![p, p, p, p, id],
        )?;
        Ok(())
    }

    /// 桶表懒校验+全量重建（行数 ≠ 4×phash 行数时；纯 SQL，20 万行秒级）。
    /// 返回是否执行了重建。
    pub fn ensure_similar_buckets(&self) -> Result<bool, String> {
        let (phash_rows, bucket_rows): (i64, i64) = self
            .0
            .query_row(
                "SELECT (SELECT COUNT(*) FROM assets WHERE phash IS NOT NULL), \
                 (SELECT COUNT(*) FROM similar_bucket)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| e.to_string())?;
        if bucket_rows == phash_rows * 4 {
            return Ok(false);
        }
        self.0
            .execute_batch(
                "DELETE FROM similar_bucket; \
                 INSERT INTO similar_bucket (segment, seg_val, asset_id) \
                 SELECT 0, (phash >> 48) & 0xFFFF, id FROM assets WHERE phash IS NOT NULL; \
                 INSERT INTO similar_bucket (segment, seg_val, asset_id) \
                 SELECT 1, (phash >> 32) & 0xFFFF, id FROM assets WHERE phash IS NOT NULL; \
                 INSERT INTO similar_bucket (segment, seg_val, asset_id) \
                 SELECT 2, (phash >> 16) & 0xFFFF, id FROM assets WHERE phash IS NOT NULL; \
                 INSERT INTO similar_bucket (segment, seg_val, asset_id) \
                 SELECT 3, phash & 0xFFFF, id FROM assets WHERE phash IS NOT NULL;",
            )
            .map_err(|e| e.to_string())?;
        Ok(true)
    }

    /// 近重复扫描行：(asset_id, phash, pair_asset_id)。**RAW 孪生整行排除**
    /// （pair 非空且自己是 RAW——同拍摄不算重复；只排除边不够，孪生会经
    /// 第三成员间接入组）。
    pub fn similar_scan_rows(&self) -> Result<Vec<(i64, i64, Option<i64>)>> {
        let mut stmt = self.0.prepare(
            "SELECT id, phash, pair_asset_id FROM assets \
             WHERE phash IS NOT NULL AND kind IN ('photo', 'raw') AND in_trash = 0 \
               AND NOT (kind = 'raw' AND pair_asset_id IS NOT NULL) ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    }

    /// 多探针桶成员表：(segment, seg_val) → 资产 id 列表（仅 ≥2 成员的桶）。
    pub fn similar_bucket_members(&self) -> Result<Vec<SimilarBucketMembers>> {
        let mut stmt = self.0.prepare(
            "SELECT segment, seg_val, asset_id FROM similar_bucket \
             ORDER BY segment, seg_val, asset_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        let mut out: Vec<((i64, i64), Vec<i64>)> = Vec::new();
        for row in rows {
            let (seg, val, id) = row?;
            match out.last_mut() {
                Some((key, members)) if *key == (seg, val) => members.push(id),
                _ => out.push(((seg, val), vec![id])),
            }
        }
        out.retain(|(_, members)| members.len() >= 2);
        Ok(out)
    }

    /// 完全重复组：(size, xxhash) 分组计数 >1 的指纹键（count 降序）。
    pub fn exact_duplicate_keys(&self) -> Result<Vec<(i64, u64)>> {
        let mut stmt = self.0.prepare(
            "SELECT size, xxhash, COUNT(*) AS c FROM assets \
             WHERE kind IN ('photo', 'raw') AND in_trash = 0 \
             GROUP BY size, xxhash HAVING c > 1 ORDER BY c DESC, size, xxhash",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as u64))
        })?;
        rows.collect()
    }

    // —— 原片-成片版本关系（0017：photo_group / group_asset / asset_relation）——

    /// 资产所属组 id（未入组 None）。
    pub fn asset_group_of(&self, asset_id: i64) -> Result<Option<i64>> {
        self.0
            .query_row(
                "SELECT group_id FROM group_asset WHERE asset_id = ?1",
                [asset_id],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
    }

    /// 组成员表（role 升序 = raw→sooc→derived，id tiebreak；版本切换数据源）。
    pub fn group_members(&self, group_id: i64) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.0.prepare(
            "SELECT asset_id, role FROM group_asset WHERE group_id = ?1 \
             ORDER BY CASE role WHEN 'raw' THEN 0 WHEN 'sooc' THEN 1 ELSE 2 END, asset_id",
        )?;
        let rows = stmt.query_map(params![group_id], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.collect()
    }

    /// 存量回填：由既有 pair_asset_id 双向链建 photo_group（0017 配套，
    /// 幂等可重跑——已同组的配对直接跳过）。只处理 raw+photo 孪生，角色
    /// 按 kind 定（raw/sooc）。返回 (回填配对数, 新建组数)。
    /// 真库用 scripts/backfill_photo_groups.py（同语义 SQL，独立跑）。
    #[allow(dead_code)]
    pub fn backfill_photo_groups_from_pairs(&self) -> Result<(u64, u64)> {
        let pairs: Vec<(i64, i64)> = {
            let mut stmt = self.0.prepare(
                "SELECT id, pair_asset_id FROM assets \
                 WHERE pair_asset_id IS NOT NULL AND id < pair_asset_id ORDER BY id",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<Result<Vec<_>>>()?
        };
        let mut linked = 0u64;
        let mut created = 0u64;
        for (a, b) in pairs {
            let group_of = |aid: i64| -> Result<Option<i64>> {
                self.0
                    .query_row(
                        "SELECT group_id FROM group_asset WHERE asset_id = ?1",
                        [aid],
                        |r| r.get(0),
                    )
                    .map(Some)
                    .or_else(|e| match e {
                        Error::QueryReturnedNoRows => Ok(None),
                        other => Err(other),
                    })
            };
            let (ga, gb) = (group_of(a)?, group_of(b)?);
            if ga.is_some() && ga == gb {
                continue; // 已同组：幂等跳过
            }
            let kind_of = |aid: i64| -> Result<String> {
                self.0
                    .query_row("SELECT kind FROM assets WHERE id = ?1", [aid], |r| {
                        r.get::<_, String>(0)
                    })
            };
            let role_of = |k: String| -> Option<&'static str> {
                match k.as_str() {
                    "raw" => Some("raw"),
                    "photo" => Some("sooc"),
                    _ => None,
                }
            };
            let (ka, kb) = (kind_of(a)?, kind_of(b)?);
            let (Some(ra), Some(rb)) = (role_of(ka), role_of(kb)) else {
                continue; // 非 raw+photo 孪生不建组
            };
            let before: i64 = self
                .0
                .query_row("SELECT COUNT(*) FROM photo_group", [], |r| r.get(0))?;
            group_pair_on(&self.0, a, ra, b, rb)?;
            let after: i64 = self
                .0
                .query_row("SELECT COUNT(*) FROM photo_group", [], |r| r.get(0))?;
            linked += 1;
            created += (after - before).unsigned_abs();
        }
        Ok((linked, created))
    }
}
