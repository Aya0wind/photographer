//! In-place import must leave the source tree untouched and record ownership only in SQLite.

mod common;

pub use common::{
    ai, bursts, db, devices, events, geo, import, index, ipc, metadata, migrate, platform,
    settings, tasks, tethering, thumbs,
};

use std::fs;

use import::engine::ImportMode;

#[test]
fn reference_import_records_external_paths_without_writing_to_source_or_photo_root() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let database = temp.path().join("database");
    let photos = temp.path().join("photos");
    let originals = common::build_source(&source);
    fs::create_dir_all(&database).unwrap();
    fs::create_dir_all(&photos).unwrap();

    let (_, stats) = common::run_engine(&source, &database, &photos, |plan| {
        plan.mode = ImportMode::Reference;
    });
    assert_eq!(stats.done_files, originals.len() as u64);
    assert_eq!(stats.failed_files, 0);

    let db = common::open_db(&database);
    let mut rows =
        db.0.prepare("SELECT path, origin FROM assets ORDER BY path")
            .unwrap();
    let recorded = rows
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(recorded.len(), originals.len());
    for (path, origin) in recorded {
        assert_eq!(origin, "external");
        assert!(path.starts_with(source.to_str().unwrap()));
    }
    for (relative, content) in &originals {
        assert_eq!(fs::read(source.join(relative)).unwrap(), *content);
    }
    let source_files = walkdir::WalkDir::new(&source)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .count();
    assert_eq!(
        source_files,
        originals.len(),
        "no sidecar or cache in source"
    );
    assert_eq!(fs::read_dir(&photos).unwrap().count(), 0);
}
