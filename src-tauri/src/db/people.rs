//! 人物与人脸仓储。

use super::*;

impl Db {
    // —— 人脸 / 人物簇（M4，faces + people 表）——

    /// 新建未命名人物簇，返回簇 id。
    pub fn create_person(&self) -> Result<i64> {
        self.0.execute(
            "INSERT INTO people (name, cover_face_id, created_at) VALUES (NULL, NULL, ?1)",
            params![now_rfc3339()],
        )?;
        Ok(self.0.last_insert_rowid())
    }

    /// 人物重命名（空串归一为 NULL = 未命名）；簇不存在报错。
    pub fn rename_person(&self, id: i64, name: &str) -> Result<()> {
        let name = name.trim();
        let name: Option<&str> = if name.is_empty() { None } else { Some(name) };
        let n = self.0.execute(
            "UPDATE people SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        if n == 0 {
            return Err(Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    /// 删除人物簇（删簇不删脸数据：faces.cluster_id 经 FK ON DELETE SET NULL
    /// 解除归属）；簇不存在报错。
    pub fn delete_person(&self, id: i64) -> Result<()> {
        let n = self
            .0
            .execute("DELETE FROM people WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    /// 落一条人脸（SCRFD 框 + ArcFace 特征 + 簇归属），返回 face id。
    /// embedding 存 f32 小端字节流（512×4 = 2048 B blob）。
    #[allow(clippy::too_many_arguments)]
    pub fn insert_face(
        &self,
        asset_id: i64,
        box_x: f64,
        box_y: f64,
        box_w: f64,
        box_h: f64,
        embedding: &[f32],
        cluster_id: Option<i64>,
    ) -> Result<i64> {
        let blob: Vec<u8> = embedding.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.0.execute(
            "INSERT INTO faces (asset_id, box_x, box_y, box_w, box_h, embedding, cluster_id, \
             created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                asset_id,
                box_x,
                box_y,
                box_w,
                box_h,
                blob,
                cluster_id,
                now_rfc3339()
            ],
        )?;
        Ok(self.0.last_insert_rowid())
    }

    /// 封面晋升：新脸面积大于当前封面脸时替换（首张无条件担任封面）。
    pub fn maybe_promote_cover(&self, face_id: i64, cluster_id: i64, area: f64) -> Result<()> {
        self.0.execute(
            "UPDATE people SET cover_face_id = ?1 \
             WHERE id = ?2 AND (cover_face_id IS NULL OR \
               ?3 > (SELECT f.box_w * f.box_h FROM faces f \
                     WHERE f.id = people.cover_face_id))",
            params![face_id, cluster_id, area],
        )?;
        Ok(())
    }

    /// 各簇特征向量和（在线聚类簇心的增量基元）：cluster_id → (向量和, 成员数)。
    pub fn face_cluster_sums(&self) -> Result<HashMap<i64, (Vec<f32>, u32)>> {
        let mut stmt = self
            .0
            .prepare("SELECT cluster_id, embedding FROM faces WHERE cluster_id IS NOT NULL")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        let mut sums: HashMap<i64, (Vec<f32>, u32)> = HashMap::new();
        for row in rows {
            let (cluster, blob) = row?;
            let entry = sums.entry(cluster).or_insert_with(|| (Vec::new(), 0));
            let chunks: &[[u8; 4]] = blob.as_chunks().0;
            if entry.0.is_empty() {
                entry.0 = vec![0f32; chunks.len()];
            }
            for (dim, chunk) in chunks.iter().enumerate() {
                if let Some(slot) = entry.0.get_mut(dim) {
                    *slot += f32::from_le_bytes(*chunk);
                }
            }
            entry.1 += 1;
        }
        Ok(sums)
    }

    /// 每个人物最早的三张参考脸。重启后保持相同锚点，不退化为只比较簇心。
    pub fn face_cluster_anchors(&self) -> Result<HashMap<i64, Vec<Vec<f32>>>> {
        let mut stmt = self.0.prepare(
            "SELECT cluster_id, embedding FROM (SELECT cluster_id, embedding, \
             ROW_NUMBER() OVER (PARTITION BY cluster_id ORDER BY id) AS rank \
             FROM faces WHERE cluster_id IS NOT NULL) WHERE rank <= 3 ORDER BY cluster_id, rank",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        let mut anchors: HashMap<i64, Vec<Vec<f32>>> = HashMap::new();
        for row in rows {
            let (id, blob) = row?;
            let vector = blob
                .chunks_exact(4)
                .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
                .collect();
            anchors.entry(id).or_default().push(vector);
        }
        Ok(anchors)
    }

    /// 人物簇列表（face_count 降序、id 升序稳定排序；空簇不返回）。
    /// 封面资产 = 封面人脸所在资产，缺失回退簇内最大框人脸的资产。
    pub fn people_list(&self) -> Result<Vec<PersonRow>> {
        let mut stmt = self.0.prepare(
            "SELECT p.id, p.name, COUNT(DISTINCT f.asset_id), \
               (SELECT f2.asset_id FROM faces f2 WHERE f2.id = p.cover_face_id), \
               (SELECT f3.asset_id FROM faces f3 WHERE f3.cluster_id = p.id \
                 AND f3.asset_id IN (SELECT id FROM assets WHERE in_trash = 0) \
                 ORDER BY f3.box_w * f3.box_h DESC, f3.id ASC LIMIT 1) \
             FROM people p JOIN faces f ON f.cluster_id = p.id \
             JOIN assets a ON a.id = f.asset_id AND a.in_trash = 0 \
             GROUP BY p.id \
             ORDER BY COUNT(DISTINCT f.asset_id) DESC, p.id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(PersonRow {
                id: row.get(0)?,
                name: row.get(1)?,
                face_count: row.get::<_, i64>(2)? as u64,
                cover_asset_id: row
                    .get::<_, Option<i64>>(3)?
                    .or(row.get::<_, Option<i64>>(4)?),
            })
        })?;
        rows.collect()
    }

    /// 某人物簇的资产页（去重，captured_at DESC、id DESC tiebreak；NULL
    /// captured_at 最先——与画廊排序契约一致）。回收站资产不出现。
    pub fn assets_by_cluster(&self, cluster_id: i64, limit: u32) -> Result<Vec<AssetPageRow>> {
        let mut stmt = self.0.prepare(&format!(
            "SELECT {ASSET_PAGE_COLS} FROM \
             (SELECT DISTINCT {ASSET_PAGE_COLS_A}, \
                    COALESCE(a.captured_at, ?2) AS k \
                   FROM assets a JOIN faces f ON f.asset_id = a.id \
                   WHERE f.cluster_id = ?1 AND a.in_trash = 0) \
             ORDER BY k DESC, id DESC LIMIT ?3",
        ))?;
        let rows = stmt.query_map(
            params![cluster_id, CAPTURED_NULL_HIGH, limit],
            map_asset_page,
        )?;
        rows.collect()
    }

    /// 一键清除人脸数据（事务）：faces + people 清空、face 通道任务清空、
    /// assets.face_indexed_at 复位（可重新回填）。
    pub fn clear_face_data(&self) -> Result<()> {
        let tx = self.0.unchecked_transaction()?;
        tx.execute("DELETE FROM faces", [])?;
        tx.execute("DELETE FROM people", [])?;
        tx.execute("DELETE FROM index_tasks WHERE kind = 'face'", [])?;
        tx.execute("UPDATE assets SET face_indexed_at = NULL", [])?;
        tx.commit()?;
        Ok(())
    }

    /// 为人脸回填建任务：`face_indexed_at IS NULL` 的库内照片（photo/raw）
    /// 且无未完成 face 任务的行，逐行插 pending face 任务；返回建任务数。
    pub fn create_face_tasks_for_unindexed(&self) -> Result<u64> {
        let now = now_rfc3339();
        let created = self.0.execute(
            "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
             SELECT 'face', a.id, 'pending', 0, ?1, ?1 FROM assets a \
             WHERE a.face_indexed_at IS NULL AND a.kind IN ('photo', 'raw') \
               AND NOT EXISTS (SELECT 1 FROM index_tasks t \
                               WHERE t.kind = 'face' AND t.asset_id = a.id)",
            params![now],
        )?;
        Ok(created as u64)
    }

    /// 人脸处理记账（聚类入库成功后调用）。
    pub fn set_face_indexed(&self, id: i64) -> Result<()> {
        self.0.execute(
            "UPDATE assets SET face_indexed_at = ?2 WHERE id = ?1",
            params![id, now_rfc3339()],
        )?;
        Ok(())
    }
}
