use std::{fs, path::Path};

pub fn validate(root: &Path, target_os: &str, expected_lock: &[u8]) -> Result<(), String> {
    let library = match target_os {
        "windows" => "photographer_raw.dll",
        "macos" => "libphotographer_raw.dylib",
        _ => "libphotographer_raw.so",
    };
    for file in [library, "upstream.lock.json", "films/catalog.json"] {
        let path = root.join(file);
        if !fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() > 0) {
            return Err(format!(
                "RawLab resource missing or empty: {}",
                path.display()
            ));
        }
    }
    if fs::read(root.join("upstream.lock.json")).map_err(|e| e.to_string())? != expected_lock {
        return Err("RawLab resources do not match the pinned upstream revision".into());
    }
    for (directory, extension) in [("films", "cube"), ("lensfun/version_1", "xml")] {
        let path = root.join(directory);
        let present = fs::read_dir(&path).is_ok_and(|entries| {
            entries.filter_map(Result::ok).any(|entry| {
                entry.path().extension().is_some_and(|ext| ext == extension)
                    && entry.metadata().is_ok_and(|m| m.is_file() && m.len() > 0)
            })
        });
        if !present {
            return Err(format!(
                "RawLab {extension} resources missing: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "photographer-rawlab-resources-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn populate(&self, target: &str) {
            let library = match target {
                "windows" => "photographer_raw.dll",
                "macos" => "libphotographer_raw.dylib",
                _ => "libphotographer_raw.so",
            };
            for file in [
                library,
                "upstream.lock.json",
                "films/catalog.json",
                "films/test.cube",
                "lensfun/version_1/test.xml",
            ] {
                let path = self.0.join(file);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, b"fixture").unwrap();
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn placeholder_resources_cannot_pass_release_validation() {
        let fixture = Fixture::new();
        fs::write(fixture.0.join(".gitkeep"), b"").unwrap();
        assert!(validate(&fixture.0, "windows", b"fixture")
            .unwrap_err()
            .contains("photographer_raw.dll"));
    }

    #[test]
    fn staged_resources_require_matching_revision_and_supporting_data() {
        let fixture = Fixture::new();
        fixture.populate("windows");
        assert!(validate(&fixture.0, "windows", b"fixture").is_ok());
        assert!(validate(&fixture.0, "windows", b"other revision").is_err());
        for file in [
            "photographer_raw.dll",
            "films/catalog.json",
            "films/test.cube",
            "lensfun/version_1/test.xml",
        ] {
            let path = fixture.0.join(file);
            fs::write(&path, b"").unwrap();
            assert!(
                validate(&fixture.0, "windows", b"fixture").is_err(),
                "{file}"
            );
            fs::write(path, b"fixture").unwrap();
        }
    }

    #[test]
    fn library_names_follow_the_target_platform() {
        for target in ["windows", "macos", "linux"] {
            let fixture = Fixture::new();
            fixture.populate(target);
            assert!(validate(&fixture.0, target, b"fixture").is_ok());
        }
    }
}
