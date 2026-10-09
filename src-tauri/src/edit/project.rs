//! 高级编辑状态由应用管理，按库/资产关联；不向 UI 暴露项目文件路径。
//! 存储适配集中于本模块，库管理重构只需替换 resolve/path，不改 UI 或引擎。
use super::recipe::{parse_recipe, EditRecipe, RenderEngine};
use crate::ipc::{run_blocking, SharedState};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
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

/// 解析资产并校验照片库归属，拒绝旧会话跨库沿用资产 id。
/// 大一统定案（2026-10-09）：库是资产的静态归属，不再有「活动库切换」，
/// 等价守卫 = 会话期望库与资产实际所属库一致。
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

/// 编辑状态归应用数据库目录（asset id 全局唯一，无需按库分目录）。
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
        let path = project_path(&crate::ipc::app_database_dir(state), id);
        let project = Project {
            version: 1,
            library_id,
            asset_id: id,
            recipe: serde_json::to_value(recipe).map_err(|e| e.to_string())?,
        };
        let bytes = serde_json::to_vec(&project).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_PROJECT_BYTES {
            return Err("编辑状态过大".into());
        }
        let parent = path.parent().ok_or("编辑状态位置不可用")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        temp.write_all(&bytes).map_err(|e| e.to_string())?;
        temp.as_file().sync_all().map_err(|e| e.to_string())?;
        temp.persist(&path)
            .map_err(|e| format!("保存编辑失败: {e}"))?;
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
        let path = project_path(&crate::ipc::app_database_dir(state), id);
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("读取编辑状态失败: {e}")),
        };
        let mut bytes = Vec::new();
        file.take(MAX_PROJECT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_PROJECT_BYTES {
            return Err("编辑状态过大".into());
        }
        let project: Project =
            serde_json::from_slice(&bytes).map_err(|e| format!("编辑状态无效: {e}"))?;
        if project.version != 1 || project.library_id != library_id || project.asset_id != id {
            return Err("编辑状态与照片不匹配".into());
        }
        validate_recipe(&project.recipe).map(Some)
    })
    .await
}
