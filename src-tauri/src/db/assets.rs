//! 资产读写与查询仓储。

use super::*;

// The display order and both paging directions must agree: unknown dates are last.
const CAPTURED_UNKNOWN_LAST: &str = "";

impl Db {
    /// 画廊 keyset 分页：序 = (COALESCE(captured_at, 哨兵) DESC, id DESC)，
    /// 拍摄时间降序、id 倒序 tiebreak，NULL captured_at 最后。
    /// 游标 after_id 为上一页末行 id（0 = 第一页）；游标行已不存在时按第一页
    /// 处理。全参数化：所有过滤条件经动态槽位构造（`?N` 显式编号 + 同序
    /// push，绑定按编号而非文本位置），零值拼接。M5 扩展条件的 NULL 语义见
    /// [`AssetFilters`] 注释。
    pub fn assets_page(
        &self,
        after_id: i64,
        limit: u32,
        filters: &AssetFilters,
    ) -> Result<Vec<AssetPageRow>> {
        self.assets_page_cursor(after_id, limit, filters, false, false)
    }

    /// Seek directly to an anchor, or fetch the nearest newer page for upward scrolling.
    pub fn assets_seek(
        &self,
        anchor_id: i64,
        limit: u32,
        filters: &AssetFilters,
        before: bool,
    ) -> Result<Vec<AssetPageRow>> {
        if anchor_id <= 0 || self.sort_key_of(anchor_id)?.is_none() {
            return Ok(Vec::new());
        }
        self.assets_page_cursor(anchor_id, limit, filters, !before, before)
    }

    fn assets_page_cursor(
        &self,
        after_id: i64,
        limit: u32,
        filters: &AssetFilters,
        inclusive: bool,
        before: bool,
    ) -> Result<Vec<AssetPageRow>> {
        // 游标键解析：after 行的归一排序键（NULL → 低哨兵）
        let (cursor_key, cursor_id) = if after_id > 0 {
            match self.sort_key_of(after_id)? {
                Some(key) => (key, after_id),
                None => (CAPTURED_UNKNOWN_LAST.to_string(), 0), // 行已删：回退第一页
            }
        } else {
            (CAPTURED_UNKNOWN_LAST.to_string(), 0)
        };
        use rusqlite::types::Value as V;
        let mut params_vec: Vec<V> = Vec::new();
        // 统一槽位构造：?N 显式编号 + params_vec 同序 push（按编号绑定）
        let slot = |params_vec: &mut Vec<V>, v: V| -> String {
            let s = format!("?{}", params_vec.len() + 1);
            params_vec.push(v);
            s
        };

        // —— 排序哨兵（子查询内 COALESCE 的 NULL 归一）——
        let sentinel_slot = slot(&mut params_vec, V::from(CAPTURED_UNKNOWN_LAST.to_string()));

        let conds = asset_filter_conditions(filters, &mut params_vec);

        // —— 游标（归一排序键二元组；cursor_id=0 即第一页短路）——
        let cursor_id_slot = slot(&mut params_vec, V::from(cursor_id));
        let cursor_key_slot = slot(&mut params_vec, V::from(cursor_key));
        let limit_slot = slot(&mut params_vec, V::from(limit));

        let all = if conds.is_empty() {
            "1 = 1".to_string()
        } else {
            conds.join(" AND ")
        };
        // 过滤条件作用于内层（可引用 assets 全列），游标/排序用外层投影列
        let compare = if before { ">" } else { "<" };
        let id_compare = if inclusive { "<=" } else { compare };
        let order = if before { "ASC" } else { "DESC" };
        let mut stmt = self.0.prepare(&format!(
            "SELECT {ASSET_PAGE_COLS} FROM \
             (SELECT {ASSET_PAGE_COLS_A}, \
              COALESCE(a.captured_at, {sentinel_slot}) AS k \
              FROM assets a WHERE {all}) \
             WHERE ({cursor_id_slot} = 0 OR k {compare} {cursor_key_slot} \
                    OR (k = {cursor_key_slot} AND id {id_compare} {cursor_id_slot})) \
             ORDER BY k {order}, id {order} LIMIT {limit_slot}",
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params_vec), map_asset_page)?;
        let mut result = rows.collect::<Result<Vec<_>>>()?;
        if before {
            result.reverse();
        }
        Ok(result)
    }

