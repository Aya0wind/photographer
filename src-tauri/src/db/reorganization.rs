//! Opt-in file organization. Plan before moving, never overwrite, never remove
//! directories, and keep a durable per-file journal in the database.
use super::Db;
use rusqlite::params;
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
};
use xxhash_rust::xxh64::Xxh64;

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub library_id: String,
    pub phase: String,
    pub completed: u64,
    pub total: u64,
}
#[derive(Debug)]
struct Move {
    source: PathBuf,
    destination: PathBuf,
    base_destination: PathBuf,
    size: u64,
    hash: String,
    group_key: String,
}
fn key(path: &Path) -> String {
    let value = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value
    }
}
fn hash_file(path: &Path) -> Result<(u64, String), String> {
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("读取照片失败 {}：{e}", path.display()))?;
    let mut hash = Xxh64::new(0);
    let mut size = 0;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        size += n as u64;
    }
    Ok((size, hash.digest().to_string()))
}
fn walk(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in std::fs::read_dir(root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            walk(&entry.path(), files)?;
        } else if kind.is_file() {
            let ext = entry
                .path()
                .extension()
                .map(|s| s.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if crate::devices::is_media_ext(&ext) || ext == "xmp" {
                files.push(entry.path());
            }
        }
    }
    Ok(())
}
fn photo(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| crate::devices::is_media_ext(&e.to_string_lossy()))
}
fn group_stem(path: &Path) -> String {
    let stem = path.file_stem().unwrap_or_default();
    let stem_path = Path::new(stem);
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("xmp"))
        && stem_path
            .extension()
            .is_some_and(|ext| crate::devices::is_media_ext(&ext.to_string_lossy()))
    {
        stem_path
            .file_stem()
            .unwrap_or(stem)
            .to_string_lossy()
            .to_lowercase()
    } else {
        stem.to_string_lossy().to_lowercase()
    }
}
fn suffixed(path: &Path, n: u32) -> String {
    let name = path.file_name().unwrap().to_string_lossy();
    if n == 0 {
        return name.into_owned();
    }
    let stem = path.file_stem().unwrap().to_string_lossy();
    let ext = path.extension().unwrap().to_string_lossy();
    format!("{stem}_{n}.{ext}")
}
fn validate_destination(root: &Path, path: &Path) -> Result<(), String> {
    if !path.starts_with(root) {
        return Err("整理目标超出照片库".into());
    }
    let mut parent = path.parent().ok_or("整理路径无效")?;
    while !parent.exists() {
        parent = parent.parent().ok_or("整理路径无效")?;
    }
    let real = std::fs::canonicalize(parent).map_err(|e| e.to_string())?;
    if !real.starts_with(root) || !real.is_dir() {
        return Err("整理目录包含指向照片库外的链接或被文件占用".into());
    }
    Ok(())
}
fn plan(root: &Path, template: &str) -> Result<Vec<Move>, String> {
    super::organization::validate_organization_template(template)?;
    let mut files = Vec::new();
    walk(root, &mut files)?;
    files.sort();
    let mut reserved: HashSet<_> = files.iter().map(|p| key(p)).collect();
    let mut groups = BTreeMap::<(PathBuf, String), Vec<PathBuf>>::new();
    for path in files {
        groups
            .entry((path.parent().unwrap().to_path_buf(), group_stem(&path)))
            .or_default()
            .push(path);
    }
    let mut result = Vec::new();
    for ((parent, stem), mut group) in groups {
        let group_key = format!("{}\0{stem}", key(&parent));
        group.sort_by_key(|path| {
            (
                !photo(path),
                path.extension()
                    .map(|e| e.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default()
                    != "jpg",
                path.clone(),
            )
        });
        let mut captured = None;
        let mut camera = None;
        for path in group.iter().filter(|path| photo(path)) {
            let mut head = Vec::new();
            std::fs::File::open(path)
                .map_err(|e| e.to_string())?
                .take(2 * 1024 * 1024)
                .read_to_end(&mut head)
                .map_err(|e| e.to_string())?;
            let meta = crate::metadata::exif_lite::parse(&head);
            captured = captured.or(meta.captured_at);
            camera = camera.or(meta.camera);
        }
        let fallback = std::fs::metadata(&group[0])
            .and_then(|m| m.modified())
            .map_err(|e| e.to_string())?;
        let fallback: chrono::DateTime<chrono::Utc> = fallback.into();
        let captured = captured.unwrap_or_else(|| {
            fallback
                .with_timezone(&chrono::Local)
                .naive_local()
                .and_utc()
        });
        let mut suffix = 0u32;
        let mut base_destinations = Vec::new();
        let destinations = loop {
            let mut destinations = Vec::new();
            for path in &group {
                let destination = if photo(path) || !group.iter().any(|p| photo(p)) {
                    root.join(super::organization::organized_rel(
                        template,
                        captured,
                        &suffixed(path, suffix),
                        camera.as_deref(),
                    )?)
                } else {
                    let stem_path = PathBuf::from(path.file_stem().unwrap());
                    let compound = stem_path
                        .extension()
                        .is_some_and(|ext| crate::devices::is_media_ext(&ext.to_string_lossy()));
                    let partner = if compound {
                        group
                            .iter()
                            .position(|p| {
                                p.extension().is_some_and(|e| {
                                    e.to_string_lossy().eq_ignore_ascii_case(
                                        &stem_path.extension().unwrap().to_string_lossy(),
                                    )
                                })
                            })
                            .unwrap_or(0)
                    } else {
                        0
                    };
                    let partner_destination: &PathBuf =
                        destinations.get(partner).ok_or("XMP 配套照片路径无效")?;
                    if compound {
                        partner_destination.with_file_name(format!(
                            "{}.xmp",
                            partner_destination.file_name().unwrap().to_string_lossy()
                        ))
                    } else {
                        partner_destination.with_extension("xmp")
                    }
                };
                validate_destination(root, &destination)?;
                destinations.push(destination);
            }
            let mut seen = HashSet::new();
            if suffix == 0 {
                base_destinations = destinations.clone();
            }
            if destinations
                .iter()
                .any(|destination| !seen.insert(key(destination)))
            {
                return Err("同组文件生成了重复目标路径，请调整组织模板或检查重复 XMP 文件".into());
            }
            let conflict = group
                .iter()
                .zip(&destinations)
                .any(|(source, destination)| {
                    key(source) != key(destination)
                        && (destination.exists() || reserved.contains(&key(destination)))
                });
            if !conflict {
                break destinations;
            }
            suffix = suffix.checked_add(1).ok_or("整理重名序号耗尽")?;
        };
        for ((source, destination), base_destination) in
            group.into_iter().zip(destinations).zip(base_destinations)
        {
            reserved.insert(key(&destination));
            if key(&source) == key(&destination) {
                continue;
            }
            let (size, hash) = hash_file(&source)?;
            result.push(Move {
                source,
                destination,
                base_destination,
                size,
                hash,
                group_key: group_key.clone(),
            });
        }
    }
    Ok(result)
}
#[cfg(windows)]
fn move_noclobber(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    let source: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<_> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    unsafe {
        windows::Win32::Storage::FileSystem::MoveFileW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
        )
    }
    .map_err(|e| e.to_string())
}
#[cfg(not(windows))]
fn move_noclobber(source: &Path, destination: &Path) -> Result<(), String> {
    // Link publication is exclusive. If interruption happens before unlink,
    // resume only accepts two paths referring to the same inode.
    std::fs::hard_link(source, destination).map_err(|e| e.to_string())?;
    std::fs::remove_file(source).map_err(|e| e.to_string())
}
#[cfg(unix)]
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}
#[cfg(not(unix))]
fn same_file(_a: &Path, _b: &Path) -> bool {
    false
}

