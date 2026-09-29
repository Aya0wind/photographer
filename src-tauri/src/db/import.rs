//! 导入任务与日志仓储。

use super::*;

impl Db {
    /// 建导入任务（status='running'，started_at=now），返回 job_id。
    /// （引擎走 [`Db::create_job_with_plan`]；本方法保留为仓储基元，测试覆盖。）
    #[allow(dead_code)]
    pub fn create_job(
        &self,
        kind: &str,
        device_id: &str,
        device_name: &str,
        total_files: u64,
        total_bytes: u64,
    ) -> Result<i64> {
        self.0.execute(
            "INSERT INTO jobs (kind, device_id, device_name, status, total_files, total_bytes, \
             started_at) VALUES (?1, ?2, ?3, 'running', ?4, ?5, ?6)",
            params![
                kind,
                device_id,
                device_name,
                total_files as i64,
                total_bytes as i64,
                now_rfc3339()
            ],
        )?;
        Ok(self.0.last_insert_rowid())
    }

    /// 建导入任务并保存 ImportPlan JSON（断点恢复/失败重试时重建引擎）。
    pub fn create_job_with_plan(
        &self,
        kind: &str,
        device_id: &str,
        device_name: &str,
        total_files: u64,
        total_bytes: u64,
        plan_json: &str,
    ) -> Result<i64> {
        self.0.execute(
            "INSERT INTO jobs (kind, device_id, device_name, status, total_files, total_bytes, \
             plan_json, started_at) VALUES (?1, ?2, ?3, 'running', ?4, ?5, ?6, ?7)",
            params![
                kind,
                device_id,
                device_name,
                total_files as i64,
                total_bytes as i64,
                plan_json,
                now_rfc3339()
            ],
        )?;
        Ok(self.0.last_insert_rowid())
    }

    /// 读取任务的 ImportPlan JSON（无则 None）。
    pub fn job_plan_json(&self, job_id: i64) -> Result<Option<String>> {
        let mut stmt = self.0.prepare("SELECT plan_json FROM jobs WHERE id = ?1")?;
        let mut rows = stmt.query(params![job_id])?;
        match rows.next()? {
            Some(row) => Ok(row.get(0)?),
            None => Err(Error::QueryReturnedNoRows),
        }
    }