    /// 当前筛选条件的真实总数，与 assets_page 复用完全相同的 SQL 条件。
    pub fn assets_count(&self, filters: &AssetFilters) -> Result<u64> {
        use rusqlite::types::Value as V;
        let mut params_vec: Vec<V> = Vec::new();
        let conditions = asset_filter_conditions(filters, &mut params_vec);
        let where_sql = if conditions.is_empty() {
            "1 = 1".to_string()
        } else {
            conditions.join(" AND ")
        };
        let count: i64 = self.0.query_row(
            &format!("SELECT COUNT(*) FROM assets a WHERE {where_sql}"),
            rusqlite::params_from_iter(params_vec),
            |row| row.get(0),
        )?;
        Ok(count as u64)
    }

    /// 最近添加分页（「最近添加」页数据源）：created_at DESC、id DESC
    /// keyset——created_at NOT NULL 定宽 RFC3339，字典序即时间序；游标
    /// after_id 为上一页末行 id（0 = 第一页；行已删按第一页）。回收站资产
    /// 不出现（in_trash=0）。
    pub fn recent_assets_page(&self, after_id: i64, limit: u32) -> Result<Vec<AssetPageRow>> {
        let (cursor_key, cursor_id) = if after_id > 0 {
            match self.0.query_row(
                "SELECT created_at FROM assets WHERE id = ?1",
                [after_id],
                |r| r.get::<_, String>(0),
            ) {
                Ok(key) => (key, after_id),
                Err(_) => ("9999-12-31T23:59:59.999Z".to_string(), 0), // 行已删：回退第一页
            }
        } else {
            ("9999-12-31T23:59:59.999Z".to_string(), 0)
        };
        let mut stmt = self.0.prepare(&format!(
            "SELECT {ASSET_PAGE_COLS} FROM \
             (SELECT {ASSET_PAGE_COLS_A}, a.created_at AS ck FROM assets a \
              WHERE a.in_trash = 0 AND a.kind IN ('photo', 'raw')) \
             WHERE (?1 = 0 OR ck < ?2 OR (ck = ?2 AND id < ?1)) \
             ORDER BY ck DESC, id DESC LIMIT ?3",
        ))?;
        let rows = stmt.query_map(params![cursor_id, cursor_key, limit], map_asset_page)?;
        rows.collect()
    }

