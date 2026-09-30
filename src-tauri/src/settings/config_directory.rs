//! The dev rename is a Windows application-data migration, not a request for
//! macOS (or smoke-test identifiers) to take ownership of another installation.
use std::path::Path;

pub(crate) fn prepare(config_dir: &Path) -> std::io::Result<()> {
    prepare_with_legacy_migration(config_dir, cfg!(windows))
}

fn prepare_with_legacy_migration(config_dir: &Path, migrate_windows: bool) -> std::io::Result<()> {
    if migrate_windows
        && config_dir.file_name() == Some(std::ffi::OsStr::new("photohub"))
        && !config_dir.exists()
    {
        if let Some(legacy) = config_dir
            .parent()
            .map(|parent| parent.join("com.smartphoto.app"))
            .filter(|path| path.is_dir())
        {
            if let Err(error) = std::fs::rename(&legacy, config_dir) {
                eprintln!(
                    "旧配置目录迁移失败（{} → {}）: {error}",
                    legacy.display(),
                    config_dir.display()
                );
            }
        }
    }
    std::fs::create_dir_all(config_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy(root: &Path) -> std::path::PathBuf {
        let legacy = root.join("com.smartphoto.app");
        std::fs::create_dir(&legacy).unwrap();
        std::fs::write(legacy.join("settings.json"), b"legacy-settings").unwrap();
        legacy
    }

    #[test]
    fn windows_production_rename_preserves_settings() {
        let root = tempfile::tempdir().unwrap();
        let old = legacy(root.path());
        let new = root.path().join("photohub");
        prepare_with_legacy_migration(&new, true).unwrap();
        assert!(!old.exists());
        assert_eq!(std::fs::read(new.join("settings.json")).unwrap(), b"legacy-settings");
    }

    #[test]
    fn existing_destination_does_not_replace_either_installation() {
        let root = tempfile::tempdir().unwrap();
        let old = legacy(root.path());
        let new = root.path().join("photohub");
        std::fs::create_dir(&new).unwrap();
        std::fs::write(new.join("settings.json"), b"current-settings").unwrap();
        prepare_with_legacy_migration(&new, true).unwrap();
        assert_eq!(std::fs::read(old.join("settings.json")).unwrap(), b"legacy-settings");
        assert_eq!(std::fs::read(new.join("settings.json")).unwrap(), b"current-settings");
    }

    #[test]
    fn macos_and_smoke_identifiers_never_consume_legacy_settings() {
        let root = tempfile::tempdir().unwrap();
        let old = legacy(root.path());
        for (name, migrate) in [
            ("com.smartphoto.photohub", false),
            ("com.smartphoto.photohub.macos-smoke", false),
            ("photohub", false),
            ("photohub.test", true),
        ] {
            let new = root.path().join(name);
            prepare_with_legacy_migration(&new, migrate).unwrap();
            assert!(new.is_dir());
            assert!(!new.join("settings.json").exists());
            assert_eq!(std::fs::read(old.join("settings.json")).unwrap(), b"legacy-settings");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_macos_entry_point_is_isolated() {
        let root = tempfile::tempdir().unwrap();
        let old = legacy(root.path());
        let new = root.path().join("com.smartphoto.photohub");
        prepare(&new).unwrap();
        assert!(old.join("settings.json").is_file());
        assert!(!new.join("settings.json").exists());
    }
}
