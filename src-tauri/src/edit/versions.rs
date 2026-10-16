//! Persistent edit history, named snapshots and virtual copies. Pixels stay shared.
use super::{project,recipe::{parse_recipe,EditRecipe}};
use crate::ipc::{run_blocking,SharedState};
use rusqlite::{params,OptionalExtension};
use serde::Serialize;
use tauri::State;

#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct Edition {id:String,kind:String,name:String,updated_at:i64,active:bool}

fn normalized(value:&serde_json::Value)->Result<String,String>{
    let recipe=parse_recipe(value)?;
    if recipe.renderer!=Some(super::recipe::RenderEngine::Photocraft){return Err("此版本需要高级编辑引擎".into());}
    let text=serde_json::to_string(&recipe).map_err(|e|e.to_string())?;
    if text.len()>4*1024*1024{return Err("编辑配方过大".into());}Ok(text)
}
fn initialize(db:&crate::db::Db,id:i64,text:&str)->Result<(),String>{
    let master=format!("original:{id}");
    db.0.execute("INSERT OR IGNORE INTO editor_version(id,asset_id,kind,name,recipe,updated_at) VALUES(?1,?2,'original','Original',?3,?4)",params![master,id,text,chrono::Utc::now().timestamp_millis()]).map_err(|e|e.to_string())?;
    db.0.execute("INSERT OR IGNORE INTO editor_workspace(asset_id,active_id) VALUES(?1,?2)",params![id,master]).map_err(|e|e.to_string())?;Ok(())
}
/// Caller owns the transaction, keeping the active copy and canonical recipe atomic.
fn save_inner(db:&crate::db::Db,id:i64,text:&str)->Result<(),String>{
    let previous=db.edit_recipe_get(id).map_err(|e|e.to_string())?.map(|v|v.0);
    initialize(db,id,previous.as_deref().unwrap_or(text))?;
    let now=chrono::Utc::now().timestamp_millis();
    if let Some(old)=previous.filter(|old|old!=text){
        db.0.execute("INSERT INTO editor_version(id,asset_id,kind,name,recipe,updated_at) VALUES(?1,?2,'history','',?3,?4)",params![uuid::Uuid::new_v4().to_string(),id,old,now]).map_err(|e|e.to_string())?;
        db.0.execute("DELETE FROM editor_version WHERE asset_id=?1 AND kind='history' AND id NOT IN (SELECT id FROM editor_version WHERE asset_id=?1 AND kind='history' ORDER BY updated_at DESC,rowid DESC LIMIT 100)",[id]).map_err(|e|e.to_string())?;
    }
    db.0.execute("DELETE FROM editor_version WHERE asset_id=?1 AND kind='history' AND id NOT IN (SELECT id FROM (SELECT id,SUM(length(CAST(recipe AS BLOB))) OVER (ORDER BY updated_at DESC,rowid DESC) AS bytes FROM editor_version WHERE asset_id=?1 AND kind='history') WHERE bytes<=16777216)",[id]).map_err(|e|e.to_string())?;
    db.edit_recipe_upsert(id,text,now).map_err(|e|e.to_string())?;
    db.0.execute("UPDATE editor_version SET recipe=?2,updated_at=?3 WHERE asset_id=?1 AND id=(SELECT active_id FROM editor_workspace WHERE asset_id=?1)",params![id,text,now]).map_err(|e|e.to_string())?;Ok(())
}
pub(super) fn save(db:&crate::db::Db,id:i64,value:&serde_json::Value)->Result<(),String>{
    let text=normalized(value)?;
    let tx=db.0.unchecked_transaction().map_err(|e|e.to_string())?;
    save_inner(db,id,&text)?;tx.commit().map_err(|e|e.to_string())
}
#[tauri::command]
pub async fn edit_versions_list(state:State<'_,SharedState>,library_id:String,asset_id:String)->Result<Vec<Edition>,String>{
    run_blocking(state.inner().clone(),move |state|{
        let (_,asset)=project::resolve(state,&library_id,&asset_id)?;let db=crate::ipc::app_database_db(state)?;
        let mut stmt=db.0.prepare("SELECT v.id,v.kind,v.name,v.updated_at,COALESCE(v.id=w.active_id,0) FROM editor_version v LEFT JOIN editor_workspace w ON w.asset_id=v.asset_id WHERE v.asset_id=?1 ORDER BY v.updated_at DESC,v.rowid DESC").map_err(|e|e.to_string())?;
        let rows=stmt.query_map([asset.id],|r|Ok(Edition{id:r.get(0)?,kind:r.get(1)?,name:r.get(2)?,updated_at:r.get(3)?,active:r.get(4)?})).map_err(|e|e.to_string())?;
        rows.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())
    }).await
}
#[tauri::command]
pub async fn edit_version_create(state:State<'_,SharedState>,library_id:String,asset_id:String,kind:String,name:String,recipe:serde_json::Value)->Result<(),String>{
    run_blocking(state.inner().clone(),move |state|{
        let (_,asset)=project::resolve(state,&library_id,&asset_id)?;let db=crate::ipc::app_database_db(state)?;
        if !matches!(kind.as_str(),"snapshot"|"copy")||name.trim().is_empty()||name.chars().count()>120{return Err("版本类型或名称无效".into());}
        let count:i64=db.0.query_row("SELECT COUNT(*) FROM editor_version WHERE asset_id=?1 AND kind IN ('snapshot','copy')",[asset.id],|r|r.get(0)).map_err(|e|e.to_string())?;
        if count>=200{return Err("快照和副本最多 200 个".into());}
        let text=normalized(&recipe)?;
        let total:i64=db.0.query_row("SELECT COALESCE(SUM(length(CAST(recipe AS BLOB))),0) FROM editor_version WHERE asset_id=?1 AND kind IN ('snapshot','copy')",[asset.id],|r|r.get(0)).map_err(|e|e.to_string())?;
        if total+text.len() as i64>64*1024*1024{return Err("快照和副本超过 64 MB，请先清理不需要的版本".into());}
        let tx=db.0.unchecked_transaction().map_err(|e|e.to_string())?;
        let previous=db.edit_recipe_get(asset.id).map_err(|e|e.to_string())?.map(|v|v.0);
        initialize(&db,asset.id,previous.as_deref().unwrap_or(&text))?;
        db.0.execute("INSERT INTO editor_version(id,asset_id,kind,name,recipe,updated_at) VALUES(?1,?2,?3,?4,?5,?6)",params![uuid::Uuid::new_v4().to_string(),asset.id,kind,name.trim(),text,chrono::Utc::now().timestamp_millis()]).map_err(|e|e.to_string())?;tx.commit().map_err(|e|e.to_string())
    }).await
}
#[tauri::command]
pub async fn edit_version_apply(state:State<'_,SharedState>,library_id:String,asset_id:String,id:String)->Result<EditRecipe,String>{
    run_blocking(state.inner().clone(),move |state|{
        let (_,asset)=project::resolve(state,&library_id,&asset_id)?;let db=crate::ipc::app_database_db(state)?;
        let tx=db.0.unchecked_transaction().map_err(|e|e.to_string())?;
        let (text,kind):(String,String)=db.0.query_row("SELECT recipe,kind FROM editor_version WHERE id=?1 AND asset_id=?2",params![id,asset.id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?.ok_or("编辑版本不存在")?;
        let recipe=parse_recipe(&serde_json::from_str::<serde_json::Value>(&text).map_err(|e|e.to_string())?)?;
        if matches!(kind.as_str(),"original"|"copy"){
            db.0.execute("UPDATE editor_workspace SET active_id=?2 WHERE asset_id=?1",params![asset.id,id]).map_err(|e|e.to_string())?;
            db.edit_recipe_upsert(asset.id,&text,chrono::Utc::now().timestamp_millis()).map_err(|e|e.to_string())?;
        }else{save_inner(&db,asset.id,&text)?;}
        tx.commit().map_err(|e|e.to_string())?;Ok(recipe)
    }).await
}
#[tauri::command]
pub async fn edit_version_delete(state:State<'_,SharedState>,library_id:String,asset_id:String,id:String)->Result<(),String>{
    run_blocking(state.inner().clone(),move |state|{
        let (_,asset)=project::resolve(state,&library_id,&asset_id)?;let db=crate::ipc::app_database_db(state)?;
        let changed=db.0.execute("DELETE FROM editor_version WHERE id=?1 AND asset_id=?2 AND kind!='original' AND id NOT IN (SELECT active_id FROM editor_workspace WHERE asset_id=?2)",params![id,asset.id]).map_err(|e|e.to_string())?;
        if changed==0{return Err("当前版本或原版不能删除".into());}Ok(())
    }).await
}

/// Reading a reference never changes the active virtual copy.
#[tauri::command]
pub async fn edit_version_read(state:State<'_,SharedState>,library_id:String,asset_id:String,id:String)->Result<EditRecipe,String>{
    run_blocking(state.inner().clone(),move |state|{
        let (_,asset)=project::resolve(state,&library_id,&asset_id)?;let db=crate::ipc::app_database_db(state)?;
        let text:String=db.0.query_row("SELECT recipe FROM editor_version WHERE asset_id=?1 AND id=?2",params![asset.id,id],|r|r.get(0)).map_err(|e|e.to_string())?;
        parse_recipe(&serde_json::from_str::<serde_json::Value>(&text).map_err(|e|e.to_string())?)
    }).await
}