    /// 某资产 id 的归一排序键（行不存在返回 None）。
    fn sort_key_of(&self, id: i64) -> Result<Option<String>> {
        let mut stmt = self
            .0
            .prepare("SELECT COALESCE(captured_at, ?2) FROM assets WHERE id = ?1")?;
        let mut rows = stmt.query(params![id, CAPTURED_UNKNOWN_LAST])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    /// 本地时区日期分组（降序；unknown 组沉底，与画廊分页一致）。
    /// date() 无值（NULL）→ 'unknown'；cover 取组内同排序首张
    /// （captured DESC、id DESC）。回收站资产不计（in_trash=0）。
    pub fn asset_group_dates(&self) -> Result<Vec<DateGroupRow>> {
        self.asset_group_dates_filtered(&AssetFilters::default())
    }

    pub fn asset_group_dates_filtered(&self, filters: &AssetFilters) -> Result<Vec<DateGroupRow>> {
        use rusqlite::types::Value as V;
        let mut values: Vec<V> = vec![V::from(CAPTURED_UNKNOWN_LAST.to_string())];
        let conditions = asset_filter_conditions(filters, &mut values).join(" AND ");
        // One filtered scan; cover and counts share the same scope, including albums.
        let mut stmt = self.0.prepare(&format!(
            "WITH dated AS (SELECT a.id, COALESCE(date(a.captured_at, 'localtime'), 'unknown') AS day, \
             ROW_NUMBER() OVER (PARTITION BY COALESCE(date(a.captured_at, 'localtime'), 'unknown') \
             ORDER BY COALESCE(a.captured_at, ?1) DESC, a.id DESC) AS rn \
             FROM assets a WHERE {conditions}) \
             SELECT day, COUNT(*), MAX(CASE WHEN rn = 1 THEN id END) FROM dated \
             GROUP BY day ORDER BY (day = 'unknown') ASC, day DESC"
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(values), |row| {
            Ok(DateGroupRow {
                date: row.get(0)?,
                count: row.get::<_, i64>(1)? as u64,
                cover_asset_id: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    /// 按 id 取完整资产行（详情数据源）。
    pub fn asset_by_id(&self, id: i64) -> Result<Option<AssetRow>> {
        let mut stmt = self.0.prepare(
            "SELECT path, filename, size, mtime, xxhash, kind, captured_at, camera, \
             source, created_at, origin, width, height, iso, f_number, exposure_time, \
             focal_length, lens, pair_asset_id, thumb_state, orientation, flash, \
             metering_mode, white_balance, exposure_program, software, artist, \
             gps_lat, gps_lon, rating, flagged, color_label, rejected, library_id, \
             missing, xmp_dirty, volume_serial, file_id FROM assets WHERE id = ?1 AND kind IN ('photo', 'raw')",
        )?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(map_asset_full(row)?)),
            None => Ok(None),
        }
    }

    /// 库内同指纹 (size, xxhash) 的**其他**资产数（不含自身）。回收站资产
    /// 不计（详情页重复计数与画廊一致口径）。
    pub fn asset_duplicate_count(&self, id: i64, size: u64, xxhash: u64) -> Result<u64> {
        let mut stmt = self.0.prepare(
            "SELECT COUNT(*) FROM assets WHERE size = ?1 AND xxhash = ?2 AND id != ?3 \
             AND in_trash = 0 AND kind IN ('photo', 'raw')",
        )?;
        let count: i64 = stmt.query_row(params![size as i64, xxhash as i64, id], |r| r.get(0))?;
        Ok(count as u64)
    }

    /// 资产入库：同路径重复导入整行覆盖（REPLACE 换 id 时自动重指
    /// pair_asset_id 既有引用），随后按 (同目录, 同 stem, 异扩展名) 双向
    /// 写 RAW/JPG 配对（pair_asset_id）。
    /// （引擎走 [`Db::insert_asset_with_album`]；本方法保留为仓储基元，
    /// 测试覆盖——与 create_job 同款约定。）
    #[allow(dead_code)]
    pub fn insert_asset(&self, a: &AssetRow) -> Result<()> {
        insert_asset_on(&self.0, a, None, None)
    }

    /// 资产入库 + 同事务挂相册（导入引擎的 album_id 通道，0015）：资产行、
    /// 配对、索引待办与 album_item 引用原子落库——中断恢复时要么资产与
    /// 引用都在、要么都不在，INSERT OR IGNORE 保证 resume 重放幂等。
    /// 相册在导入期间被删除（用户侧并发操作）时**跳过挂载不报错**：
    /// 文件已安全复制落盘，不因相册消失判整个文件失败（journal 不留假失败）。
    pub fn insert_asset_with_album(
        &self,
        a: &AssetRow,
        album_id: Option<i64>,
        album_subgroup: Option<&str>,
    ) -> Result<()> {
        // 导入和索引同时写库：先取得写锁再查询，避免 deferred 事务读完
        // 后升级写锁遇到 SQLITE_BUSY_SNAPSHOT（busy_timeout 无法等待）。
        let tx = rusqlite::Transaction::new_unchecked(
            &self.0,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        insert_asset_on(&tx, a, album_id, album_subgroup)?;
        tx.commit()
    }

    /// Tethered captures must keep their chosen album; deletion is an error rather
    /// than silently registering an ungrouped photo. Check and insert share a lock.
    pub fn insert_captured_asset(&self, asset: &AssetRow, album_id: i64) -> Result<()> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.0,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        if !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM album WHERE id=?1)",
            [album_id],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(Error::QueryReturnedNoRows);
        }
        insert_asset_on(&tx, asset, Some(album_id), None)?;
        tx.commit()
    }

    /// 相机聚合（搜索页相机勾选）：camera 非空分组计数，count 降序、
    /// camera 升序稳定排序。
    pub fn camera_list(&self) -> Result<Vec<CameraCountRow>> {
        let value = equipment_text_sql("camera");
        let mut stmt = self.0.prepare(&format!(
            "SELECT {value}, COUNT(*) FROM assets \
             WHERE {value} IS NOT NULL AND in_trash = 0 AND kind IN ('photo', 'raw') \
             GROUP BY {value} ORDER BY COUNT(*) DESC, {value} ASC",
        ))?;
        let rows = stmt.query_map([], |row| {
            Ok(CameraCountRow {
                camera: row.get(0)?,
                count: row.get::<_, i64>(1)? as u64,
            })
        })?;
        rows.collect()
    }

    /// 镜头聚合（搜索页镜头勾选）：lens 非空分组计数，count 降序、
    /// lens 升序稳定排序（与 camera_list 同构）。
    pub fn lens_list(&self) -> Result<Vec<CameraCountRow>> {
        let value = equipment_text_sql("lens");
        let mut stmt = self.0.prepare(&format!(
            "SELECT {value}, COUNT(*) FROM assets \
             WHERE {value} IS NOT NULL AND in_trash = 0 AND kind IN ('photo', 'raw') \
             GROUP BY {value} ORDER BY COUNT(*) DESC, {value} ASC",
        ))?;
        let rows = stmt.query_map([], |row| {
            Ok(CameraCountRow {
                camera: row.get(0)?,
                count: row.get::<_, i64>(1)? as u64,
            })
        })?;
        rows.collect()
    }

    /// 格式聚合（搜索页格式勾选）：路径最后一个点后的扩展名大写分组，
    /// count 降序、格式升序稳定排序；无扩展名的文件不计入（LIKE '%.%'
    /// 门槛。rtrim 技巧：`rtrim(path, replace(path,'.',''))` 剥掉尾部扩展名字符、
    /// 停在点前（结果**含**该点）→ substr(前缀长 + 1) = ext。
    pub fn format_list(&self) -> Result<Vec<CameraCountRow>> {
        let mut stmt = self.0.prepare(
            "SELECT upper(substr(path, length(rtrim(path, replace(path, '.', ''))) + 1)) AS fmt, \
             COUNT(*) FROM assets \
             WHERE path LIKE '%.%' AND in_trash = 0 AND kind IN ('photo', 'raw') \
             GROUP BY fmt ORDER BY COUNT(*) DESC, fmt ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(CameraCountRow {
                camera: row.get(0)?,
                count: row.get::<_, i64>(1)? as u64,
            })
        })?;
        rows.collect()
    }
    /// exif 任务深提取成果落库（gen-3 回填 worker / 导入管线共用）。
    /// 真机发现（2026-09-20）：119 张老库 lens/0004 拍摄参数列全空——旧链
    /// 只在导入时提取，存量永远没人补。gen-3 起一并回填（COALESCE 保留
    /// 已有值，提取不到不清空）；captured_at/camera 不动（导入已写对）。
    pub fn update_asset_deep_exif(
        &self,
        id: i64,
        meta: &crate::metadata::exif_lite::MetaLite,
    ) -> Result<()> {
        let deep = &meta.deep;
        self.0.execute(
            "UPDATE assets SET lens = COALESCE(?2, lens), \
             width = COALESCE(?3, width), height = COALESCE(?4, height), \
             iso = COALESCE(?5, iso), f_number = COALESCE(?6, f_number), \
             exposure_time = COALESCE(?7, exposure_time), \
             focal_length = COALESCE(?8, focal_length), \
             orientation = COALESCE(?9, orientation), flash = ?10, \
             metering_mode = ?11, white_balance = ?12, exposure_program = ?13, \
             software = ?14, artist = ?15, gps_lat = ?16, gps_lon = ?17 \
             WHERE id = ?1",
            params![
                id,
                meta.lens,
                meta.width,
                meta.height,
                meta.iso,
                meta.f_number,
                meta.exposure_time,
                meta.focal_length,
                deep.orientation,
                deep.flash,
                deep.metering_mode,
                deep.white_balance,
                deep.exposure_program,
                deep.software,
                deep.artist,
                deep.gps_lat,
                deep.gps_lon,
            ],
        )?;
        // Restore explicit metadata, including fields cleared by the user.
        self.0.execute(
            "UPDATE assets SET lens=NULLIF(json_extract((SELECT value FROM asset_metadata WHERE asset_id=?1),'$.lens'),''), artist=NULLIF(json_extract((SELECT value FROM asset_metadata WHERE asset_id=?1),'$.author'),''), gps_lat=json_extract((SELECT value FROM asset_metadata WHERE asset_id=?1),'$.gpsLat'), gps_lon=json_extract((SELECT value FROM asset_metadata WHERE asset_id=?1),'$.gpsLon') WHERE id=?1 AND EXISTS (SELECT 1 FROM asset_metadata WHERE asset_id=?1)",
            [id],
        )?;
        Ok(())
    }

