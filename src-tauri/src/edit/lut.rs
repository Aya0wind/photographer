//! App-owned creative LUT resources; parsing/interpolation belong to PhotoCraft.
use crate::ipc::{run_blocking, SharedState};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
use tauri::State;

static ROOT: OnceLock<PathBuf> = OnceLock::new();
static STORE: Mutex<()> = Mutex::new(());
const MAX_BYTES: u64 = 16 * 1024 * 1024;

// lib.rs 装配调用；edit_export_test 经 #[path] 直含 edit 模块时无调用方。
#[allow(dead_code)]
pub(crate) fn init(config: &Path) {
    let _ = ROOT.set(config.join("luts"));
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LutEntry {
    pub id: String,
    pub name: String,
    pub size: usize,
    pub builtin: bool,
    #[serde(default)]
    pub removed: bool,
}
fn catalog(root: &Path) -> Result<Vec<LutEntry>, String> {
    match std::fs::read(root.join("catalog.json")) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|e| format!("读取调色方案列表失败: {e}"))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(e.to_string()),
    }
}
fn atomic_write(target: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::create_dir_all(target.parent().ok_or("资源目录无效")?).map_err(|e| e.to_string())?;
    let temporary = target.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result =
        std::fs::write(&temporary, bytes).and_then(|_| std::fs::rename(&temporary, target));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(|e| format!("保存调色方案失败: {e}"))
}
fn save(root: &Path, entries: &[LutEntry]) -> Result<(), String> {
    atomic_write(
        &root.join("catalog.json"),
        &serde_json::to_vec_pretty(entries).map_err(|e| e.to_string())?,
    )
}
pub(super) fn validate_id(id: &str) -> Result<(), String> {
    if let Some(builtin) = id.strip_prefix("builtin:") {
        if photocraft_cms::lutfile::BUILTIN
            .iter()
            .any(|(key, _)| *key == builtin)
        {
            return Ok(());
        }
    } else if id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(());
    }
    Err("调色方案标识无效".into())
}
pub(super) fn params(id: &str) -> Result<serde_json::Value, String> {
    validate_id(id)?;
    if let Some(builtin) = id.strip_prefix("builtin:") {
        return Ok(serde_json::json!({"lut":builtin,"interpolation":"tetrahedral"}));
    }
    let root = ROOT.get().ok_or("调色方案目录未初始化")?;
    Ok(
        serde_json::json!({"file":root.join(format!("{id}.cube")).to_string_lossy(),"interpolation":"tetrahedral"}),
    )
}
fn import_at(root: &Path, path: &Path) -> Result<LutEntry, String> {
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_BYTES {
        return Err("请选择不超过 16 MB 的有效 LUT 文件".into());
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("调色方案文件名无效")?;
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let parsed = photocraft_cms::lutfile::parse(name, &bytes)
        .map_err(|e| format!("无法读取调色方案: {}", e.0))?;
    if parsed.size > 65 {
        return Err("此调色方案尺寸过大，暂支持最高 65 阶".into());
    }
    if !parsed.domain_is_default() {
        return Err("此调色方案使用非标准输入范围，当前引擎不能准确应用".into());
    }
    let canonical = photocraft_cms::lutfile::write_cube(&parsed);
    let id = format!("{:x}", Sha256::digest(canonical.as_bytes()));
    let target = root.join(format!("{id}.cube"));
    if !target.is_file() {
        atomic_write(&target, canonical.as_bytes())?;
    }
    let mut entries = catalog(root)?;
    if let Some(existing) = entries.iter_mut().find(|entry| entry.id == id) {
        existing.removed = false;
        let result = existing.clone();
        save(root, &entries)?;
        return Ok(result);
    }
    let entry = LutEntry {
        id,
        name: if parsed.title.trim().is_empty() {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else {
            parsed.title
        },
        size: parsed.size,
        builtin: false,
        removed: false,
    };
    entries.push(entry.clone());
    save(root, &entries)?;
    Ok(entry)
}
fn mutate_at(root: &Path, id: &str, name: Option<&str>) -> Result<(), String> {
    validate_id(id)?;
    if id.starts_with("builtin:") {
        return Err("内置调色方案不能修改或移除".into());
    }
    let mut entries = catalog(root)?;
    let entry = entries
        .iter_mut()
        .find(|entry| entry.id == id)
        .ok_or("调色方案不存在")?;
    if let Some(name) = name {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 120 {
            return Err("名称不能为空，且最多 120 个字符".into());
        }
        entry.name = name.into();
    } else {
        // Hide from the resource list, retain immutable payload for saved recipes.
        entry.removed = true;
    }
    save(root, &entries)
}
#[tauri::command]
pub async fn edit_lut_list(state: State<'_, SharedState>) -> Result<Vec<LutEntry>, String> {
    run_blocking(state.inner().clone(), move |state| {
        let _guard = STORE.lock().map_err(|_| "资源锁不可用")?;
        let mut entries: Vec<LutEntry> = photocraft_cms::lutfile::BUILTIN
            .iter()
            .map(|(id, name)| LutEntry {
                id: format!("builtin:{id}"),
                name: (*name).into(),
                size: 33,
                builtin: true,
                removed: false,
            })
            .collect();
        entries.extend(
            catalog(&state.config_dir.join("luts"))?
                .into_iter()
                .filter(|e| !e.removed),
        );
        Ok(entries)
    })
    .await
}
#[tauri::command]
pub async fn edit_lut_import(
    state: State<'_, SharedState>,
    path: String,
) -> Result<LutEntry, String> {
    run_blocking(state.inner().clone(), move |state| {
        let _guard = STORE.lock().map_err(|_| "资源锁不可用")?;
        import_at(&state.config_dir.join("luts"), Path::new(&path))
    })
    .await
}
#[tauri::command]
pub async fn edit_lut_rename(
    state: State<'_, SharedState>,
    id: String,
    name: String,
) -> Result<(), String> {
    run_blocking(state.inner().clone(), move |state| {
        let _guard = STORE.lock().map_err(|_| "资源锁不可用")?;
        mutate_at(&state.config_dir.join("luts"), &id, Some(&name))
    })
    .await
}
#[tauri::command]
pub async fn edit_lut_remove(state: State<'_, SharedState>, id: String) -> Result<(), String> {
    run_blocking(state.inner().clone(), move |state| {
        let _guard = STORE.lock().map_err(|_| "资源锁不可用")?;
        mutate_at(&state.config_dir.join("luts"), &id, None)
    })
    .await
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn imports_deduplicate_and_removal_keeps_saved_project_payload() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("luts");
        let source = dir.path().join("My look.cube");
        let lut = photocraft_cms::lutfile::LutFile::identity(2);
        std::fs::write(&source, photocraft_cms::lutfile::write_cube(&lut)).unwrap();
        let first = import_at(&root, &source).unwrap();
        let second = import_at(&root, &source).unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(catalog(&root).unwrap().len(), 1);
        mutate_at(&root, &first.id, Some("New name")).unwrap();
        assert_eq!(catalog(&root).unwrap()[0].name, "New name");
        mutate_at(&root, &first.id, None).unwrap();
        assert!(catalog(&root).unwrap()[0].removed);
        assert!(root.join(format!("{}.cube", first.id)).is_file());
        import_at(&root, &source).unwrap();
        assert!(!catalog(&root).unwrap()[0].removed);
    }
    #[test]
    fn rejects_unsafe_identifiers_and_non_default_domains() {
        assert!(params("../../outside").is_err());
        assert!(params("builtin:unknown").is_err());
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("custom.cube");
        let mut lut = photocraft_cms::lutfile::LutFile::identity(2);
        lut.domain_max = [2.0; 3];
        std::fs::write(&file, format!("DOMAIN_MAX 2 2 2\n{}",photocraft_cms::lutfile::write_cube(&lut))).unwrap();
        assert!(import_at(&dir.path().join("luts"), &file)
            .unwrap_err()
            .contains("输入范围"));
    }
}
