//! 编辑配方与导出任务仓储。

use super::*;

impl Db {
    // —— 编辑配方（0022 edit_recipe）——

    /// 读取配方：返回 (JSON 文本, Unix 毫秒)。无配方返回 None。
    pub fn edit_recipe_get(&self, asset_id: i64) -> Result<Option<(String, i64)>> {
        let mut stmt = self
            .0
            .prepare("SELECT recipe, updated_at FROM edit_recipe WHERE asset_id = ?1")?;
        let mut rows = stmt.query_map(params![asset_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// 保存配方（PK 覆盖更新）。
    pub fn edit_recipe_upsert(&self, asset_id: i64, recipe: &str, updated_at: i64) -> Result<()> {
        self.0.execute(
            "INSERT INTO edit_recipe (asset_id, recipe, updated_at) VALUES (?1, ?2, ?3) \
             ON CONFLICT (asset_id) DO UPDATE SET recipe = excluded.recipe, \
             updated_at = excluded.updated_at",
            params![asset_id, recipe, updated_at],
        )?;
        Ok(())
    }

    /// 删除配方；返回是否删到行（幂等：无配方返回 false）。
    pub fn edit_recipe_delete(&self, asset_id: i64) -> Result<bool> {
        let n = self
            .0
            .execute("DELETE FROM edit_recipe WHERE asset_id = ?1", [asset_id])?;
        Ok(n > 0)
    }

    // —— 导出任务账（0022 export_job）——

    /// 建导出任务（status=queued），返回任务 id。
    pub fn export_job_create(&self, asset_id: i64, mode: &str) -> Result<i64> {
        self.0.execute(
            "INSERT INTO export_job (asset_id, mode, status, created_at) \
             VALUES (?1, ?2, 'queued', ?3)",
            params![asset_id, mode, now_rfc3339()],
        )?;
        Ok(self.0.last_insert_rowid())
    }

    /// 状态推进（queued→running 等；终态走 finish/fail）。
    pub fn export_job_set_status(&self, id: i64, status: &str) -> Result<()> {
        self.0.execute(
            "UPDATE export_job SET status = ?2 WHERE id = ?1",
            params![id, status],
        )?;
        Ok(())
    }

    /// 成功收尾：结果四元组 + （album 模式）新资产 id。
    pub fn export_job_finish(
        &self,
        id: i64,
        output_path: &str,
        width: u32,
        height: u32,
        bytes: u64,
        new_asset_id: Option<i64>,
    ) -> Result<()> {
        self.0.execute(
            "UPDATE export_job SET status = 'done', output_path = ?2, width = ?3, \
             height = ?4, bytes = ?5, new_asset_id = ?6, finished_at = ?7 WHERE id = ?1",
            params![
                id,
                output_path,
                width as i64,
                height as i64,
                bytes as i64,
                new_asset_id,
                now_rfc3339()
            ],
        )?;
        Ok(())
    }

    /// 失败收尾（error 终态 + 原因）。
    pub fn export_job_fail(&self, id: i64, error: &str) -> Result<()> {
        self.0.execute(
            "UPDATE export_job SET status = 'error', error = ?2, finished_at = ?3 WHERE id = ?1",
            params![id, error, now_rfc3339()],
        )?;
        Ok(())
    }

    /// 单任务读取（DTO 组装 / 测试）。
    pub fn export_job_get(&self, id: i64) -> Result<Option<ExportJobRow>> {
        let mut stmt = self.0.prepare(
            "SELECT id, asset_id, mode, status, output_path, width, height, bytes, \
             new_asset_id, error, created_at, finished_at FROM export_job WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], map_export_job)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// 非 `alive` 集合内且仍在 queued/running 的任务 id（进程重启孤儿收尸用）。
    pub fn export_job_stale_ids(&self, alive: &[i64]) -> Result<Vec<i64>> {
        let mut stmt = self
            .0
            .prepare("SELECT id FROM export_job WHERE status IN ('queued', 'running')")?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        Ok(rows
            .filter_map(Result::ok)
            .filter(|id| !alive.contains(id))
            .collect())
    }

    // —— 相册导出任务账（M6 album_export_job，§六 LR 互操作）——

    /// 建相册导出任务（status=queued，total 预填成员数），返回任务 id。
    pub fn album_export_job_create(
        &self,
        album_id: i64,
        subgroup: Option<&str>,
        output_dir: &str,
        total: u64,
    ) -> Result<i64> {
        self.0.execute(
            "INSERT INTO album_export_job (album_id, subgroup, output_dir, status, total, \
             created_at) VALUES (?1, ?2, ?3, 'queued', ?4, ?5)",
            params![album_id, subgroup, output_dir, total as i64, now_rfc3339()],
        )?;
        Ok(self.0.last_insert_rowid())
    }

    /// 状态推进（queued→running；终态走 finish）。
    pub fn album_export_job_set_status(&self, id: i64, status: &str) -> Result<()> {
        self.0.execute(
            "UPDATE album_export_job SET status = ?2 WHERE id = ?1",
            params![id, status],
        )?;
        Ok(())
    }

    /// 进度落库（绝对值计数；upsert 形态不必——行已存在，仅更新计数列）。
    pub fn album_export_job_progress(&self, id: i64, done: u64, linked: u64) -> Result<()> {
        self.0.execute(
            "UPDATE album_export_job SET done = ?2, linked = ?3 WHERE id = ?1",
            params![id, done as i64, linked as i64],
        )?;
        Ok(())
    }

    /// 收尾（done|cancelled|error + 终值计数；error 为 None 时清空）。
    pub fn album_export_job_finish(
        &self,
        id: i64,
        status: &str,
        done: u64,
        linked: u64,
        error: Option<&str>,
    ) -> Result<()> {
        self.0.execute(
            "UPDATE album_export_job SET status = ?2, done = ?3, linked = ?4, error = ?5, \
             finished_at = ?6 WHERE id = ?1",
            params![id, status, done as i64, linked as i64, error, now_rfc3339()],
        )?;
        Ok(())
    }

    /// 最近一次任务行（album_export_status 数据源；无任务 None）。
    pub fn album_export_job_latest(&self) -> Result<Option<AlbumExportJobRow>> {
        self.0
            .query_row(
                "SELECT id, album_id, subgroup, output_dir, status, total, done, linked, error, \
                 created_at, finished_at FROM album_export_job ORDER BY id DESC LIMIT 1",
                [],
                map_album_export_job,
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
    }

    /// 相册导出版孤儿收尸清单（queued/running 且不在活跃集合）。
    pub fn album_export_job_stale_ids(&self, alive: &[i64]) -> Result<Vec<i64>> {
        let mut stmt = self
            .0
            .prepare("SELECT id FROM album_export_job WHERE status IN ('queued', 'running')")?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        Ok(rows
            .filter_map(Result::ok)
            .filter(|id| !alive.contains(id))
            .collect())
    }
}
