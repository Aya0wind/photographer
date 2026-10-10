//! 两种编辑入口共用 SQLite 编辑配方；旧 advanced-edits 文件只作兼容读取。
//! 库归属守卫和旧项目迁移集中于本模块；打开不改写存储，保存才更新真值。
use super::recipe::{parse_recipe, EditRecipe, RenderEngine};
use crate::ipc::{run_blocking, SharedState};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::PathBuf;
use tauri::State;

const MAX_PROJECT_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Project {
    version: u32,
    library_id: String,
    asset_id: i64,
    recipe: serde_json::Value,
}

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

/// 旧编辑文件位于激活数据库目录；保留用于读取历史项目。
fn project_path(app_db_dir: &std::path::Path, asset_id: i64) -> PathBuf {
    app_db_dir
        .join("advanced-edits")
        .join(format!("{asset_id}.json"))
}

fn validate_recipe(value: &serde_json::Value) -> Result<EditRecipe, String> {
    let recipe = parse_recipe(value)?;
    if recipe.renderer != Some(RenderEngine::Photocraft) {
        return Err("不是高级编辑状态".into());
    }
    Ok(recipe)
}

pub(super) fn remove_legacy(dir:&std::path::Path,id:i64)->Result<(),String> {
    match std::fs::remove_file(project_path(dir,id)) {
        Ok(())=>Ok(()),Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(()),Err(e)=>Err(format!("清除旧编辑状态失败：{e}"))
    }
}

pub fn fetch_project_delete(db:&crate::db::Db,dir:&std::path::Path,id:i64)->Result<(),String> {
    remove_legacy(dir,id)?;
    super::ipc::fetch_edit_recipe_delete(db,id)
}

/// Canonical SQLite recipe is shared by both editor entry points. Old side
/// projects are read only for migration; opening never modifies stored data.
pub fn fetch_project_open(db:&crate::db::Db,dir:&std::path::Path,library_id:&str,id:i64)->Result<Option<EditRecipe>,String> {
    let stored=super::ipc::fetch_edit_recipe(db,id)?.recipe.map(|v|parse_recipe(&v)).transpose()?;
    if stored.as_ref().is_some_and(|r|r.renderer==Some(RenderEngine::Photocraft)) {return Ok(stored);}
    let file=match std::fs::File::open(project_path(dir,id)) {
        Ok(file)=>Some(file),Err(e) if e.kind()==std::io::ErrorKind::NotFound=>None,Err(e)=>return Err(format!("读取编辑状态失败：{e}"))
    };
    if let Some(file)=file {
        let mut bytes=Vec::new();file.take(MAX_PROJECT_BYTES+1).read_to_end(&mut bytes).map_err(|e|e.to_string())?;
        if bytes.len() as u64>MAX_PROJECT_BYTES {return Err("编辑状态过大".into());}
        let project:Project=serde_json::from_slice(&bytes).map_err(|e|format!("编辑状态无效：{e}"))?;
        if project.version!=1||project.library_id!=library_id||project.asset_id!=id {return Err("编辑状态与照片不匹配".into());}
        return validate_recipe(&project.recipe).map(Some);
    }
    Ok(stored.map(|mut recipe|{
        recipe.renderer=Some(RenderEngine::Photocraft);
        recipe.legacy_adjustments=recipe.adjustments.take().filter(|a|a.brightness!=0.0||a.contrast!=0.0||a.saturation!=0.0);
        recipe
    }))
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
        resolve(state, &library_id, &asset_id)?;
        let recipe = validate_recipe(&recipe)?;
        let value=serde_json::to_value(recipe).map_err(|e|e.to_string())?;
        let bytes=serde_json::to_vec(&value).map_err(|e|e.to_string())?;
        if bytes.len() as u64 > MAX_PROJECT_BYTES {
            return Err("编辑状态过大".into());
        }
        super::ipc::fetch_edit_recipe_save(&crate::ipc::app_database_db(state)?,id,&value)?;
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
        fetch_project_open(&crate::ipc::app_database_db(state)?,&crate::ipc::app_database_dir(state)?,&library_id,id)
    })
    .await
}
