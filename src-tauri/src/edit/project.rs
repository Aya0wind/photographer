//! Both editor entry points use the canonical SQLite recipe.
use super::recipe::{parse_recipe, EditRecipe, RenderEngine};
use crate::ipc::{run_blocking, SharedState};
use tauri::State;

const MAX_PROJECT_BYTES: u64 = 4 * 1024 * 1024;

/// 解析资产并校验照片库归属，拒绝旧会话资产 id 沿用。
/// 多数据库修正（2026-10-09）：project 文件落激活数据库目录
///（[`crate::ipc::app_database_dir`] 自动跟随切换），不同数据库的高级编辑
/// 状态天然隔离；本守卫校验**激活库内**会话期望库与资产实际所属库一致
///（跨数据库则是另一份 SQLite，资产 id 天然不串）。
pub(super) fn resolve(
    state: &crate::ipc::AppState,
    library_id: &str,
    asset_id: &str,
) -> Result<(crate::db::libraries::PhotosLibraryRow, crate::db::AssetRow), String> {
    let id = super::ipc::parse_asset_id(asset_id)?;
    let db = crate::ipc::app_database_db(state)?;
    let asset = db
        .asset_by_id(id)
        .map_err(|e| e.to_string())?
        .ok_or("照片不存在")?;
    if asset.library_id.as_deref() != Some(library_id) {
        return Err("照片库已切换，请重新打开高级编辑".into());
    }
    let library = db
        .photos_library_get(library_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("照片库不存在：{library_id}"))?;
    Ok((library, asset))
}

fn validate_recipe(value: &serde_json::Value) -> Result<EditRecipe, String> {
    let recipe = parse_recipe(value)?;
    if recipe.renderer != Some(RenderEngine::Photocraft) {
        return Err("不是高级编辑状态".into());
    }
    Ok(recipe)
}

pub fn fetch_project_delete(db:&crate::db::Db,id:i64)->Result<(),String>{super::ipc::fetch_edit_recipe_delete(db,id)}

pub fn fetch_project_open(db:&crate::db::Db,id:i64)->Result<Option<EditRecipe>,String>{
    super::ipc::fetch_edit_recipe(db,id)?.recipe.map(|value|validate_recipe(&value)).transpose()
}

#[tauri::command]
pub async fn edit_project_save(
    state: State<'_, SharedState>,
    library_id: String,
    asset_id: String,
    recipe: serde_json::Value,
) -> Result<(), String> {
    run_blocking(state.inner().clone(), move |state| {
        let id = super::ipc::parse_asset_id(&asset_id)?;
        let (_,asset)=resolve(state, &library_id, &asset_id)?;
        let recipe = validate_recipe(&recipe)?;
        if asset.kind!=crate::events::AssetKind::Raw&&recipe.color_pipeline.as_ref().and_then(|p|p.lens_correction.as_ref()).is_some_and(|l|l.distortion||l.tca){return Err("Lensfun 镜头配置仅适用于 RAW 照片".into());}
        if asset.kind!=crate::events::AssetKind::Raw&&recipe.color_pipeline.as_ref().is_some_and(|p|p.raw_white_balance.is_some()){return Err("传感器白平衡仅适用于 RAW 照片".into());}
        let value=serde_json::to_value(recipe).map_err(|e|e.to_string())?;
        let bytes=serde_json::to_vec(&value).map_err(|e|e.to_string())?;
        if bytes.len() as u64 > MAX_PROJECT_BYTES {
            return Err("编辑状态过大".into());
        }
        super::versions::save(&crate::ipc::app_database_db(state)?,id,&value)?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn edit_project_open(
    state: State<'_, SharedState>,
    library_id: String,
    asset_id: String,
) -> Result<Option<EditRecipe>, String> {
    run_blocking(state.inner().clone(), move |state| {
        let id = super::ipc::parse_asset_id(&asset_id)?;
        resolve(state, &library_id, &asset_id)?;
        fetch_project_open(&crate::ipc::app_database_db(state)?,id)
    })
    .await
}
