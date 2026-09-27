//! 版本查询命令：资产版本切换数据源（photo_group 组员表）。
//!
//! `asset_versions`：详情页版本 chips 数据源——RAW+机内 JPEG 孪生分组
//! （0017 photo_group）的组员 raw→sooc 序；子分组（0019）与派生件概念
//! 已按用户定案移除（派生件走普通导入 + 相册子分组，不建组）。
//!
//! 全部走 active_library_db + run_blocking（铁律：DB 查询不上主线程）。

use serde::{Deserialize, Serialize};
use tauri::State;

use super::{run_blocking, SharedState};

/// 版本组成员（详情页版本切换条目）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionMemberDto {
    pub asset_id: i64,
    /// raw | sooc（未入组资产 None；'derived' 角色随原成片概念移除后
    /// 不再产生，保留在 CHECK 约束里仅为兼容历史库数据）。
    pub role: Option<String>,
    pub name: String,
    /// 缩略图就绪（thumb_state==1）。
    pub thumb_ready: bool,
}

/// 版本查询载荷：asset_id 未入组时 group_id=None 且 members 只有自己。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetVersionsDto {
    pub group_id: Option<i64>,
    pub members: Vec<VersionMemberDto>,
}

/// 版本查询核：组员 raw→sooc 序；未入组返回自己（role=None）。
pub fn asset_versions_core(db: &crate::db::Db, asset_id: i64) -> Result<AssetVersionsDto, String> {
    let group_id = db.asset_group_of(asset_id).map_err(|e| e.to_string())?;
    let Some(group_id) = group_id else {
        let member = db
            .asset_by_id(asset_id)
            .map_err(|e| e.to_string())?
            .map(|a| VersionMemberDto {
                asset_id,
                role: None,
                name: a.filename,
                thumb_ready: a.thumb_state == 1,
            });
        return Ok(AssetVersionsDto {
            group_id: None,
            members: member.into_iter().collect(),
        });
    };
    let members: Vec<VersionMemberDto> = db
        .group_members(group_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|(mid, role)| {
            let (name, thumb_state) =
                db.0.query_row(
                    "SELECT filename, thumb_state FROM assets WHERE id = ?1",
                    [mid],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                        ))
                    },
                )
                .map_err(|e| e.to_string())?;
            Ok(VersionMemberDto {
                asset_id: mid,
                role: Some(role),
                name,
                thumb_ready: thumb_state == 1,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(AssetVersionsDto {
        group_id: Some(group_id),
        members,
    })
}

/// 版本查询（详情页版本切换数据源）。
#[tauri::command]
pub async fn asset_versions(
    state: State<'_, SharedState>,
    asset_id: i64,
) -> Result<AssetVersionsDto, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        let db = super::active_library_db(state)?;
        asset_versions_core(&db, asset_id)
    })
    .await
}
