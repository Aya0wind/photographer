//! 选片标记与回收站仓储。

use super::*;

impl Db {
    /// 收藏旗标写入（0/1；资产不存在返回 false）。
    pub fn set_asset_flagged(&self, id: i64, flagged: bool) -> Result<bool> {
        let n = self.0.execute(
            "UPDATE assets SET flagged = ?2 WHERE id = ?1",
            params![id, i64::from(flagged)],
        )?;
        Ok(n > 0)
    }

    // —— 选片状态（0016：颜色标签 / 拒绝；批量）——

    /// 批量写颜色标签（token 已由 IPC 层校验；None = 清除）。
    /// 返回实际更新行数（失效 id 自然不计）。同语句置 `xmp_dirty=1`
    /// （§五 M2c 写方向闭环：改动先落库并记账，边车写成功后由调用方清脏；
    /// 离线/缺失期间边车写不出去 → 脏标志留待库扫描补写）。XMP 写回由
    /// 调用方派发。
    pub fn assets_label_set(&self, ids: &[i64], label: Option<&str>) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let slots = (0..ids.len())
            .map(|i| format!("?{}", i + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let n = self.0.execute(
            &format!("UPDATE assets SET color_label = ?1, xmp_dirty = 1 WHERE id IN ({slots})"),
            rusqlite::params_from_iter(
                std::iter::once(rusqlite::types::Value::from(label.map(str::to_string)))
                    .chain(ids.iter().map(|id| rusqlite::types::Value::from(*id))),
            ),
        )?;
        Ok(n as u64)
    }

    /// 批量写接受/拒绝状态（布尔语义 0/1；同语句置 `xmp_dirty=1`，语义同
    /// [`Db::assets_label_set`]）。返回实际更新行数。
    pub fn assets_reject_set(&self, ids: &[i64], rejected: bool) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let slots = (0..ids.len())
            .map(|i| format!("?{}", i + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let n = self.0.execute(
            &format!("UPDATE assets SET rejected = ?1, xmp_dirty = 1 WHERE id IN ({slots})"),
            rusqlite::params_from_iter(
                std::iter::once(i64::from(rejected)).chain(ids.iter().copied()),
            ),
        )?;
        Ok(n as u64)
    }

    // —— 应用内回收站（0016：软删标记 + 显式恢复/清除）——

    /// 移入回收站（幂等：已在站的行不动、trashed_at 不刷新）。返回新移入数。
    pub fn assets_trash_move(&self, ids: &[i64]) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let slots = (0..ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let n = self.0.execute(
            &format!(
                "UPDATE assets SET in_trash = 1, trashed_at = ?{0} \
                 WHERE id IN ({slots}) AND in_trash = 0",
                ids.len() + 1
            ),
            rusqlite::params_from_iter(
                ids.iter()
                    .map(|id| rusqlite::types::Value::from(*id))
                    .chain(std::iter::once(rusqlite::types::Value::from(now_rfc3339()))),
            ),
        )?;
        Ok(n as u64)
    }

    /// 回收站列表（trashed_at DESC、id DESC keyset；复用画廊分页行结构）。
    /// 游标 after_id 为上一页末行 id（0 = 第一页；行已删/已恢复按第一页）。
    pub fn trash_list(&self, after_id: i64, limit: u32) -> Result<Vec<AssetPageRow>> {
        let (cursor_key, cursor_id) = if after_id > 0 {
            match self.0.query_row(
                "SELECT trashed_at FROM assets WHERE id = ?1 AND in_trash = 1",
                [after_id],
                |r| r.get::<_, String>(0),
            ) {
                Ok(key) => (key, after_id),
                Err(_) => (now_rfc3339(), 0), // 游标失效：回退第一页
            }
        } else {
            (now_rfc3339(), 0)
        };
        // 哨兵 = 当前时刻：trashed_at 恒不晚于 now，第一页必含最新行
        let mut stmt = self.0.prepare(&format!(
            "SELECT {ASSET_PAGE_COLS} FROM assets \
             WHERE in_trash = 1 AND kind IN ('photo', 'raw') \
               AND (?2 = 0 OR trashed_at < ?1 OR (trashed_at = ?1 AND id < ?2)) \
             ORDER BY trashed_at DESC, id DESC LIMIT ?3",
        ))?;
        let rows = stmt.query_map(params![cursor_key, cursor_id, limit], map_asset_page)?;
        rows.collect()
    }

    /// 回收站还原（幂等）。返回还原行数。
    /// 保留尚存的相册引用；没有相册的照片还原后仍然没有相册。

    pub fn trash_restore(&self, ids: &[i64]) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let slots = (0..ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let n = self.0.execute(
            &format!(
                "UPDATE assets SET in_trash = 0, trashed_at = NULL \
                 WHERE id IN ({slots}) AND in_trash = 1"
            ),
            rusqlite::params_from_iter(ids.iter().copied()),
        )? as u64;
        Ok(n)
    }

    /// 回收站内资产行（purge 分类清单，§五 M2c 清空回收站语义）：
    /// (id, path, origin, missing, library_id, library_status, library_name)。
    /// library_* 三列 LEFT JOIN photos_libraries——library_id 为 NULL 的行
    /// （历史/外部引用）status 视同 online。物理删除决策由调用方按
    /// 在线/缺失/外部三态分类。
    pub fn trash_entries(
        &self,
        ids: &[i64],
    ) -> Result<
        Vec<(
            i64,
            String,
            String,
            bool,
            Option<String>,
            bool,
            Option<String>,
        )>,
    > {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let slots = (0..ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let mut stmt = self.0.prepare(&format!(
            "SELECT a.id, a.path, a.origin, a.missing != 0, a.library_id, \
             COALESCE(l.status, 'online') = 'online', l.name \
             FROM assets a LEFT JOIN photos_libraries l ON l.id = a.library_id \
             WHERE a.id IN ({slots}) AND a.in_trash = 1"
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, bool>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        rows.collect()
    }

    /// 资产记录级删除（duplicate_delete / trash_purge 共用路径）：事务内
    /// 清 pair 双向引用 → 删行（album_item/faces/view_history/similar_bucket/
    /// index_tasks 等经既有 FK ON DELETE CASCADE 级联）→ 孤儿人物簇清理
    ///（faces 级联后无脸挂靠的 person，[`delete_orphan_people`]）→ 清空组/
    /// 空连拍壳。返回删除数。物理文件与派生数据（缩略图/向量）删除由调用
    /// 方负责（失败容忍记账，见 ipc::duplicates / ipc::selection）。
    pub fn assets_delete_rows(&self, ids: &[i64]) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let slots = (0..ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let tx = self.0.unchecked_transaction()?;
        // 孤儿簇记账先行：被删资产 faces 上挂的簇 id（删行后即查不到了）
        let clusters = clusters_on_assets(&tx, ids)?;
        tx.execute(
            &format!("UPDATE assets SET pair_asset_id = NULL WHERE pair_asset_id IN ({slots})"),
            rusqlite::params_from_iter(ids.iter()),
        )?;
        let n = tx.execute(
            &format!("DELETE FROM assets WHERE id IN ({slots})"),
            rusqlite::params_from_iter(ids.iter()),
        )?;
        delete_orphan_people(&tx, &clusters)?;
        // 空组/空连拍壳清理（group_asset/burst_id 随资产级联后，可能留下
        // 无成员壳行；bursts 整体重建型账本，壳行只碍统计不碍正确性）
        tx.execute(
            "DELETE FROM photo_group WHERE id NOT IN (SELECT DISTINCT group_id FROM group_asset)",
            [],
        )?;
        tx.execute(
            "DELETE FROM bursts \
             WHERE id NOT IN (SELECT DISTINCT burst_id FROM assets WHERE burst_id IS NOT NULL)",
            [],
        )?;
        tx.commit()?;
        Ok(n as u64)
    }
}

/// 被删资产 faces 上挂的簇 id（去重；孤儿人物清理的输入）。只取确有
/// 挂靠的簇——本就无脸的簇不受本次删除影响，不进清理候选。
pub(super) fn clusters_on_assets(conn: &Connection, ids: &[i64]) -> Result<Vec<i64>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let slots = (0..ids.len())
        .map(|i| format!("?{}", i + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = conn.prepare(&format!(
        "SELECT DISTINCT cluster_id FROM faces WHERE cluster_id IS NOT NULL AND asset_id IN ({slots})"
    ))?;
    let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |r| r.get(0))?;
    rows.collect()
}

/// 孤儿人物簇清理（删资产后的共用收尾）：候选簇中已无任何 faces 挂靠的
/// person 删除（faces 级联清空 = 该人物只剩空壳）；仍被其他库资产使用
/// （还有脸）的簇保留。剩余人物若封面脸被级联删，cover_face_id 已由 FK
/// ON DELETE SET NULL 归空，人物页封面回退簇内最大框的既有路径。
pub(super) fn delete_orphan_people(conn: &Connection, clusters: &[i64]) -> Result<()> {
    if clusters.is_empty() {
        return Ok(());
    }
    let slots = (0..clusters.len())
        .map(|i| format!("?{}", i + 1))
        .collect::<Vec<_>>()
        .join(", ");
    conn.execute(
        &format!(
            "DELETE FROM people WHERE id IN ({slots}) \
             AND NOT EXISTS (SELECT 1 FROM faces WHERE cluster_id = people.id)"
        ),
        rusqlite::params_from_iter(clusters.iter()),
    )?;
    Ok(())
}