fn retarget_group(db: &Db, id: &str, group_key: &str) -> Result<(), String> {
    let rows:Vec<(i64,PathBuf,PathBuf,bool,u64,String,PathBuf)>=db.0.prepare("SELECT seq,source,destination,done,size,xxhash,base_destination FROM library_organization_moves WHERE library_id=?1 AND group_key=?2 ORDER BY seq").and_then(|mut stmt|stmt.query_map(params![id,group_key],|row|Ok((row.get(0)?,PathBuf::from(row.get::<_,String>(1)?),PathBuf::from(row.get::<_,String>(2)?),row.get(3)?,row.get::<_,i64>(4)? as u64,row.get(5)?,PathBuf::from(row.get::<_,String>(6)?))))?.collect()).map_err(|e|e.to_string())?;
    let all_destinations: HashSet<String> =
        db.0.prepare("SELECT destination FROM library_organization_moves WHERE library_id=?1")
            .and_then(|mut stmt| {
                stmt.query_map([id], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|path| key(Path::new(&path)))
            .collect();
    for (_, source, destination, done, size, hash, _) in &rows {
        let current = if *done { destination } else { source };
        if hash_file(current)? != (*size, hash.clone()) {
            return Err("重名处理期间文件发生变化，已停止整理".into());
        }
    }
    let mut n = 1u32;
    let targets = loop {
        let targets: Vec<_> = rows
            .iter()
            .map(|(_, _, _, _, _, _, destination)| {
                let stem = PathBuf::from(destination.file_stem().unwrap());
                if destination
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("xmp"))
                    && stem
                        .extension()
                        .is_some_and(|e| crate::devices::is_media_ext(&e.to_string_lossy()))
                {
                    destination.with_file_name(format!("{}.xmp", suffixed(&stem, n)))
                } else {
                    destination.with_file_name(suffixed(destination, n))
                }
            })
            .collect();
        if targets
            .iter()
            .all(|target| !target.exists() && !all_destinations.contains(&key(target)))
        {
            break targets;
        }
        n = n.checked_add(1).ok_or("重名序号耗尽")?;
    };
    let tx = db.0.unchecked_transaction().map_err(|e| e.to_string())?;
    for ((seq, source, destination, done, _, _, _), target) in rows.into_iter().zip(targets) {
        let source = if done { destination } else { source };
        tx.execute("UPDATE library_organization_moves SET source=?3,destination=?4,done=0 WHERE library_id=?1 AND seq=?2",params![id,seq,source.to_string_lossy(),target.to_string_lossy()]).map_err(|e|e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

pub fn run(db: &Db, id: &str, progress: &dyn Fn(Progress)) -> Result<Progress, String> {
    let library = db
        .photos_library_get(id)
        .map_err(|e| e.to_string())?
        .ok_or("照片库不存在")?;
    let root = std::fs::canonicalize(&library.root_path).map_err(|e| e.to_string())?;
    let job: Option<(String, String)> =
        db.0.query_row(
            "SELECT template,status FROM library_organization_jobs WHERE library_id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map(Some)
        .or_else(|e| {
            if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
                Ok(None)
            } else {
                Err(e)
            }
        })
        .map_err(|e| e.to_string())?;
    let (template, status) = job.ok_or("没有等待执行的照片整理任务")?;
    if status == "done" {
        return Ok(Progress {
            library_id: id.into(),
            phase: "done".into(),
            ..Default::default()
        });
    }
    if status=="cancelled"{return Err("整理任务已停止".into());}
    let execute = || -> Result<Progress, String> {
        let count: i64 =
            db.0.query_row(
                "SELECT COUNT(*) FROM library_organization_moves WHERE library_id=?1",
                [id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if count == 0 {
            progress(Progress {
                library_id: id.into(),
                phase: "scan".into(),
                ..Default::default()
            });
            let moves = plan(&root, &template)?;
            let tx = db.0.unchecked_transaction().map_err(|e| e.to_string())?;
            for (seq, movement) in moves.iter().enumerate() {
                tx.execute("INSERT INTO library_organization_moves(library_id,seq,source,destination,size,xxhash,group_key,base_destination) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![id,seq as i64,movement.source.to_string_lossy(),movement.destination.to_string_lossy(),movement.size as i64,movement.hash,movement.group_key,movement.base_destination.to_string_lossy()]).map_err(|e|e.to_string())?;
            }
            tx.commit().map_err(|e| e.to_string())?;
        }
        db.0.execute(
            "UPDATE library_organization_jobs SET status='moving',error=NULL WHERE library_id=?1",
            [id],
        )
        .map_err(|e| e.to_string())?;
        let total: u64 =
            db.0.query_row(
                "SELECT COUNT(*) FROM library_organization_moves WHERE library_id=?1",
                [id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        let mut completed: u64 =
            db.0.query_row(
                "SELECT COUNT(*) FROM library_organization_moves WHERE library_id=?1 AND done=1",
                [id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        progress(Progress {
            library_id: id.into(),
            phase: "move".into(),
            completed,
            total,
        });
        loop {
            let row=db.0.query_row("SELECT seq,source,destination,size,xxhash,group_key,base_destination FROM library_organization_moves WHERE library_id=?1 AND done=0 ORDER BY seq LIMIT 1",[id],|row|Ok((row.get::<_,i64>(0)?,Move{source:PathBuf::from(row.get::<_,String>(1)?),destination:PathBuf::from(row.get::<_,String>(2)?),size:row.get::<_,i64>(3)? as u64,hash:row.get(4)?,group_key:row.get(5)?,base_destination:PathBuf::from(row.get::<_,String>(6)?)}))).map(Some).or_else(|e|if matches!(e,rusqlite::Error::QueryReturnedNoRows){Ok(None)}else{Err(e)}).map_err(|e|e.to_string())?;
            let Some((seq, movement)) = row else {
                break;
            };
            validate_destination(&root, &movement.destination)?;
            if !movement.source.starts_with(&root) {
                return Err("整理源文件超出照片库".into());
            }
            if movement.source.exists() {
                if std::fs::symlink_metadata(&movement.source)
                    .map_err(|e| e.to_string())?
                    .file_type()
                    .is_symlink()
                {
                    return Err("整理期间源文件变成了链接，已停止".into());
                }
                if hash_file(&movement.source)? != (movement.size, movement.hash.clone()) {
                    return Err(format!(
                        "照片在整理期间发生变化，已停止：{}",
                        movement.source.display()
                    ));
                }
                if movement.destination.exists() {
                    if same_file(&movement.source, &movement.destination) {
                        std::fs::remove_file(&movement.source).map_err(|e| e.to_string())?;
                    } else {
                        retarget_group(db, id, &movement.group_key)?;
                        continue;
                    }
                } else {
                    std::fs::create_dir_all(movement.destination.parent().unwrap())
                        .map_err(|e| e.to_string())?;
                    validate_destination(&root, &movement.destination)?;
                    if let Err(error) = move_noclobber(&movement.source, &movement.destination) {
                        if movement.destination.exists() && movement.source.exists() {
                            retarget_group(db, id, &movement.group_key)?;
                            continue;
                        }
                        return Err(error);
                    }
                }
            } else if !movement.destination.is_file()
                || hash_file(&movement.destination)? != (movement.size, movement.hash.clone())
            {
                return Err(format!(
                    "整理文件缺失或内容不一致，已停止：{}",
                    movement.source.display()
                ));
            }
            let tx = db.0.unchecked_transaction().map_err(|e| e.to_string())?;
            let source_path = Path::new(&library.root_path).join(
                movement
                    .source
                    .strip_prefix(&root)
                    .map_err(|e| e.to_string())?,
            );
            let destination_path = Path::new(&library.root_path).join(
                movement
                    .destination
                    .strip_prefix(&root)
                    .map_err(|e| e.to_string())?,
            );
            if photo(&movement.destination) {
                tx.execute("UPDATE assets SET path=?1,filename=?2,thumb_state=0 WHERE library_id=?3 AND path=?4",params![destination_path.to_string_lossy(),destination_path.file_name().unwrap().to_string_lossy(),id,source_path.to_string_lossy()]).map_err(|e|e.to_string())?;
                tx.execute("UPDATE index_tasks SET state='pending',attempts=0 WHERE kind='thumb' AND asset_id IN(SELECT id FROM assets WHERE library_id=?1 AND path=?2)",params![id,destination_path.to_string_lossy()]).map_err(|e|e.to_string())?;
            }
            tx.execute(
                "UPDATE library_organization_moves SET done=1 WHERE library_id=?1 AND seq=?2",
                params![id, seq],
            )
            .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            completed=db.0.query_row("SELECT COUNT(*) FROM library_organization_moves WHERE library_id=?1 AND done=1",[id],|row|row.get(0)).map_err(|e|e.to_string())?;
            progress(Progress {
                library_id: id.into(),
                phase: "move".into(),
                completed,
                total,
            });
        }
        db.0.execute(
            "UPDATE library_organization_jobs SET status='done',error=NULL WHERE library_id=?1",
            [id],
        )
        .map_err(|e| e.to_string())?;
        Ok(Progress {
            library_id: id.into(),
            phase: "done".into(),
            completed,
            total,
        })
    };
    match execute() {
        Ok(result) => {
            progress(result.clone());
            Ok(result)
        }
        Err(error) => {
            let _ = db.0.execute(
                "UPDATE library_organization_jobs SET status='error',error=?2 WHERE library_id=?1",
                params![id, error],
            );
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (tempfile::TempDir, Db, PathBuf, String) {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let root = dir.path().join("photos");
        std::fs::create_dir_all(root.join("old/empty")).unwrap();
        let db = Db::open(&data.join("library.db")).unwrap();
        let row = db
            .photos_library_register_with_template(
                "Photos",
                &root.to_string_lossy(),
                &data,
                "{filename}",
            )
            .unwrap();
        db.photos_library_queue_organization(&row.id, "{filename}")
            .unwrap();
        (dir, db, root, row.id)
    }
    fn seed(root: &Path) {
        for (path, bytes) in [
            ("DSC.JPG", b"existing".as_slice()),
            ("old/DSC.JPG", b"incoming jpeg".as_slice()),
            ("old/DSC.ARW", b"incoming raw".as_slice()),
            ("old/DSC.xmp", b"sidecar".as_slice()),
            ("old/DSC.JPG.xmp", b"jpeg sidecar".as_slice()),
            ("old/readme.txt", b"keep me".as_slice()),
        ] {
            std::fs::write(root.join(path), bytes).unwrap();
        }
    }
    #[test]
    fn organizes_photos_and_sidecars_without_overwriting_or_deleting_directories() {
        let (_dir, db, root, id) = setup();
        seed(&root);
        let progress = run(&db, &id, &|_| {}).unwrap();
        assert_eq!(progress.completed, 4);
        for (name, bytes) in [
            ("DSC.JPG", b"existing".as_slice()),
            ("DSC_1.JPG", b"incoming jpeg".as_slice()),
            ("DSC_1.ARW", b"incoming raw".as_slice()),
            ("DSC_1.xmp", b"sidecar".as_slice()),
            ("DSC_1.JPG.xmp", b"jpeg sidecar".as_slice()),
            ("old/readme.txt", b"keep me".as_slice()),
        ] {
            assert_eq!(std::fs::read(root.join(name)).unwrap(), bytes, "{name}");
        }
        assert!(root.join("old").is_dir());
        assert!(root.join("old/empty").is_dir());
        assert!(!db.photos_library_organization_pending(&id).unwrap());
    }
    #[test]
    fn resumes_after_interruption_and_renames_the_whole_group_for_a_late_collision() {
        let (_dir, db, root, id) = setup();
        seed(&root);
        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run(&db, &id, &|progress| {
                if progress.completed == 1 {
                    panic!("simulated interruption");
                }
            })
        }));
        assert!(interrupted.is_err());
        assert!(db.photos_library_organization_pending(&id).unwrap());
        std::fs::write(root.join("DSC_1.ARW"), b"foreign file").unwrap();
        run(&db, &id, &|_| {}).unwrap();
        for (name, bytes) in [
            ("DSC_2.JPG", b"incoming jpeg".as_slice()),
            ("DSC_2.ARW", b"incoming raw".as_slice()),
            ("DSC_2.xmp", b"sidecar".as_slice()),
            ("DSC_2.JPG.xmp", b"jpeg sidecar".as_slice()),
            ("DSC_1.ARW", b"foreign file".as_slice()),
            ("DSC.JPG", b"existing".as_slice()),
        ] {
            assert_eq!(std::fs::read(root.join(name)).unwrap(), bytes, "{name}");
        }
        assert!(root.join("old/empty").is_dir());
    }
    #[test]
    fn recovers_a_move_completed_before_its_journal_update() {
        let (_dir, db, root, id) = setup();
        seed(&root);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run(&db, &id, &|progress| {
                if progress.completed == 1 {
                    panic!("stop");
                }
            })
        }));
        db.0.execute(
            "UPDATE library_organization_moves SET done=0 WHERE library_id=?1 AND done=1",
            [&id],
        )
        .unwrap();
        run(&db, &id, &|_| {}).unwrap();
        assert_eq!(
            std::fs::read(root.join("DSC_1.JPG")).unwrap(),
            b"incoming jpeg"
        );
        assert_eq!(std::fs::read(root.join("DSC_1.xmp")).unwrap(), b"sidecar");
    }
    #[test]
    fn changed_photo_stops_without_discarding_the_changed_file_or_other_files() {
        let (_dir, db, root, id) = setup();
        seed(&root);
        let error = run(&db, &id, &|progress| {
            if progress.phase == "move" && progress.completed == 0 {
                std::fs::write(root.join("old/DSC.JPG"), b"changed by user").unwrap();
            }
        })
        .unwrap_err();
        assert!(error.contains("发生变化"));
        assert_eq!(
            std::fs::read(root.join("old/DSC.JPG")).unwrap(),
            b"changed by user"
        );
        assert_eq!(
            std::fs::read(root.join("old/DSC.ARW")).unwrap(),
            b"incoming raw"
        );
        assert!(root.join("old/empty").is_dir());
    }
}
