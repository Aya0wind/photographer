//! 索引任务与分析结果仓储。

use super::*;
use rusqlite::OptionalExtension;

impl Db {
    // —— 索引任务（index_tasks）——

    /// 为缩略图状态仍未完成、但任务账缺失的资产补种待办。旧版本清理过
    /// done 行，因此不能只依赖 index_tasks 判断是否已经生成。
    pub fn create_thumb_tasks_for_unindexed(&self) -> Result<u64> {
        let now = now_rfc3339();
        let created = self.0.execute(
            "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
             SELECT 'thumb', a.id, 'pending', 0, ?1, ?1 FROM assets a \
             WHERE a.thumb_state = 0 AND a.kind IN ('photo', 'raw') \
               AND NOT EXISTS (SELECT 1 FROM index_tasks t \
                               WHERE t.kind = 'thumb' AND t.asset_id = a.id)",
            params![now],
        )?;
        Ok(created as u64)
    }

    /// 认领一条 pending 任务（原子置 running；多 worker 并发认领不重不漏，
    /// SQLite 写锁串行化保证单行独占）。无待办返回 None。
    pub fn claim_index_task(&self, kind: &str) -> Result<Option<IndexTaskRow>> {
        let now = now_rfc3339();
        let mut stmt = self.0.prepare(
            "UPDATE index_tasks SET state = 'running', updated_at = ?2 \
             WHERE id = (SELECT id FROM index_tasks WHERE state = 'pending' \
                         AND kind = ?1 ORDER BY id LIMIT 1) \
             RETURNING id, kind, asset_id, state, attempts, created_at, updated_at",
        )?;
        let mut rows = stmt.query_map(params![kind, now], map_index_task)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// 批量认领（2026-09-21 语义推理批量化）：一次认领至多 `limit` 条
    /// pending（id 升序）原子置 running——回填 worker 组批单次
    /// session.run 的前提。不足 limit 照常小批，空批返回空 Vec。
    pub fn claim_index_tasks(&self, kind: &str, limit: usize) -> Result<Vec<IndexTaskRow>> {
        let now = now_rfc3339();
        let mut stmt = self.0.prepare(
            "UPDATE index_tasks SET state = 'running', updated_at = ?2 \
             WHERE id IN (SELECT id FROM index_tasks WHERE state = 'pending' \
                          AND kind = ?1 ORDER BY id LIMIT ?3) \
             RETURNING id, kind, asset_id, state, attempts, created_at, updated_at",
        )?;
        let rows = stmt.query_map(params![kind, now, limit as i64], map_index_task)?;
        rows.collect()
    }

    /// 为语义回填建任务：`ai_indexed_at IS NULL` 的库内照片（photo/raw）
    /// 且无未完成 ai 任务的行，逐行插 pending ai 任务；返回建任务数。
    pub fn create_ai_tasks_for_unindexed(&self) -> Result<u64> {
        let now = now_rfc3339();
        let created = self.0.execute(
            "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
             SELECT 'ai', a.id, 'pending', 0, ?1, ?1 FROM assets a \
             WHERE a.ai_indexed_at IS NULL AND a.kind IN ('photo', 'raw') \
               AND NOT EXISTS (SELECT 1 FROM index_tasks t \
                               WHERE t.kind = 'ai' AND t.asset_id = a.id)",
            params![now],
        )?;
        Ok(created as u64)
    }

    /// 语义嵌入记账（usearch 落盘成功后调用）。
    pub fn set_ai_indexed(&self, id: i64) -> Result<()> {
        self.0.execute(
            "UPDATE assets SET ai_indexed_at = ?2 WHERE id = ?1",
            params![id, now_rfc3339()],
        )?;
        Ok(())
    }

    /// 语义索引的持久化进度（已记账资产 / 可索引资产）。手动与自动触发共用
    /// 同一张任务账和资产账，事件进度必须从这里的基线继续累加。
    pub fn ai_index_progress(&self) -> Result<(u64, u64)> {
        let (done, total): (i64, i64) = self.0.query_row(
            "SELECT COUNT(CASE WHEN ai_indexed_at IS NOT NULL THEN 1 END), COUNT(*) \
             FROM assets WHERE kind IN ('photo', 'raw')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok((done as u64, total as u64))
    }

    /// 语义检索 join：资产是否存在且可检索（photo/raw）。回收站资产不可检索
    /// （0016：常规链路默认排除，恢复后自动回到检索结果）。
    pub fn asset_searchable(&self, id: i64) -> Result<bool> {
        let ok: i64 = self.0.query_row(
            "SELECT COUNT(*) FROM assets WHERE id = ?1 AND kind IN ('photo', 'raw') \
             AND in_trash = 0",
            params![id],
            |r| r.get(0),
        )?;
        Ok(ok > 0)
    }

    /// 启动恢复：上次中断遗留的 running 复位为 pending，返回复位条数。
    pub fn reclaim_running_index_tasks(&self) -> Result<usize> {
        let now = now_rfc3339();
        self.0.execute(
            "UPDATE index_tasks SET state = 'pending', updated_at = ?1 WHERE state = 'running'",
            params![now],
        )
    }

    /// 指定通道待办数（pending；回填 worker 的启动判据——不能依赖本轮
    /// 新建任务数：存量 pending 也必须有人消费，否则重复 kick 全部空转）。
    pub fn pending_index_task_count(&self, kind: &str) -> Result<u64> {
        let count: i64 = self.0.query_row(
            "SELECT COUNT(*) FROM index_tasks WHERE kind = ?1 AND state = 'pending'",
            params![kind],
            |r| r.get(0),
        )?;
        Ok(count as u64)
    }

    /// RAW 缩略图源代际升级自愈：重排全部 RAW 的 thumb 任务（done/failed
    /// 复位 pending + 为无任务行的新建），返回待处理数。照片任务不动。
    pub fn requeue_thumb_tasks_for_raw(&self) -> Result<u64> {
        let now = now_rfc3339();
        self.0.execute(
            "UPDATE index_tasks SET state = 'pending', attempts = 0, updated_at = ?1 \
             WHERE kind = 'thumb' AND state != 'pending' \
             AND asset_id IN (SELECT id FROM assets WHERE kind = 'raw')",
            params![now],
        )?;
        self.0.execute(
            "INSERT OR IGNORE INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
             SELECT 'thumb', a.id, 'pending', 0, ?1, ?1 FROM assets a \
             WHERE a.kind = 'raw' \
               AND NOT EXISTS (SELECT 1 FROM index_tasks t \
                               WHERE t.kind = 'thumb' AND t.asset_id = a.id)",
            params![now],
        )?;
        self.pending_index_task_count("thumb")
    }

    /// 重建用：重排**全部** photo/raw 的 thumb 任务（既有复位 pending +
    /// 无任务行新建；配合 thumb_state=0 复位 + 缩略图目录删除）。
    pub fn requeue_thumb_tasks_for_all(&self) -> Result<u64> {
        let now = now_rfc3339();
        self.0.execute(
            "UPDATE index_tasks SET state = 'pending', attempts = 0, updated_at = ?1              WHERE kind = 'thumb' AND state != 'pending'",
            params![now],
        )?;
        self.0.execute(
            "INSERT OR IGNORE INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at)              SELECT 'thumb', a.id, 'pending', 0, ?1, ?1 FROM assets a              WHERE a.kind IN ('photo', 'raw')                AND NOT EXISTS (SELECT 1 FROM index_tasks t                                WHERE t.kind = 'thumb' AND t.asset_id = a.id)",
            params![now],
        )?;
        self.pending_index_task_count("thumb")
    }

    /// 重建用：EXIF 深提取列全清（0004 拍摄参数 + 0008 深字段全 NULL，
    /// captured_at/camera 保留——目录结构/时间线依赖它们），配合 exif 任务
    /// 重排由 worker 重新提取。
    pub fn clear_exif_columns(&self) -> Result<()> {
        self.0.execute(
            "UPDATE assets SET                  width = NULL, height = NULL, iso = NULL, f_number = NULL,                  exposure_time = NULL, focal_length = NULL, lens = NULL,                  orientation = NULL, flash = NULL, metering_mode = NULL,                  white_balance = NULL, exposure_program = NULL, software = NULL,                  artist = NULL, gps_lat = NULL, gps_lon = NULL",
            [],
        )?;
        Ok(())
    }

    /// 重建用：缩略图状态镜像全复位（配合 thumbs 目录删除）。
    pub fn reset_thumb_states(&self) -> Result<()> {
        self.0.execute("UPDATE assets SET thumb_state = 0", [])?;
        Ok(())
    }

    /// 手动“立即索引”重试：复用已有失败任务，不再为同一资产重复插行。
    /// 历史版本可能已经为同一资产制造多条 failed，先压成每资产一条；
    /// attempts 清零后仍沿用单轮最多三次的坏文件熔断策略。
    pub fn retry_failed_index_tasks(&self, kind: &str) -> Result<usize> {
        self.0.execute(
            "DELETE FROM index_tasks WHERE kind = ?1 AND state = 'failed' \
             AND id NOT IN (SELECT MAX(id) FROM index_tasks \
                            WHERE kind = ?1 AND state = 'failed' GROUP BY asset_id)",
            params![kind],
        )?;
        self.0.execute(
            "UPDATE index_tasks SET state = 'pending', attempts = 0, updated_at = ?2 \
             WHERE kind = ?1 AND state = 'failed'",
            params![kind, now_rfc3339()],
        )
    }

    /// 待办统计（pending+running；启动恢复事件/前端提示数据源）。
    pub fn pending_index_count(&self) -> Result<u64> {
        let count: i64 = self.0.query_row(
            "SELECT COUNT(*) FROM index_tasks WHERE state IN ('pending', 'running')",
            [],
            |r| r.get(0),
        )?;
        Ok(count as u64)
    }

    /// 任务收尾：done（成果落 assets 列由调用方先写）或 failed（attempts+1，
    /// 未达 3 次封顶回 pending 自动重试，达 3 次保持 failed——防坏死资产
    /// 无限循环）。
    pub fn finish_index_task(&self, id: i64, ok: bool) -> Result<()> {
        let now = now_rfc3339();
        if ok {
            self.0.execute(
                "UPDATE index_tasks SET state = 'done', updated_at = ?2 WHERE id = ?1",
                params![id, now],
            )?;
        } else {
            self.0.execute(
                "UPDATE index_tasks SET attempts = attempts + 1, updated_at = ?2, \
                 state = CASE WHEN attempts + 1 >= 3 THEN 'failed' ELSE 'pending' END \
                 WHERE id = ?1",
                params![id, now],
            )?;
        }
        Ok(())
    }

    /// 资产缩略图状态镜像更新（index worker 成果）。
    pub fn set_thumb_state(&self, id: i64, state: i32) -> Result<()> {
        self.0.execute(
            "UPDATE assets SET thumb_state = ?2 WHERE id = ?1",
            params![id, state],
        )?;
        Ok(())
    }

    /// 按需兜底通道的快路径：资产 (path, thumb_state)；无资产返回 None。
    pub fn thumb_info_by_id(&self, id: i64) -> Result<Option<(String, i32)>> {
        let mut stmt = self.0.prepare(
            "SELECT path, thumb_state FROM assets WHERE id = ?1 AND kind IN ('photo', 'raw')",
        )?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some((
                row.get(0)?,
                row.get::<_, Option<i64>>(1)?.unwrap_or(0) as i32,
            ))),
            None => Ok(None),
        }
    }

