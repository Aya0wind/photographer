//! 相册与子组仓储。

use super::*;

impl Db {
    // —— 相册（M9，album + album_item 表：纯引用照片组）——

    /// 相册列表（item_count 子查询免 GROUP BY 空相册丢行；createdAt DESC、
    /// id DESC tiebreak——空相册同样返回且 itemCount=0）。
    pub fn album_list(&self) -> Result<Vec<AlbumRow>> {
        let mut stmt = self.0.prepare(
            "SELECT a.id, a.name, \
                    (SELECT id FROM assets WHERE id = a.cover_asset_id AND kind IN ('photo', 'raw')), \
                    (SELECT COUNT(*) FROM album_item i JOIN assets x ON x.id = i.asset_id AND x.kind IN ('photo', 'raw') WHERE i.album_id = a.id), a.created_at, \
                    a.dir_name \
             FROM album a ORDER BY a.created_at DESC, a.id DESC",
        )?;
        let rows = stmt.query_map([], map_album)?;
        rows.collect()
    }

    /// 资产 → 所属相册反查（查看器详情「所属相册」行）。按相册创建时间 DESC；
    /// 未入任何相册返回空。
    pub fn asset_albums(&self, asset_id: i64) -> Result<Vec<AlbumRow>> {
        let mut stmt = self.0.prepare(
            "SELECT a.id, a.name, \
                    (SELECT id FROM assets WHERE id = a.cover_asset_id AND kind IN ('photo', 'raw')), \
                    (SELECT COUNT(*) FROM album_item i2 JOIN assets x ON x.id = i2.asset_id AND x.kind IN ('photo', 'raw') WHERE i2.album_id = a.id), a.created_at, \
                    a.dir_name \
             FROM album a \
             WHERE EXISTS (SELECT 1 FROM album_item i WHERE i.album_id = a.id AND i.asset_id = ?1) \
             ORDER BY a.created_at DESC, a.id DESC",
        )?;
        let rows = stmt.query_map(params![asset_id], map_album)?;
        rows.collect()
    }

