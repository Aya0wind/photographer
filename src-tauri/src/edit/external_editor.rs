//! External applications edit a managed copy. Originals and their recipes remain untouched.
use crate::ipc::{run_blocking, SharedState};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::State;
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalSession {
    pub id: i64,
    pub path: String,
    pub status: String,
    pub imported_asset_id: Option<i64>,
    pub error: Option<String>,
}
fn sessions(db: &crate::db::Db, id: i64) -> Result<Vec<ExternalSession>, String> {
    let mut stmt=db.0.prepare("SELECT e.id,e.path,CASE WHEN e.imported_asset_id IS NOT NULL THEN 'imported' ELSE j.status END,e.imported_asset_id,j.error FROM editor_external e JOIN export_job j ON j.id=e.id WHERE e.asset_id=? ORDER BY e.id DESC LIMIT 100").map_err(|e|e.to_string())?;
    let rows = stmt
        .query_map([id], |r| {
            Ok(ExternalSession {
                id: r.get(0)?,
                path: r.get(1)?,
                status: r.get(2)?,
                imported_asset_id: r.get(3)?,
                error: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn edit_external_list(
    state: State<'_, SharedState>,
    asset_id: String,
    library_id: String,
) -> Result<Vec<ExternalSession>, String> {
    run_blocking(state.inner().clone(), move |state| {
        let (_, asset) = super::project::resolve(state, &library_id, &asset_id)?;
        let db = crate::ipc::app_database_db(state)?;
        super::export::reap_orphan_jobs(&db);
        sessions(
            &db,
            db.asset_id_by_path(&asset.path)
                .map_err(|e| e.to_string())?
                .ok_or("照片不存在")?,
        )
    })
    .await
}
#[tauri::command]
pub async fn edit_external_prepare(
    state: State<'_, SharedState>,
    asset_id: String,
    library_id: String,
    session_id: String,
    recipe: serde_json::Value,
    color_space: super::export::ExportColorSpace,
) -> Result<super::export::ExportTaskDto, String> {
    run_blocking(state.inner().clone(), move |state| {
        let (_, asset) = super::project::resolve(state, &library_id, &asset_id)?;
        super::preview::validate_source(&session_id, Path::new(&asset.path))?;
        let directory = crate::ipc::app_database_dir(state)?.join("external-edits");
        let filename = format!("{}.tif", uuid::Uuid::new_v4());
        let path = directory.join(&filename);
        let options = super::export::ExportOptions {
            file_name: None,
            finish: Default::default(),
            format: super::export::ExportFormat::Tiff,
            color_space,
            mode: super::export::ExportMode::Folder,
            folder: Some(super::export::ExportFolderTarget {
                output_dir: directory.to_string_lossy().into_owned(),
                file_name: filename,
            }),
            album: None,
            long_edge: None,
            quality: None,
            remove_gps: false,
            copyright: None,
            author: None,
            keywords: vec![],
        };
        let id = super::ipc::parse_asset_id(&asset_id)?;
        let task = super::ipc::fetch_export_operation(
            state,
            id,
            &recipe,
            &options,
            Some(&library_id),
            None,
        )?;
        if let Err(error) = crate::ipc::app_database_db(state)?.0.execute(
            "INSERT INTO editor_external(id,asset_id,path) VALUES (?1,?2,?3)",
            params![task.id, id, path.to_string_lossy()],
        ) {
            let _ = super::export::cancel_job(task.id);
            return Err(error.to_string());
        }
        Ok(task)
    })
    .await
}
fn resolve(db: &crate::db::Db, asset_id: i64, job_id: i64) -> Result<PathBuf, String> {
    let row:Option<(String,String)>=db.0.query_row("SELECT e.path,j.status FROM editor_external e JOIN export_job j ON j.id=e.id WHERE e.id=?1 AND e.asset_id=?2",params![job_id,asset_id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?;
    let (path, status) = row.ok_or("外部编辑会话不存在")?;
    if status != "done" {
        return Err("请等待外部编辑副本准备完成".into());
    }
    Ok(path.into())
}
fn fingerprint(path: &Path) -> Result<String, String> {
    use sha2::Digest;
    let data = read_stable(path)?;
    Ok(format!("{:x}", sha2::Sha256::digest(&data)))
}
fn read_stable(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let before = std::fs::metadata(path).map_err(|e| format!("编辑副本不可用：{e}"))?;
    if !before.is_file() || before.len() > 512 * 1024 * 1024 {
        return Err("外部编辑副本需要为不超过 512 MB 的 TIFF 文件".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(512 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let after = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if bytes.len() as u64 != before.len()
        || after.len() != before.len()
        || after.modified().ok() != before.modified().ok()
    {
        return Err("外部应用仍在写入，请完成保存后重试".into());
    }
    Ok(bytes)
}
#[tauri::command]
pub async fn edit_external_launch(
    state: State<'_, SharedState>,
    asset_id: String,
    library_id: String,
    job_id: i64,
    program: String,
) -> Result<(), String> {
    run_blocking(state.inner().clone(), move |state| {
        super::project::resolve(state, &library_id, &asset_id)?;
        let id = super::ipc::parse_asset_id(&asset_id)?;
        let db = crate::ipc::app_database_db(state)?;
        let path = resolve(&db, id, job_id)?;
        let baseline = fingerprint(&path)?;
        let executable = Path::new(&program);
        if !executable.is_absolute() || !executable.exists() {
            return Err("请选择本机编辑器程序".into());
        }
        #[cfg(windows)]
        if !executable.is_file()
            || executable
                .extension()
                .and_then(|v| v.to_str())
                .is_none_or(|v| !v.eq_ignore_ascii_case("exe"))
        {
            return Err("请选择 .exe 应用程序".into());
        }
        // Store the baseline once; reopening must not hide changes already made.
        db.0.execute(
            "UPDATE editor_external SET baseline=COALESCE(baseline,?2) WHERE id=?1",
            params![job_id, baseline],
        )
        .map_err(|e| e.to_string())?;
        let mut command = if cfg!(target_os = "macos") {
            let mut c = std::process::Command::new("/usr/bin/open");
            c.arg("-a").arg(executable).arg("--");
            c
        } else {
            std::process::Command::new(executable)
        };
        command
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("启动编辑器失败：{e}"))?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    })
    .await
}
/// Recover the publication/receipt gap after a crash using the session's unique filename
/// and the complete saved content. A failed receipt update must not publish a second asset.
fn published_copy(
    db: &crate::db::Db,
    library_id: &str,
    stem: &str,
    digest: &str,
) -> Result<Option<(PathBuf, i64)>, String> {
    let mut stmt =
        db.0.prepare(
            "SELECT path,id FROM assets WHERE library_id=?1 AND filename GLOB ?2 AND in_trash=0",
        )
        .map_err(|e| e.to_string())?;
    let candidates = stmt
        .query_map(
            params![library_id, format!("external_{stem}_edit*.tif")],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
        )
        .map_err(|e| e.to_string())?;
    for row in candidates {
        let (path, id) = row.map_err(|e| e.to_string())?;
        let path = PathBuf::from(path);
        if fingerprint(&path).is_ok_and(|value| value == digest) {
            return Ok(Some((path, id)));
        }
    }
    Ok(None)
}
#[tauri::command]
pub async fn edit_external_import(
    state: State<'_, SharedState>,
    asset_id: String,
    library_id: String,
    job_id: i64,
) -> Result<super::export::ExportResultDto, String> {
    run_blocking(state.inner().clone(), move |state| {
        static IMPORT: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = IMPORT.lock().map_err(|_| "外部编辑导入锁不可用")?;
        let (library, mut asset) = super::project::resolve(state, &library_id, &asset_id)?;
        let id = super::ipc::parse_asset_id(&asset_id)?;
        let db = crate::ipc::app_database_db(state)?;
        let path = resolve(&db, id, job_id)?;
        let (baseline, imported): (Option<String>, Option<i64>) =
            db.0.query_row(
                "SELECT baseline,imported_asset_id FROM editor_external WHERE id=?1",
                [job_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| e.to_string())?;
        if imported.is_some() {
            return Err("此编辑副本已经导入；再次编辑请准备新副本".into());
        }
        let bytes = read_stable(&path)?;
        use sha2::Digest;
        let digest = format!("{:x}", sha2::Sha256::digest(&bytes));
        if baseline.as_deref().is_none_or(|b| b == digest) {
            return Err("尚未检测到编辑器保存的变化，请保存后再导回".into());
        }
        let reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
            .with_guessed_format()
            .map_err(|e| e.to_string())?;
        if reader.format() != Some(image::ImageFormat::Tiff) {
            return Err("请将编辑副本保存为 TIFF，而不是仅改扩展名".into());
        }
        let (w, h) = reader
            .into_dimensions()
            .map_err(|e| format!("编辑副本尚不完整：{e}"))?;
        if u64::from(w) * u64::from(h) > 128_000_000 {
            return Err("外部编辑成片超过 1.28 亿像素".into());
        }
        let decoded = photocraft_io::import("external.tif", &bytes)
            .map_err(|e| format!("编辑副本尚不完整：{e}"))?;
        if decoded.document.size.width != w || decoded.document.size.height != h {
            return Err("编辑副本尺寸校验失败".into());
        }
        drop(decoded);
        if fingerprint(&path)? != digest {
            return Err("编辑副本仍在变化，请完成保存后重试".into());
        }
        asset.filename = format!(
            "external_{}.tif",
            path.file_stem()
                .ok_or("编辑副本路径无效")?
                .to_string_lossy()
        );
        let options = super::export::ValidatedOptions {
            file_name: None,
            finish: Default::default(),
            format: super::export::ExportFormat::Tiff,
            color_space: Default::default(),
            mode: super::export::ExportMode::Album,
            folder: None,
            album: None,
            long_edge: None,
            quality: None,
            remove_gps: false,
            copyright: None,
            author: None,
            keywords: vec![],
        };
        let recovered = published_copy(
            &db,
            &library_id,
            &path
                .file_stem()
                .ok_or("编辑副本路径无效")?
                .to_string_lossy(),
            &digest,
        )?;
        let (output, new_id) = if let Some(published) = recovered {
            published
        } else {
            super::export::place_in_album(
                &db,
                &bytes,
                &asset,
                &options,
                Path::new(&library.root_path),
                w,
                h,
                None,
                None,
            )?
        };
        db.0.execute(
            "UPDATE editor_external SET imported_asset_id=?2 WHERE id=?1",
            params![job_id, new_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(super::export::ExportResultDto {
            output_path: output.to_string_lossy().into_owned(),
            width: w,
            height: h,
            bytes: bytes.len() as u64,
            asset_id: Some(new_id),
        })
    })
    .await
}
