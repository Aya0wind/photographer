//! Portable recipe plus verified creative LUTs. Does not include source photos,
//! database ownership or previews; applying is a single reversible UI edit.
use super::{lut,recipe::{EditRecipe,parse_recipe,RenderEngine}};
use crate::ipc::{SharedState,run_blocking};
use serde::{Serialize,Deserialize};
use std::{collections::BTreeSet,io::Read,path::Path};
use tauri::State;
const MAX_BYTES:u64=96*1024*1024;
fn validate_size(recipe:&EditRecipe)->Result<(),String>{if serde_json::to_vec(recipe).map_err(|e|e.to_string())?.len()>4*1024*1024{return Err("编辑参数过大".into());}Ok(())}
#[derive(Serialize,Deserialize)]
struct Scheme {format:String,version:u32,recipe:serde_json::Value,resources:Vec<lut::PackagedLut>,#[serde(default,skip_serializing_if="Vec::is_empty")] films:Vec<super::film::PackagedFilm>}
fn references(recipe:&EditRecipe)->BTreeSet<String>{
    recipe.advanced.iter().chain(recipe.masks.iter().filter_map(|m|m.advanced.as_ref())).filter_map(|a|a.lookup.as_ref().map(|l|l.id.clone())).collect()
}
fn pack(root:&Path,recipe:EditRecipe)->Result<Vec<u8>,String>{
    validate_size(&recipe)?;
    if recipe.renderer!=Some(RenderEngine::Photocraft){return Err("请使用高级编辑保存方案".into());}
    let resources=lut::package_resources(root,&references(&recipe))?;
    let films=super::film::package(&recipe)?;
    let bytes=serde_json::to_vec(&Scheme{format:"photographer.edit".into(),version:if recipe.color_pipeline.is_some(){2}else{1},recipe:serde_json::to_value(recipe).map_err(|e|e.to_string())?,resources,films}).map_err(|e|e.to_string())?;
    if bytes.len() as u64>MAX_BYTES{return Err("编辑方案过大".into());}Ok(bytes)
}
fn unpack(bytes:&[u8])->Result<(EditRecipe,Vec<lut::PackagedLut>),String>{
    if bytes.len() as u64>MAX_BYTES{return Err("编辑方案过大".into());}
    let scheme:Scheme=serde_json::from_slice(bytes).map_err(|e|format!("编辑方案无效：{e}"))?;
    if !matches!(scheme.version,1|2)||scheme.format!="photographer.edit"{return Err("不支持此编辑方案版本".into());}
    let recipe=parse_recipe(&scheme.recipe)?;
    validate_size(&recipe)?;
    if recipe.renderer!=Some(RenderEngine::Photocraft){return Err("编辑方案引擎无效".into());}
    let required:BTreeSet<_>=references(&recipe).into_iter().filter(|id|!id.starts_with("builtin:")).collect();
    let provided:BTreeSet<_>=scheme.resources.iter().map(|r|r.id.clone()).collect();
    if provided!=required||provided.len()!=scheme.resources.len(){return Err("编辑方案的调色资源缺失或重复".into());}
    let expected=recipe.color_pipeline.as_ref().and_then(|p|p.film.as_ref()).map(|f|f.id.as_str());
    if scheme.films.len()!=usize::from(expected.is_some()) || scheme.films.first().map(|f|f.id.as_str())!=expected{return Err("方案胶片资源缺失或重复".into());}
    for resource in &scheme.films{super::film::validate(resource)?;}
    for resource in &scheme.resources{lut::validate_packaged(resource)?;}
    Ok((recipe,scheme.resources))
}
#[cfg(test)]
mod tests{
 use super::*;
 #[test]
 fn scheme_carries_global_and_local_luts_and_restores_them_in_an_empty_store(){
  let source=tempfile::tempdir().unwrap();let target=tempfile::tempdir().unwrap();let file=source.path().join("look.cube");
  std::fs::write(&file,photocraft_cms::lutfile::write_cube(&photocraft_cms::lutfile::LutFile::identity(2))).unwrap();
  let entry=lut::import_at(&source.path().join("luts"),&file).unwrap();
  let mut local=photocraft_cms::lutfile::LutFile::identity(2);local.data[0]=0.1;std::fs::write(&file,photocraft_cms::lutfile::write_cube(&local)).unwrap();let local_entry=lut::import_at(&source.path().join("luts"),&file).unwrap();
  let recipe=parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","advanced":{"exposure":1,"lookup":{"id":entry.id,"amount":75,"enabled":true}},"masks":[{"id":"m","name":"Local","kind":"linear","enabled":true,"inverted":false,"density":80,"feather":0,"from":{"x":0,"y":0},"to":{"x":1,"y":1},"advanced":{"lookup":{"id":local_entry.id,"amount":30,"enabled":false}}}]})).unwrap();
  let bytes=pack(&source.path().join("luts"),recipe.clone()).unwrap();let (restored,resources)=unpack(&bytes).unwrap();
  let mut duplicate:serde_json::Value=serde_json::from_slice(&bytes).unwrap();let extra=duplicate["resources"][0].clone();duplicate["resources"].as_array_mut().unwrap().push(extra);assert!(unpack(&serde_json::to_vec(&duplicate).unwrap()).is_err());
  assert_eq!(restored,recipe);assert_eq!(resources.len(),2);lut::install_packaged(&target.path().join("luts"),&resources).unwrap();
  for resource in &resources{assert_eq!(std::fs::read(target.path().join("luts").join(format!("{}.cube",resource.id))).unwrap(),resource.cube.as_bytes());}
  lut::install_packaged(&target.path().join("luts"),&resources).unwrap();
  let catalog:Vec<lut::LutEntry>=serde_json::from_slice(&std::fs::read(target.path().join("luts/catalog.json")).unwrap()).unwrap();assert_eq!(catalog.len(),2);
 }
 #[test]
 fn malformed_resources_are_rejected_before_any_installation(){
  let root=tempfile::tempdir().unwrap();let cube=photocraft_cms::lutfile::write_cube(&photocraft_cms::lutfile::LutFile::identity(2));
  let resource=lut::PackagedLut{id:"0".repeat(64),name:"Look".into(),cube};
  assert!(lut::install_packaged(&root.path().join("luts"),&[resource.clone()]).is_err());assert!(!root.path().join("luts").exists());
  let mut value=serde_json::json!({"format":"photographer.edit","version":1,"recipe":{"version":1,"renderer":"photocraft","advanced":{"lookup":{"id":resource.id,"amount":100,"enabled":true}}},"resources":[]});
  assert!(unpack(&serde_json::to_vec(&value).unwrap()).is_err());value["resources"]=serde_json::json!([resource]);assert!(unpack(&serde_json::to_vec(&value).unwrap()).is_err());
  value["version"]=serde_json::json!(2);assert!(unpack(&serde_json::to_vec(&value).unwrap()).is_err());
 }
 #[test]
 fn builtin_only_schemes_need_no_external_resources(){
  let root=tempfile::tempdir().unwrap();let recipe=parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","advanced":{"lookup":{"id":"builtin:warm","amount":50,"enabled":true}}})).unwrap();
  let bytes=pack(root.path(),recipe.clone()).unwrap();let (restored,resources)=unpack(&bytes).unwrap();assert_eq!(recipe,restored);assert!(resources.is_empty());
 }
}
#[tauri::command]
pub async fn edit_scheme_save(state:State<'_,SharedState>,library_id:String,asset_id:String,recipe:serde_json::Value,path:String)->Result<(),String>{
    run_blocking(state.inner().clone(),move |state|{
        super::project::resolve(state,&library_id,&asset_id)?;
        let target=Path::new(&path);
        if target.extension().and_then(|e|e.to_str()).is_none_or(|e|!e.eq_ignore_ascii_case("photoedit")){return Err("请使用 .photoedit 编辑方案文件名".into());}
        let bytes=pack(&state.config_dir.join("luts"),parse_recipe(&recipe)?)?;
        lut::atomic_write(target,&bytes)
    }).await
}
#[tauri::command]
pub async fn edit_scheme_open(state:State<'_,SharedState>,library_id:String,asset_id:String,path:String)->Result<EditRecipe,String>{
    run_blocking(state.inner().clone(),move |state|{
        super::project::resolve(state,&library_id,&asset_id)?;
        let file=std::fs::File::open(&path).map_err(|e|format!("读取编辑方案失败：{e}"))?;
        let mut bytes=Vec::new();file.take(MAX_BYTES+1).read_to_end(&mut bytes).map_err(|e|e.to_string())?;
        let (recipe,resources)=unpack(&bytes)?;let scheme:Scheme=serde_json::from_slice(&bytes).map_err(|e|e.to_string())?;super::film::install(&scheme.films)?;lut::install_packaged(&state.config_dir.join("luts"),&resources)?;
        Ok(recipe)
    }).await
}
