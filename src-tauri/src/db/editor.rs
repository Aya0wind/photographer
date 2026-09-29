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
}