    /// 评分写入（0-5 由 IPC 层校验；资产不存在返回 false）。
    /// 只动 DB——XMP 边车同步由调用方（ipc::rating）异步派发。
    /// 读入方向（边车回填）专用：不置 xmp_dirty（DB 在向边车对齐，无待写）。
    pub fn set_asset_rating(&self, id: i64, rating: i64) -> Result<bool> {
        let n = self.0.execute(
            "UPDATE assets SET rating = ?2 WHERE id = ?1",
            params![id, rating],
        )?;
        Ok(n > 0)
    }

    /// 评分写入 + 置脏（§五 M2c 写方向闭环）：用户改动先落库并记
    /// `xmp_dirty=1`；边车写成功后由调用方清脏（[`Db::clear_asset_xmp_dirty`]），
    /// 离线/缺失期间写不出去 → 脏标志留待库恢复在线后扫描补写。
    pub fn set_asset_rating_mark_dirty(&self, id: i64, rating: i64) -> Result<bool> {
        let n = self.0.execute(
            "UPDATE assets SET rating = ?2, xmp_dirty = 1 WHERE id = ?1",
            params![id, rating],
        )?;
        Ok(n > 0)
    }

    /// 当前评分（资产不存在 None；exif 通道 XMP 回填判定用）。
    pub fn asset_rating_of(&self, id: i64) -> Result<Option<i64>> {
        let mut stmt = self.0.prepare("SELECT rating FROM assets WHERE id = ?1")?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    /// 浏览记账（upsert：每资产一行，浏览即刷新 viewed_at）。
    /// 资产不存在静默（调用方契约：mark 对无效 id 不报错）。
    pub fn mark_asset_viewed(&self, asset_id: i64) -> Result<()> {
        let tx = self.0.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO view_history (asset_id, viewed_at) VALUES (?1, ?2)              ON CONFLICT (asset_id) DO UPDATE SET viewed_at = excluded.viewed_at",
            params![asset_id, now_rfc3339()],
        )?;
        tx.execute(
            "DELETE FROM view_history WHERE asset_id NOT IN (SELECT asset_id FROM view_history ORDER BY viewed_at DESC, asset_id DESC LIMIT 200)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 最近浏览资产（viewed_at DESC；复用画廊分页行结构）。回收站资产不出现。
    pub fn recently_viewed(&self, limit: u32) -> Result<Vec<AssetPageRow>> {
        let mut stmt = self.0.prepare(&format!(
            "SELECT {ASSET_PAGE_COLS} FROM view_history v \
             JOIN assets a ON a.id = v.asset_id \
             WHERE a.in_trash = 0 AND a.kind IN ('photo', 'raw') \
             ORDER BY v.viewed_at DESC, v.asset_id DESC LIMIT ?1",
        ))?;
        let rows = stmt.query_map(params![limit], map_asset_page)?;
        rows.collect()
    }

    /// 按多个 id 批量取分页行（组内按 created_at 升序；分组装配由调用方做）。
    pub fn assets_by_ids_ordered(&self, ids: &[i64]) -> Result<Vec<AssetPageRow>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let slots = (0..ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {ASSET_PAGE_COLS} FROM assets WHERE id IN ({slots}) AND kind IN ('photo', 'raw') ORDER BY created_at ASC, id ASC"
        );
        let mut stmt = self.0.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), map_asset_page)?;
        rows.collect()
    }

    /// 那年今天：本地时区同月日的 photo/raw，年份 DESC、年内时间 ASC。
    /// WHERE 片段抽成常量——列表与 sidebar 计数两处共用，杜绝口径漂移。
    pub fn assets_on_this_day(&self, month_day: &str) -> Result<Vec<AssetPageRow>> {
        let mut stmt = self.0.prepare(&format!(
            "SELECT {ASSET_PAGE_COLS} FROM assets \
             WHERE {ON_THIS_DAY_WHERE} \
             ORDER BY substr(captured_at, 1, 4) DESC, captured_at ASC, id ASC",
        ))?;
        let rows = stmt.query_map(params![month_day], map_asset_page)?;
        rows.collect()
    }

    /// 那年今天的计数口径（列表 assets_on_this_day 与侧栏计数共用；
    /// month_day 为本地时区 "%m-%d"）。
    pub fn count_on_this_day(&self, month_day: &str) -> Result<i64> {
        self.0.query_row(
            &format!("SELECT COUNT(*) FROM assets WHERE {ON_THIS_DAY_WHERE}"),
            params![month_day],
            |r| r.get(0),
        )
    }

    /// 侧栏计数（2026-09-21）：库内资产总数（全 kind——侧栏「照片」即画廊
    /// 全量；0016 起排除回收站）与浏览历史行数。纯 COUNT，毫秒级。
    pub fn sidebar_assets_count(&self) -> Result<i64> {
        self.0.query_row(
            "SELECT COUNT(*) FROM assets WHERE in_trash = 0 AND kind IN ('photo', 'raw')",
            [],
            |r| r.get(0),
        )
    }

    pub fn sidebar_viewed_count(&self) -> Result<i64> {
        self.0
            .query_row("SELECT COUNT(*) FROM view_history", [], |r| r.get(0))
    }

    /// 相册数（侧栏计数 0015 起：真实 COUNT(album)，不再是标签墙常量）。
    pub fn sidebar_albums_count(&self) -> Result<i64> {
        self.0
            .query_row("SELECT COUNT(*) FROM album", [], |r| r.get(0))
    }

    /// Only valid, positive capture values participate in equipment buckets.
    /// The same expressions are used by range filters, including for legacy rows.
    pub fn gear_bucket_counts(&self) -> Result<Vec<i64>, String> {
        let focal = positive_decimal_sql("focal_length");
        let iso = positive_decimal_sql("iso");
        let aperture = positive_decimal_sql("f_number");
        let shutter = exposure_seconds_sql("exposure_time");
        let mut stmt = self
            .0
            .prepare(&format!(
                "SELECT \
                    SUM(CASE WHEN f < 24 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN f >= 24 AND f < 50 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN f >= 50 AND f < 85 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN f >= 85 AND f < 135 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN f >= 135 AND f < 200 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN f >= 200 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN iso <= 100 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN iso > 100 AND iso <= 200 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN iso > 200 AND iso <= 400 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN iso > 400 AND iso <= 800 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN iso > 800 AND iso <= 1600 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN iso > 1600 AND iso <= 3200 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN iso > 3200 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN ap <= 1.4 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN ap > 1.4 AND ap <= 2.8 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN ap > 2.8 AND ap <= 4 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN ap > 4 AND ap <= 5.6 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN ap > 5.6 AND ap <= 8 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN ap > 8 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN sec > 1.0 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN sec <= 1.0 AND sec >= 0.5 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN sec < 0.5 AND sec >= 0.125 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN sec < 0.125 AND sec >= 1.0/60 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN sec < 1.0/60 AND sec > 1.0/500 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN sec <= 1.0/500 THEN 1 ELSE 0 END) \
                 FROM ( \
                    SELECT {focal} AS f, {iso} AS iso, {aperture} AS ap, {shutter} AS sec \
                    FROM assets WHERE kind IN ('photo', 'raw') AND in_trash = 0 \
                 )",
            ))
            .map_err(|e| e.to_string())?;
        let counts = stmt
            .query_row([], |r| {
                let mut row = [0i64; 25];
                for (i, slot) in row.iter_mut().enumerate() {
                    *slot = r.get::<_, Option<i64>>(i)?.unwrap_or(0);
                }
                Ok(row)
            })
            .map_err(|e| e.to_string())?;
        Ok(counts.to_vec())
    }

    /// 查重索引：同 (size, xxhash) 的既有**在线**资产 id（导入前快速预判；
    /// **同库范围**——2026-10-09 §四 跨库重复合法，查重只在目标库内生效；
    /// §八-1 定案：skip 仅当存在**在线**同哈希资产，missing 行不算命中——
    /// 命中 missing 的重绑判定走 [`Db::find_asset_brief_by_size_xxh`]）。
    pub fn find_asset_by_size_xxh(
        &self,
        library_id: &str,
        size: u64,
        xxhash: u64,
    ) -> Result<Option<i64>> {
        let mut stmt = self.0.prepare(
            "SELECT id FROM assets WHERE library_id = ?1 AND size = ?2 AND xxhash = ?3 \
             AND missing = 0 LIMIT 1",
        )?;
        let mut rows = stmt.query(params![library_id, size as i64, xxhash as i64])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    /// 宽松查重键（T7 §查重① / T8 new_files 预判）：size + filename +
    /// mtime ±2s，同库范围。RFC3339 定宽字符串按字典序比较即时间序。
    /// §八-1：同库 skip 仅当存在**在线**同键资产——missing 行不算命中
    ///（重新导入回归 missing 照片应放行到精确层重绑，不得在宽松层吞掉）。
    pub fn find_asset_loose(
        &self,
        library_id: &str,
        size: u64,
        filename: &str,
        mtime_from: &str,
        mtime_to: &str,
    ) -> Result<Option<i64>> {
        let mut stmt = self.0.prepare(
            "SELECT id FROM assets WHERE library_id = ?1 AND size = ?2 AND filename = ?3 \
             AND mtime >= ?4 AND mtime <= ?5 AND missing = 0 LIMIT 1",
        )?;
        let mut rows = stmt.query(params![
            library_id,
            size as i64,
            filename,
            mtime_from,
            mtime_to
        ])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    /// 宽松查重键的全库形态（设备扫描 new_files 预检专用，UI 提示口径；
    /// 导入查重闸门走 [`Db::find_asset_loose`] 的同库范围版本）。
    pub fn find_asset_loose_any(
        &self,
        size: u64,
        filename: &str,
        mtime_from: &str,
        mtime_to: &str,
    ) -> Result<Option<i64>> {
        let mut stmt = self.0.prepare(
            "SELECT id FROM assets WHERE size = ?1 AND filename = ?2 \
             AND mtime >= ?3 AND mtime <= ?4 LIMIT 1",
        )?;
        let mut rows = stmt.query(params![size as i64, filename, mtime_from, mtime_to])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    /// 路径是否已在库中（同路径同名查重层；引擎现用文件系统直查）。
    #[allow(dead_code)]
    pub fn asset_path_exists(&self, path: &str) -> Result<bool> {
        let mut stmt = self
            .0
            .prepare("SELECT 1 FROM assets WHERE path = ?1 LIMIT 1")?;
        let mut rows = stmt.query(params![path])?;
        Ok(rows.next()?.is_some())
    }

    /// 边车关键字合并进 asset_metadata（§三/§六 读入方向「关键字从 XMP
    /// 边车读入」）：只覆写 `keywords` 键，其余字段（用户编辑的标题/描述
    /// 等显式库元数据）原样保留——与 album_export 导出方向读同一存储。
    /// 空列表无事可做（不建空行）。
    pub fn merge_asset_keywords(&self, asset_id: i64, keywords: &[String]) -> Result<()> {
        if keywords.is_empty() {
            return Ok(());
        }
        let existing: Option<String> = self
            .0
            .query_row(
                "SELECT value FROM asset_metadata WHERE asset_id = ?1",
                [asset_id],
                |r| r.get(0),
            )
            .ok();
        let mut value: serde_json::Value = existing
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        value["keywords"] = serde_json::json!(keywords);
        let json = serde_json::to_string(&value)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?;
        self.0.execute(
            "INSERT INTO asset_metadata(asset_id, value) VALUES(?1, ?2) \
             ON CONFLICT(asset_id) DO UPDATE SET value = excluded.value",
            params![asset_id, json],
        )?;
        Ok(())
    }

    /// 按路径取资产 id（F1 清卡：journal dst → 库内资产映射）；无则 None。
    pub fn asset_id_by_path(&self, path: &str) -> Result<Option<i64>> {
        let mut stmt = self
            .0
            .prepare("SELECT id FROM assets WHERE path = ?1 LIMIT 1")?;
        let mut rows = stmt.query(params![path])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }
}