    /// 按相册 id 取目录名（相册改名 / 物理挪移用）。
    pub fn album_dir_name(&self, id: i64) -> Result<Option<String>> {
        self.0
            .query_row("SELECT dir_name FROM album WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .map(Some)
            .or_else(|e| match e {
                Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
    }

    /// 相册主目录相对段（photoRoot 下，含 dir_name，**无**头尾分隔符）：
    /// `{创建YYYY}/{创建MM}/{dir_name}`——布局公式只此一处定义（用户定案
    /// 2026-09-28：时间/相册 + 相册内平铺），导入引擎（dir_template 覆写）、
    /// 归册挪移（claim）、album 模式导出三调用点共用，不得各自拼。
    /// 外层两段 = 相册 created_at（UTC 口径，与列存储一致）的字面量段，
    /// 相册级常量——相册内照片平铺，拍摄日分组在应用 UI（groupAssetsByDate）
    /// 完成，不落存储层。created_at 解析失败（理论不可能，列 NOT NULL）
    /// 兜底 dir_name 直挂 photoRoot。段间用 `/`（render_dir 渲染产物同形态，
    /// Windows Path::join 兼容）。
    ///
    /// 子组物理化（0022）后「相册内平铺」的唯一例外 = 子组：条目级落位
    /// 请用 [`Db::album_item_home_rel`]（本方法保留为相册主目录基段）。
    pub fn album_home_rel(&self, id: i64) -> Result<Option<String>> {
        self.0
            .query_row(
                "SELECT created_at, dir_name FROM album WHERE id = ?1",
                [id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .map(|(created_at, dir_name)| album_home_rel_parts(&created_at, &dir_name))
            .map(Some)
            .or_else(|e| match e {
                Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
    }

    /// 全部相册的主目录相对段（claim 挪移的主相册前缀判定用，
    /// [`Db::album_home_rel`] 的批量形态）。
    pub fn album_home_rels(&self) -> Result<Vec<(i64, String)>> {
        let mut stmt = self
            .0
            .prepare("SELECT id, created_at, dir_name FROM album ORDER BY id")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                album_home_rel_parts(&r.get::<_, String>(1)?, &r.get::<_, String>(2)?),
            ))
        })?;
        rows.collect()
    }

    /// 相册内**条目**目录相对段（子组物理化 0022，存储布局唯一不平铺例外）：
    /// `{创建YYYY}/{创建MM}/{dir_name}[/{子组}]`——subgroup=Some 时在相册
    /// 主目录（[`Db::album_home_rel`]）之后追加净化子组段，None = 相册根
    /// （平铺现状）。四调用点共用（导入引擎 dir_template 覆写、移组挪移
    /// album_item_move_subgroup、album 模式导出、claim 归册），不得各自拼。
    ///
    /// 子组段口径：与 album dir_name 同一 [`sanitize_dir_name`]（非法字符/
    /// 控制符折叠、尾点空格剥离、保留设备名前缀、80 字符截断、空兜底）——
    /// DB subgroup 原值（UI datalist 输入）与物理段是**纯函数映射**，四个
    /// 调用点对同一子组名永远解析出同一段，物理归位不漂移。子组嵌套不支持
    /// （一层；`/` 等分隔符在 sanitize 中折叠为 `-`，天然拍平）。同名文件
    /// 占位该目录名时物理 mkdir 失败由调用方按行报错（简化定案：sanitize
    /// 后直接用，不做让位）。
    pub fn album_item_home_rel(&self, id: i64, subgroup: Option<&str>) -> Result<Option<String>> {
        match self.album_home_rel(id)? {
            Some(home) => Ok(Some(match subgroup {
                Some(sub) => format!("{}/{}", home, sanitize_dir_name(sub)),
                None => home,
            })),
            None => Ok(None),
        }
    }

    /// 批量改写资产路径前缀（0018 相册目录 rename / claim 挪移共用）：
    /// `path` 以 `old_prefix`（含尾分隔符）开头的行改为 `new_prefix +`
    /// 剩余部分，同事务内顺带更新 album.dir_name。前缀匹配**分隔符归一**
    /// （`\` 与 `/` 视为等同——库内 path 存在两种形态：引擎 render_dir
    /// 产物段内是 `/`、claim/迁移脚本产物是 `\`；替换只动前缀段，剩余
    /// 部分保留原分隔符）。返回改写行数。filename 若在新路径下失效由
    /// 调用方一并处理。
    pub fn album_rewrite_paths(
        &self,
        album_id: i64,
        new_dir_name: &str,
        old_prefix: &str,
        new_prefix: &str,
    ) -> Result<u64> {
        let norm_old = old_prefix.replace('\\', "/");
        let prefix_chars = norm_old.chars().count() as i64;
        let tx = self.0.unchecked_transaction()?;
        let n = tx.execute(
            "UPDATE assets SET path = ?2 || substr(path, ?4 + 1) \
             WHERE substr(replace(path, char(92), '/'), 1, ?4) = ?3",
            params![album_id, new_prefix, norm_old, prefix_chars],
        )?;
        tx.execute(
            "UPDATE album SET dir_name = ?2 WHERE id = ?1",
            params![album_id, new_dir_name],
        )?;
        tx.commit()?;
        Ok(n as u64)
    }

    /// 单资产路径更新（claim 挪移落库；文件名可随冲突后缀变化）。
    pub fn asset_update_path(&self, id: i64, new_path: &str, new_filename: &str) -> Result<()> {
        self.0.execute(
            "UPDATE assets SET path = ?2, filename = ?3 WHERE id = ?1",
            params![id, new_path, new_filename],
        )?;
        Ok(())
    }

    /// 系统级保底（0018 修订）：确保默认相册「未分组」存在（按名称幂等），
    /// 返回其 id。导入启动时调用；「未分组」禁删禁改名（用户定案），故
    /// 幂等保证永远命中同一条，不会重复创建。
    pub fn ensure_default_album(&self) -> Result<i64> {
        if let Ok(id) = self.0.query_row(
            "SELECT id FROM album WHERE name = ?1",
            [DEFAULT_ALBUM_NAME],
            |r| r.get::<_, i64>(0),
        ) {
            return Ok(id);
        }
        Ok(self.album_create(DEFAULT_ALBUM_NAME)?.id)
    }

    /// 建相册，返回新行（item_count=0、cover=None）。重名由 name UNIQUE
    /// 兜底（错误透传，IPC 层转友好文案）；名称 trim/空校验在 IPC 层。
    /// dir_name 由显示名净化生成（0018），被占用时追加 `-2`/`-3`… 后缀。
    pub fn album_create(&self, name: &str) -> Result<AlbumRow> {
        let created_at = now_rfc3339();
        let base = sanitize_dir_name(name);
        let mut dir_name = base.clone();
        let mut n = 2;
        loop {
            let taken: i64 = self.0.query_row(
                "SELECT COUNT(*) FROM album WHERE dir_name = ?1",
                [&dir_name],
                |r| r.get(0),
            )?;
            if taken == 0 {
                break;
            }
            dir_name = format!("{base}-{n}");
            n += 1;
        }
        self.0.execute(
            "INSERT INTO album (name, cover_asset_id, created_at, dir_name) \
             VALUES (?1, NULL, ?2, ?3)",
            params![name, created_at, dir_name],
        )?;
        Ok(AlbumRow {
            id: self.0.last_insert_rowid(),
            name: name.to_string(),
            cover_asset_id: None,
            item_count: 0,
            created_at,
            dir_name,
        })
    }

    /// 相册重命名；相册不存在报错（同 rename_person 语义）。
    /// 重名由 UNIQUE 兜底透传。默认相册「未分组」禁改名（0018 再修订：
    /// 与禁删叠加——它完全固定，永远是系统兜底落点；自动创建按名幂等，
    /// 不会重复创建）。
    pub fn album_rename(&self, id: i64, name: &str) -> Result<()> {
        let current: String =
            self.0
                .query_row("SELECT name FROM album WHERE id = ?1", [id], |r| r.get(0))?;
        if current == DEFAULT_ALBUM_NAME {
            return Err(Error::InvalidParameterName(
                "默认相册「未分组」不可改名".into(),
            ));
        }
        let n = self.0.execute(
            "UPDATE album SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        if n == 0 {
            return Err(Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    /// 删相册：只删相册行——album_item 引用经 FK ON DELETE CASCADE 级联
    /// 消失，资产与物理文件绝不动；相册不存在报错（幂等删除由 IPC 语义定）。
    /// 默认相册「未分组」拒删（0018 修订：系统级保底，防误删后下次导入又
    /// 冒出来；禁删禁改名——完全固定）。
    pub fn album_delete(&self, id: i64) -> Result<()> {
        let name: String = self
            .0
            .query_row("SELECT name FROM album WHERE id = ?1", [id], |r| r.get(0))?;
        if name == DEFAULT_ALBUM_NAME {
            return Err(Error::InvalidParameterName(
                "默认相册「未分组」不可删除（只能清空或改名）".into(),
            ));
        }
        let n = self
            .0
            .execute("DELETE FROM album WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    /// 设/清相册封面（纯引用；asset_id 为 None 清回默认封面）。
    /// 相册不存在报错；asset 不存在触发 FK 约束错误透传（IPC 层转文案）。
    pub fn album_cover_set(&self, id: i64, asset_id: Option<i64>) -> Result<()> {
        let n = self.0.execute(
            "UPDATE album SET cover_asset_id = ?2 WHERE id = ?1",
            params![id, asset_id],
        )?;
        if n == 0 {
            return Err(Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    /// 批量入册引用：幂等（已存在仅在显式给子分组时改写归属，None = 保持
    /// 原状），返回**实际新增**数；不存在的资产 id 静默跳过（相册页列表来自
    /// 实时库，恰好在他处被永久删除的 id 属预期陈旧值）；相册不存在报错。
    /// `subgroup` = 0019 子分组命名层（None = 散在相册根）。
    pub fn album_add_assets(
        &self,
        album_id: i64,
        asset_ids: &[i64],
        subgroup: Option<&str>,
    ) -> Result<u64> {
        if !self.album_exists(album_id)? {
            return Err(Error::QueryReturnedNoRows);
        }
        let added_at = now_rfc3339();
        let tx = self.0.unchecked_transaction()?;
        let mut added = 0u64;
        for asset_id in asset_ids {
            // INSERT..SELECT：资产不存在 → 0 行（跳过），不触发 FK 错误
            let exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM album_item \
                     WHERE album_id = ?1 AND asset_id = ?2)",
                    params![album_id, asset_id],
                    |r| r.get(0),
                )
                .unwrap_or(false);
            if exists {
                if let Some(sub) = subgroup {
                    tx.execute(
                        "UPDATE album_item SET subgroup = ?3 \
                         WHERE album_id = ?1 AND asset_id = ?2",
                        params![album_id, asset_id, sub],
                    )?;
                }
                continue;
            }
            added += tx.execute(
                "INSERT INTO album_item (album_id, asset_id, added_at, subgroup) \
                     SELECT ?1, ?2, ?3, ?4 WHERE EXISTS (SELECT 1 FROM assets WHERE id = ?2 AND kind IN ('photo', 'raw'))",
                params![album_id, asset_id, added_at, subgroup],
            )? as u64;
        }
        tx.commit()?;
        Ok(added)
    }

    /// 相册内挪子分组的**账本半边**（0019；0022 物理化后由 IPC 层
    /// `fetch_album_item_move_subgroup` 编排：先物理挪移（XMP 边车随行）再
    /// 走本方法改 subgroup + `asset_update_path` 改写路径——先物理后账本，
    /// 挪移失败的行不落账）。本方法只做引用层 UPDATE（subgroup 改写，
    /// None = 挪回根），不动物理文件。不在该相册的 id 自然不命中（0 行）。
    /// 返回实际改写行数；相册不存在报错。
    pub fn album_item_move_subgroup(
        &self,
        album_id: i64,
        asset_ids: &[i64],
        subgroup: Option<&str>,
    ) -> Result<u64> {
        if !self.album_exists(album_id)? {
            return Err(Error::QueryReturnedNoRows);
        }
        if asset_ids.is_empty() {
            return Ok(0);
        }
        let slots = (0..asset_ids.len())
            .map(|i| format!("?{}", i + 3))
            .collect::<Vec<_>>()
            .join(", ");
        let n = self.0.execute(
            &format!(
                "UPDATE album_item SET subgroup = ?2 \
                 WHERE album_id = ?1 AND asset_id IN ({slots})"
            ),
            rusqlite::params_from_iter(
                std::iter::once(rusqlite::types::Value::from(album_id))
                    .chain(std::iter::once(rusqlite::types::Value::from(
                        subgroup.map(str::to_string),
                    )))
                    .chain(asset_ids.iter().map(|id| rusqlite::types::Value::from(*id))),
            ),
        )?;
        Ok(n as u64)
    }

    /// 相册子分组清单（0019）：DISTINCT subgroup + 计数，name 升序；
    /// 散在根（NULL）不入清单（根视图即相册默认视图）。
    pub fn album_subgroups(&self, album_id: i64) -> Result<Vec<(String, u64)>> {
        let mut stmt = self.0.prepare(
            "SELECT i.subgroup, COUNT(*) FROM album_item i \
             JOIN assets a ON a.id = i.asset_id AND a.kind IN ('photo', 'raw') \
             WHERE i.album_id = ?1 AND i.subgroup IS NOT NULL \
             GROUP BY subgroup ORDER BY subgroup ASC",
        )?;
        let rows = stmt.query_map(params![album_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
        })?;
        rows.collect()
    }

    /// 批量移除引用（幂等：不在册的 id 删 0 行）；相册不存在报错。
    /// 只删 album_item 行，绝不动资产行/物理文件。
    pub fn album_remove_assets(&self, album_id: i64, asset_ids: &[i64]) -> Result<()> {
        if !self.album_exists(album_id)? {
            return Err(Error::QueryReturnedNoRows);
        }
        if asset_ids.is_empty() {
            return Ok(());
        }
        let slots = (0..asset_ids.len())
            .map(|i| format!("?{}", i + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("DELETE FROM album_item WHERE album_id = ?1 AND asset_id IN ({slots})");
        self.0.execute(
            &sql,
            rusqlite::params_from_iter(std::iter::once(album_id).chain(asset_ids.iter().copied())),
        )?;
        Ok(())
    }

    /// 相册是否存在（IPC 存在性校验 / add-remove 前置）。
    pub fn album_exists(&self, id: i64) -> Result<bool> {
        let exists: i64 = self.0.query_row(
            "SELECT EXISTS(SELECT 1 FROM album WHERE id = ?1)",
            [id],
            |r| r.get(0),
        )?;
        Ok(exists != 0)
    }
}