    /// 任务的设备标识（resume 校验源一致性）。
    pub fn job_device(&self, job_id: i64) -> Result<Option<(String, String)>> {
        let mut stmt = self
            .0
            .prepare("SELECT device_id, device_name FROM jobs WHERE id = ?1")?;
        let mut rows = stmt.query_map(params![job_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// 收尾任务：写终态（受 status CHECK 约束）、stats_json、finished_at。
    pub fn finish_job(&self, job_id: i64, status: &str, stats_json: &str) -> Result<()> {
        self.0.execute(
            "UPDATE jobs SET status = ?2, stats_json = ?3, finished_at = ?4 WHERE id = ?1",
            params![job_id, status, stats_json, now_rfc3339()],
        )?;
        Ok(())
    }

    /// journal 落状态：PK(job_id, src) 冲突时整行覆盖。
    pub fn upsert_job_file(&self, row: &JobFileRow) -> Result<()> {
        self.0.execute(
            "INSERT INTO job_files (job_id, src, dst, size, state, error, xxhash, dst2) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT (job_id, src) DO UPDATE SET \
             dst = excluded.dst, size = excluded.size, state = excluded.state, \
             error = excluded.error, xxhash = excluded.xxhash, \
             dst2 = excluded.dst2",
            params![
                row.job_id,
                row.src,
                row.dst,
                row.size as i64,
                row.state,
                row.error,
                row.xxhash.map(|v| v as i64),
                row.dst2
            ],
        )?;
        Ok(())
    }

    /// 断点恢复：取 pending / failed 的文件（按 src 有序）。
    /// （引擎用 [`Db::all_job_files`] 自行分流；保留为仓储基元，测试覆盖。）
    #[allow(dead_code)]
    pub fn pending_job_files(&self, job_id: i64) -> Result<Vec<JobFileRow>> {
        let mut stmt = self.0.prepare(
            "SELECT job_id, src, dst, size, state, error, xxhash, dst2 FROM job_files \
             WHERE job_id = ?1 AND state IN (?2, ?3) ORDER BY src",
        )?;
        let rows = stmt.query_map(
            params![job_id, FileState::Pending, FileState::Failed],
            map_job_file,
        )?;
        rows.collect()
    }

    /// 任务全部 journal 行（resume 重建统计基线；按 src 有序）。
    pub fn all_job_files(&self, job_id: i64) -> Result<Vec<JobFileRow>> {
        let mut stmt = self.0.prepare(
            "SELECT job_id, src, dst, size, state, error, xxhash, dst2 FROM job_files \
             WHERE job_id = ?1 ORDER BY src",
        )?;
        let rows = stmt.query_map(params![job_id], map_job_file)?;
        rows.collect()
    }

    /// 按状态分组计数（总结弹窗 / 进度统计）。
    #[allow(dead_code)]
    pub fn job_file_counts(&self, job_id: i64) -> Result<Vec<(FileState, u64)>> {
        let mut stmt = self.0.prepare(
            "SELECT state, COUNT(*) FROM job_files WHERE job_id = ?1 GROUP BY state ORDER BY state",
        )?;
        let rows = stmt.query_map(params![job_id], |row| {
            Ok((row.get::<_, FileState>(0)?, row.get::<_, i64>(1)? as u64))
        })?;
        rows.collect()
    }

    /// 任务列表 keyset 分页：id 严格大于 after_id，升序取 limit 条。
    pub fn jobs_page(&self, after_id: i64, limit: u32) -> Result<Vec<JobRow>> {
        let mut stmt = self.0.prepare(
            "SELECT id, kind, device_id, device_name, status, total_files, total_bytes, \
             stats_json, started_at, finished_at FROM jobs \
             WHERE id > ?1 ORDER BY id ASC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![after_id, limit], |row| {
            Ok(JobRow {
                id: row.get(0)?,
                kind: row.get(1)?,
                device_id: row.get(2)?,
                device_name: row.get(3)?,
                status: row.get(4)?,
                total_files: row.get::<_, i64>(5)? as u64,
                total_bytes: row.get::<_, i64>(6)? as u64,
                stats_json: row.get(7)?,
                started_at: row.get(8)?,
                finished_at: row.get(9)?,
            })
        })?;
        rows.collect()
    }

    /// 失败重试：把 old 任务中 failed 的文件复制为 new 任务的 pending 行
    /// （沿用 kind/device/plan），返回 new job_id；无 failed 行返回 None。
    pub fn retry_failed_into_new_job(&self, old_job_id: i64) -> Result<Option<i64>> {
        let mut stmt = self.0.prepare(
            "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM job_files \
             WHERE job_id = ?1 AND state = 'failed'",
        )?;
        let (count, bytes): (i64, i64) =
            stmt.query_row(params![old_job_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        if count == 0 {
            return Ok(None);
        }
        let tx = self.0.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO jobs (kind, device_id, device_name, status, total_files, total_bytes, \
             plan_json, started_at) \
             SELECT kind, device_id, device_name, 'running', ?2, ?3, plan_json, ?4 \
             FROM jobs WHERE id = ?1",
            params![old_job_id, count, bytes, now_rfc3339()],
        )?;
        let new_id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO job_files (job_id, src, dst, size, state) \
             SELECT ?2, src, dst, size, 'pending' FROM job_files \
             WHERE job_id = ?1 AND state = 'failed'",
            params![old_job_id, new_id],
        )?;
        tx.commit()?;
        Ok(Some(new_id))
    }

    /// 追加日志（ts=now）。
    pub fn append_log(&self, level: &str, job_id: Option<i64>, msg: &str) -> Result<()> {
        self.0.execute(
            "INSERT INTO logs (ts, level, job_id, message) VALUES (?1, ?2, ?3, ?4)",
            params![now_rfc3339(), level, job_id, msg],
        )?;
        Ok(())
    }

    /// 启动自愈：孤儿导入任务终老（2026-09-21）。进程重启后引擎会话天然
    /// 清零，jobs 表遗留的 running/paused 是跨会话死任务（真库 job 18
    /// running 挂 3 天、任务抽屉删不掉）——统一转 cancelled、补 finished_at，
    /// 每任务另写一行日志。journal 行不动：resume_import 不校验终态，设备
    /// 回连后仍可从 journal 手动续传（终老只清「死状态」，不毁恢复语义）。
    /// 只动 kind='import'：photo-root-migrate 任务有自己的跨会话续跑语义
    /// （db_dir_migrate 时显式恢复未完成任务），不得误伤。
    /// 返回终老的 job id 列表（空 = 无孤儿，幂等）。
    pub fn reap_orphan_import_jobs(&self) -> Result<Vec<i64>> {
        let now = now_rfc3339();
        let tx = self.0.unchecked_transaction()?;
        let ids: Vec<i64> = {
            let mut stmt = tx.prepare(
                "UPDATE jobs SET status = 'cancelled', finished_at = ?1 \
                 WHERE kind = 'import' AND status IN ('running', 'paused') \
                 RETURNING id",
            )?;
            let rows = stmt.query_map(params![now], |r| r.get(0))?;
            rows.collect::<std::result::Result<Vec<i64>, _>>()?
        };
        for id in &ids {
            tx.execute(
                "INSERT INTO logs (ts, level, job_id, message) VALUES (?1, 'warn', ?2, ?3)",
                params![
                    now,
                    id,
                    "进程重启：孤儿任务无活跃引擎会话，自动终老为 cancelled（journal 保留，设备回连后可手动恢复）"
                ],
            )?;
        }
        tx.commit()?;
        Ok(ids)
    }

    /// 删除任务历史（用户语义：这条历史连同日志一起消失）。
    /// 非终态（running/paused）拒绝；终态（done/cancelled/failed）删
    /// logs + jobs（job_files 经 FK ON DELETE CASCADE 级联清）。
    /// 返回 Ok(false) = 任务不存在；Err = 进行中。
    pub fn delete_job_history(&self, job_id: i64) -> Result<Result<bool, String>> {
        let status: Option<String> = self
            .0
            .query_row("SELECT status FROM jobs WHERE id = ?1", [job_id], |r| {
                r.get(0)
            })
            .map(Some)
            .or_else(|e| match e {
                Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        let Some(status) = status else {
            return Ok(Ok(false)); // 不存在：幂等删除
        };
        if !matches!(status.as_str(), "done" | "cancelled" | "failed") {
            return Ok(Err("任务进行中，无法删除".into()));
        }
        let tx = self.0.unchecked_transaction()?;
        tx.execute("DELETE FROM logs WHERE job_id = ?1", [job_id])?;
        tx.execute("DELETE FROM jobs WHERE id = ?1", [job_id])?; // job_files 级联
        tx.commit()?;
        Ok(Ok(true))
    }

    /// 日志游标分页：job 内 id 严格大于 after_id，升序取 limit 条。
    pub fn logs_page(&self, job_id: i64, after_id: i64, limit: u32) -> Result<Vec<LogRow>> {
        let mut stmt = self.0.prepare(
            "SELECT id, ts, level, job_id, message FROM logs \
             WHERE job_id = ?1 AND id > ?2 ORDER BY id ASC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![job_id, after_id, limit], |row| {
            Ok(LogRow {
                id: row.get(0)?,
                ts: row.get(1)?,
                level: row.get(2)?,
                job_id: row.get(3)?,
                message: row.get(4)?,
            })
        })?;
        rows.collect()
    }
}