    /// 各通道任务状态计数（(kind, state, count)，index_status IPC 数据源）。
    pub fn index_task_state_counts(&self) -> Result<Vec<(String, String, u64)>> {
        let mut stmt = self.0.prepare(
            "SELECT kind, state, COUNT(DISTINCT asset_id) FROM index_tasks GROUP BY kind, state",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? as u64,
            ))
        })?;
        rows.collect()
    }

    // —— AI 辅助选片（0021：ai_analysis，eyes/blur 两通道）——

    /// 落一条分析结果（PK(asset_id, kind) upsert）。value/score 可空
    /// （eyes unknown / 计算不可得）。model_version = 算法或模型版本标签。
    pub fn set_ai_analysis(
        &self,
        asset_id: i64,
        kind: &str,
        value: Option<&str>,
        score: Option<f64>,
        model_version: &str,
    ) -> Result<()> {
        self.0.execute(
            "INSERT INTO ai_analysis (asset_id, kind, value, score, model_version, analyzed_at)              VALUES (?1, ?2, ?3, ?4, ?5, ?6)              ON CONFLICT (asset_id, kind) DO UPDATE SET              value = excluded.value, score = excluded.score,              model_version = excluded.model_version, analyzed_at = excluded.analyzed_at, details_json = NULL",
            params![asset_id, kind, value, score, model_version, now_rfc3339()],
        )?;
        Ok(())
    }

    /// 某资产的分析记录（详情 aiAnalysis 行；kind 升序）。
    pub fn ai_analysis_for(&self, asset_id: i64) -> Result<Vec<AiAnalysisRow>> {
        let mut stmt = self.0.prepare(
            "SELECT kind, value, score, model_version FROM ai_analysis              WHERE asset_id = ?1 ORDER BY kind ASC",
        )?;
        let rows = stmt.query_map(params![asset_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<f64>>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        rows.collect()
    }

    /// Summary and region evidence are committed together, never a mixture of
    /// an old summary and a new analysis after interruption.
    pub fn set_ai_analysis_details(
        &self,
        asset_id: i64,
        kind: &str,
        value: &str,
        score: Option<f64>,
        version: &str,
        details: &serde_json::Value,
    ) -> Result<()> {
        self.0.execute(
            "INSERT INTO ai_analysis (asset_id, kind, value, score, model_version, analyzed_at, details_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(asset_id, kind) DO UPDATE SET value=excluded.value, score=excluded.score,
             model_version=excluded.model_version, analyzed_at=excluded.analyzed_at, details_json=excluded.details_json",
            params![asset_id, kind, value, score, version, now_rfc3339(), details.to_string()],
        )?;
        Ok(())
    }

    pub fn ai_analysis_details(&self, asset_id: i64, kind: &str) -> Result<Option<String>> {
        self.0
            .query_row(
                "SELECT details_json FROM ai_analysis WHERE asset_id=?1 AND kind=?2",
                params![asset_id, kind],
                |r| r.get(0),
            )
            .optional()
            .map(|r| r.flatten())
    }

    /// 为闭眼通道建任务：photo/raw 且无 eyes 任务的行（任意状态——检测在
    /// eyes 任务时进行；无人脸 → done 且不产生记录，不重做）。
    pub fn create_eyes_tasks_for_unindexed(&self) -> Result<u64> {
        let now = now_rfc3339();
        let created = self.0.execute(
            "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at)              SELECT 'eyes', a.id, 'pending', 0, ?1, ?1 FROM assets a              WHERE a.kind IN ('photo', 'raw')                AND NOT EXISTS (SELECT 1 FROM index_tasks t                                WHERE t.kind = 'eyes' AND t.asset_id = a.id)",
            params![now],
        )?;
        Ok(created as u64)
    }

    /// 为失焦通道建任务：photo/raw 且无 blur 任务的行。
    pub fn create_blur_tasks_for_unindexed(&self) -> Result<u64> {
        let now = now_rfc3339();
        let created = self.0.execute(
            "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at)              SELECT 'blur', a.id, 'pending', 0, ?1, ?1 FROM assets a              WHERE a.kind IN ('photo', 'raw')                AND NOT EXISTS (SELECT 1 FROM index_tasks t                                WHERE t.kind = 'blur' AND t.asset_id = a.id)",
            params![now],
        )?;
        Ok(created as u64)
    }

    /// eyes 通道代际重排（指纹变更：模型/阈值变了——done 复位 pending +
    /// 无任务行新建）。
    pub fn requeue_eyes_tasks_for_all(&self) -> Result<u64> {
        let now = now_rfc3339();
        self.0.execute(
            "UPDATE index_tasks SET state = 'pending', attempts = 0, updated_at = ?1              WHERE kind = 'eyes' AND state != 'pending'",
            params![now],
        )?;
        self.create_eyes_tasks_for_unindexed()?;
        self.pending_index_task_count("eyes")
    }

    /// blur 通道代际重排（指纹变更：阈值/算法版本变了）。
    pub fn requeue_blur_tasks_for_all(&self) -> Result<u64> {
        let now = now_rfc3339();
        self.0.execute(
            "UPDATE index_tasks SET state = 'pending', attempts = 0, updated_at = ?1              WHERE kind = 'blur' AND state != 'pending'",
            params![now],
        )?;
        self.create_blur_tasks_for_unindexed()?;
        self.pending_index_task_count("blur")
    }

    /// EXIF 提取代际升级自愈（exif-gen-2）：重排**全部** photo/raw 资产的
    /// exif 任务（既有任务复位 pending + 为无任务行新建），返回待处理数。
    /// 唯一索引（0007）保证 INSERT OR IGNORE 不重复。
    pub fn requeue_exif_tasks_for_all(&self) -> Result<u64> {
        let now = now_rfc3339();
        self.0.execute(
            "UPDATE index_tasks SET state = 'pending', attempts = 0, updated_at = ?1 \
             WHERE kind = 'exif' AND state != 'pending'",
            params![now],
        )?;
        self.0.execute(
            "INSERT OR IGNORE INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
             SELECT 'exif', a.id, 'pending', 0, ?1, ?1 FROM assets a \
             WHERE a.kind IN ('photo', 'raw') \
               AND NOT EXISTS (SELECT 1 FROM index_tasks t \
                               WHERE t.kind = 'exif' AND t.asset_id = a.id)",
            params![now],
        )?;
        self.pending_index_task_count("exif")
    }
}
