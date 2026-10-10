//! Windows 应用数据目录随标识符改名做「改名接管」迁移（非 macOS）：
//! photographer ← photohub ← com.smartphoto.app，只试最近一代，一步到位。
use std::path::Path;

pub(crate) fn prepare(config_dir: &Path) -> std::io::Result<()> {
    prepare_with_legacy_migration(config_dir, cfg!(windows))
}

fn prepare_with_legacy_migration(config_dir: &Path, migrate_windows: bool) -> std::io::Result<()> {
    if migrate_windows
        && config_dir.file_name() == Some(std::ffi::OsStr::new("photographer"))
        && !config_dir.exists()
    {
        // 旧标识符目录按代取最近一个直接改名（2026-10-10 photohub →
        // photographer；更早 com.smartphoto.app 用户跨代升级同路）。
        // rename 失败（被占用等）不吞更旧的——打印后照常 create_dir。
        if let Some(legacy) = ["photohub", "com.smartphoto.app"]
            .iter()
            .find_map(|name| {
                config_dir
                    .parent()
                    .map(|parent| parent.join(name))
                    .filter(|path| path.is_dir())
            })
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

    fn make_config(root: &Path, name: &str, payload: &[u8]) -> std::path::PathBuf {
        let dir = root.join(name);
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), payload).unwrap();
        dir
    }

    #[test]
    fn windows_upgrade_from_photohub_preserves_settings() {
        let root = tempfile::tempdir().unwrap();
        let old = make_config(root.path(), "photohub", b"photohub-settings");
        let new = root.path().join("photographer");
        prepare_with_legacy_migration(&new, true).unwrap();
        assert!(!old.exists());
        assert_eq!(
            std::fs::read(new.join("settings.json")).unwrap(),
            b"photohub-settings"
        );
    }

    #[test]
    fn windows_ancient_com_smartphoto_upgrades_directly() {
        let root = tempfile::tempdir().unwrap();
        let old = make_config(root.path(), "com.smartphoto.app", b"ancient-settings");
        let new = root.path().join("photographer");
        prepare_with_legacy_migration(&new, true).unwrap();
        assert!(!old.exists());
        assert_eq!(
            std::fs::read(new.join("settings.json")).unwrap(),
            b"ancient-settings"
        );
    }

    #[test]
    fn existing_destination_does_not_replace_either_installation() {
        let root = tempfile::tempdir().unwrap();
        let old = make_config(root.path(), "photohub", b"photohub-settings");
        let new = make_config(root.path(), "photographer", b"current-settings");
        prepare_with_legacy_migration(&new, true).unwrap();
        assert_eq!(
            std::fs::read(old.join("settings.json")).unwrap(),
            b"photohub-settings"
        );
        assert_eq!(
            std::fs::read(new.join("settings.json")).unwrap(),
            b"current-settings"
        );
    }

    #[test]
    fn macos_and_smoke_identifiers_never_consume_legacy_settings() {
        let root = tempfile::tempdir().unwrap();
        let old = make_config(root.path(), "com.smartphoto.app", b"legacy-settings");
        for (name, migrate) in [
            ("com.photographer.app", false),
            ("com.photographer.app.macos-smoke", false),
            ("photohub", false),
            ("photographer.test", true),
        ] {
            let new = root.path().join(name);
            prepare_with_legacy_migration(&new, migrate).unwrap();
            assert!(new.is_dir());
            assert!(!new.join("settings.json").exists());
            assert_eq!(
                std::fs::read(old.join("settings.json")).unwrap(),
                b"legacy-settings"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_macos_entry_point_is_isolated() {
        let root = tempfile::tempdir().unwrap();
        let old = make_config(root.path(), "com.smartphoto.app", b"legacy-settings");
        let new = root.path().join("com.photographer.app");
        prepare(&new).unwrap();
        assert!(old.join("settings.json").is_file());
        assert!(!new.join("settings.json").exists());
    }
}
