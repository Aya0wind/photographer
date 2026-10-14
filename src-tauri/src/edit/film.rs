//! Immutable film resources preserve original Gamma/Gamut declarations.
use super::lut;
use crate::ipc::{run_blocking, SharedState};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
use tauri::State;
static ROOT: OnceLock<PathBuf> = OnceLock::new();
static BUILTIN_ROOT: OnceLock<PathBuf> = OnceLock::new();
static STORE: Mutex<()> = Mutex::new(());
const MAX_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) fn init(config: &Path) {
    let _ = ROOT.set(config.join("films"));
}
pub(crate) fn init_builtins(resources: &Path) {
    let _ = BUILTIN_ROOT.set(resources.join("rawlab/films"));
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilmEntry {
    pub id: String,
    pub name: String,
    pub size: u32,
    pub input_transfer: u32,
    pub output_transfer: u32,
    #[serde(default)]
    pub builtin: bool,
    #[serde(default)]
    pub subtitle: String,
    #[serde(default)]
    pub summary: String,
}
fn root() -> Result<&'static PathBuf, String> {
    ROOT.get().ok_or_else(|| "胶片资源目录未初始化".into())
}
pub(super) fn validate_id(id: &str) -> Result<(), String> {
    if id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err("胶片资源标识无效".into())
    }
}
pub(super) fn path(id: &str) -> Result<PathBuf, String> {
    validate_id(id)?;
    let path = root()?.join(format!("{id}.cube"));
    if path.is_file() {
        return Ok(path);
    }
    if builtin_catalog()?.iter().any(|entry| entry.id == id) {
        let path = BUILTIN_ROOT
            .get()
            .ok_or("内置胶片目录未初始化")?
            .join(format!("{id}.cube"));
        if path.is_file() {
            return Ok(path);
        }
        return Err("内置胶片资源缺失，请重新安装应用".into());
    }
    Err("胶片资源缺失，请重新导入对应 CUBE".into())
}
fn catalog_at(root: &Path) -> Result<Vec<FilmEntry>, String> {
    match std::fs::read(root.join("catalog.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(e.to_string()),
    }
}
fn builtin_catalog() -> Result<Vec<FilmEntry>, String> {
    BUILTIN_ROOT
        .get()
        .map(|root| catalog_at(root))
        .unwrap_or_else(|| Ok(vec![]))
}
fn catalog() -> Result<Vec<FilmEntry>, String> {
    let mut entries = builtin_catalog()?;
    for entry in catalog_at(root()?)? {
        if !entries.iter().any(|existing| existing.id == entry.id) {
            entries.push(entry);
        }
    }
    Ok(entries)
}
fn import_bytes(bytes: &[u8], name: &str) -> Result<FilmEntry, String> {
    if bytes.is_empty()
        || bytes.len() as u64 > MAX_BYTES
        || name.is_empty()
        || name.chars().count() > 120
    {
        return Err("胶片资源无效或超过 16 MB".into());
    }
    let _guard = STORE.lock().map_err(|_| "胶片资源锁不可用")?;
    let id = format!("{:x}", Sha256::digest(bytes));
    if let Some(entry) = builtin_catalog()?.into_iter().find(|entry| entry.id == id) {
        return Ok(entry);
    }
    let target = root()?.join(format!("{id}.cube"));
    // Validate the exact content-addressed bytes with core, never reserialize CUBE.
    let created = !target.is_file();
    if created {
        lut::atomic_write(&target, bytes)?;
    }
    if std::fs::read(&target).map_err(|e| e.to_string())? != bytes {
        return Err("胶片资源校验失败".into());
    }
    let info = super::rawlab::inspect_film(&target).map_err(|error| {
        if created {
            let _ = std::fs::remove_file(&target);
        }
        error
    })?;
    let entry = FilmEntry {
        id,
        name: name.to_owned(),
        size: info.size,
        input_transfer: info.input_transfer,
        output_transfer: info.output_transfer,
        builtin: false,
        subtitle: String::new(),
        summary: String::new(),
    };
    let mut entries = catalog_at(root()?)?;
    if let Some(existing) = entries.iter().find(|e| e.id == entry.id) {
        return Ok(existing.clone());
    }
    entries.push(entry.clone());
    lut::atomic_write(
        &root()?.join("catalog.json"),
        &serde_json::to_vec(&entries).map_err(|e| e.to_string())?,
    )?;
    Ok(entry)
}
#[tauri::command]
pub async fn edit_film_list(state: State<'_, SharedState>) -> Result<Vec<FilmEntry>, String> {
    run_blocking(state.inner().clone(), move |_| catalog()).await
}
#[tauri::command]
pub async fn edit_film_import(
    state: State<'_, SharedState>,
    path: String,
) -> Result<FilmEntry, String> {
    run_blocking(state.inner().clone(), move |_| {
        let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let name = Path::new(&path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Film");
        import_bytes(&bytes, name)
    })
    .await
}
#[derive(Serialize, Deserialize)]
pub(super) struct PackagedFilm {
    pub id: String,
    pub name: String,
    pub cube: String,
}
pub(super) fn package(recipe: &super::recipe::EditRecipe) -> Result<Vec<PackagedFilm>, String> {
    let Some(film) = recipe.color_pipeline.as_ref().and_then(|p| p.film.as_ref()) else {
        return Ok(vec![]);
    };
    let mut cube = String::new();
    std::fs::File::open(path(&film.id)?)
        .map_err(|e| e.to_string())?
        .take(MAX_BYTES + 1)
        .read_to_string(&mut cube)
        .map_err(|e| e.to_string())?;
    if cube.len() as u64 > MAX_BYTES {
        return Err("胶片资源过大".into());
    }
    let name = catalog()?
        .into_iter()
        .find(|e| e.id == film.id)
        .map(|e| e.name)
        .unwrap_or_else(|| "Film".into());
    let resource = PackagedFilm {
        id: film.id.clone(),
        name,
        cube,
    };
    validate(&resource)?;
    Ok(vec![resource])
}
pub(super) fn validate(resource: &PackagedFilm) -> Result<(), String> {
    validate_id(&resource.id)?;
    if resource.cube.len() as u64 > MAX_BYTES
        || format!("{:x}", Sha256::digest(resource.cube.as_bytes())) != resource.id
    {
        return Err("方案胶片资源校验失败".into());
    }
    Ok(())
}
pub(super) fn install(resources: &[PackagedFilm]) -> Result<(), String> {
    for resource in resources {
        validate(resource)?;
    }
    for resource in resources {
        import_bytes(resource.cube.as_bytes(), &resource.name)?;
    }
    Ok(())
}
