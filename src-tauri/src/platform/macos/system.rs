use std::process::Command;

// Paths are argv data, never AppleScript source. Quotes, backslashes and
// newlines in valid macOS filenames must not change the program.
const COPY_FILES_SCRIPT: &str = r#"on run argv
    set fileItems to {}
    repeat with filePath in argv
        set end of fileItems to (POSIX file (contents of filePath) as alias)
    end repeat
    set the clipboard to fileItems
end run"#;

pub(crate) fn open_with_system(resource: super::super::ResourceRef<'_>) -> Result<(), String> {
    let path = resource.local_file()?;
    let value = std::path::absolute(path).map_err(|error| error.to_string())?;
    Command::new("/usr/bin/open")
        .arg(value.as_os_str())
        .status()
        .map_err(|error| format!("调用 macOS open 失败: {error}"))?
        .success()
        .then_some(())
        .ok_or_else(|| format!("macOS 无法打开: {}", value.display()))
}

pub(crate) fn reveal_files(files: &[String]) -> Result<u32, String> {
    let mut count = 0;
    for path in files {
        let path = std::path::absolute(path).map_err(|error| error.to_string())?;
        let status = Command::new("/usr/bin/open")
            .arg("-R")
            .arg(path)
            .status()
            .map_err(|error| format!("调用 Finder 失败: {error}"))?;
        if status.success() {
            count += 1;
        }
    }
    (count > 0)
        .then_some(count)
        .ok_or_else(|| "无法在 Finder 中定位文件".into())
}

pub(crate) fn clipboard_copy_files(files: &[String]) -> Result<(), String> {
    if files.is_empty() {
        return Err("文件列表为空".into());
    }
    let paths: Vec<_> = files
        .iter()
        .map(std::path::absolute)
        .collect::<std::io::Result<_>>()
        .map_err(|error| error.to_string())?;
    let status = Command::new("/usr/bin/osascript")
        .args(["-e", COPY_FILES_SCRIPT])
        .args(paths)
        .status()
        .map_err(|error| format!("调用 macOS 剪贴板失败: {error}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| "写入 macOS 文件剪贴板失败".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_rejected_before_launching_default_handler() {
        assert!(open_with_system(super::super::super::ResourceRef::Uri(
            "https://example.invalid"
        ))
        .unwrap_err()
        .contains("URI"));
    }

    #[test]
    fn osascript_keeps_special_filename_characters_as_data() {
        // Read-only script: exercises argv/alias conversion without touching
        // the user's clipboard or opening Finder.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("照片 \"quoted\"\\line\nbreak.jpg");
        std::fs::write(&path, b"fixture").unwrap();
        let output = Command::new("/usr/bin/osascript")
            .args([
                "-e",
                "on run argv\nreturn POSIX path of (POSIX file (item 1 of argv) as alias)\nend run",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let returned = String::from_utf8(output.stdout).unwrap();
        let returned = returned.trim_end_matches('\n');
        assert_eq!(
            std::fs::canonicalize(returned).unwrap(),
            std::fs::canonicalize(path).unwrap(),
        );
    }
}
