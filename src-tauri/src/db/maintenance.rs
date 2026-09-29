//! 资产路径维护仓储。

use super::*;

impl Db {
    /// 库重定位预检（用户定案 2026-09-28 整体重定位语义）：统计 old_root
    /// 前缀下资产数与不在其下的数量，不写任何东西。
    pub fn inspect_asset_roots(&self, old_root: &str) -> Result<(u64, u64), String> {
        let mut stmt = self
            .0
            .prepare("SELECT path FROM assets")
            .map_err(|e| e.to_string())?;
        let paths: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        drop(stmt);
        let mut affected = 0u64;
        let mut unaffected = 0u64;
        for path in paths {
            if strip_root_prefix(&path, old_root).is_some() {
                affected += 1;
            } else {
                unaffected += 1;
            }
        }
        Ok((affected, unaffected))
    }

    /// 库重定位执行：把 old_root 前缀的资产路径重写为 new_root（大小写/
    /// 斜杠方向不敏感前缀匹配，尾段原样保留）。重写行同步复位 thumb_state=0
    /// 并重排 thumb 任务（缓存按路径哈希寻址，换根即失效需重建；其余通道
    /// 按 asset id 寻址不受影响）。单事务完成。返回 (重写数, 未受影响数)。
    pub fn rewrite_asset_roots(
        &self,
        old_root: &str,
        new_root: &str,
    ) -> Result<(u64, u64), String> {
        let tx = self.0.unchecked_transaction().map_err(|e| e.to_string())?;
        let mut stmt = tx
            .prepare("SELECT id, path FROM assets")
            .map_err(|e| e.to_string())?;
        let rows: Vec<(i64, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        drop(stmt);
        let mut rewritten = 0u64;
        let mut unaffected = 0u64;
        let now = now_rfc3339();
        for (id, path) in rows {
            let Some(tail) = strip_root_prefix(&path, old_root) else {
                unaffected += 1;
                continue;
            };
            let new_path = if tail.is_empty() {
                new_root.to_string()
            } else {
                format!("{new_root}\\{tail}")
            };
            tx.execute(
                "UPDATE assets SET path = ?2 WHERE id = ?1",
                params![id, new_path],
            )
            .map_err(|e| e.to_string())?;
            tx.execute(
                "UPDATE assets SET thumb_state = 0 WHERE id = ?1",
                params![id],
            )
            .map_err(|e| e.to_string())?;
            tx.execute(
                "DELETE FROM index_tasks WHERE kind = 'thumb' AND asset_id = ?1",
                params![id],
            )
            .map_err(|e| e.to_string())?;
            tx.execute(
                "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
                 VALUES ('thumb', ?1, 'pending', 0, ?2, ?2)",
                params![id, now],
            )
            .map_err(|e| e.to_string())?;
            rewritten += 1;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok((rewritten, unaffected))
    }
}
