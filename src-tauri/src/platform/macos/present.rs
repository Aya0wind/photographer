//! Mounted removable-volume discovery for macOS.
//!
//! The shared device manager already reconciles discovery periodically, so M5
//! starts with safe polling instead of a Disk Arbitration callback thread.
//! Only removable, non-disk-image mounts become import devices; the startup
//! disk and ordinary mounted application images stay out of the import list.

use std::path::Path;
use std::process::Command;

/// 只认原生 SD 总线；USB 读卡器与普通外置盘无法确定时保留手动引用入口。
pub fn is_storage_card(path: &Path) -> bool {
    let Some(mount) = path
        .ancestors()
        .find(|entry| entry.parent() == Some(Path::new("/Volumes")))
    else {
        return false;
    };
    let Ok(output) = Command::new("diskutil")
        .args(["info", "-plist"])
        .arg(mount)
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let plist = String::from_utf8_lossy(&output.stdout);
    plist_string(&plist, "BusProtocol").is_some_and(|protocol| {
        protocol.eq_ignore_ascii_case("Secure Digital") || protocol.eq_ignore_ascii_case("SD")
    })
}

pub fn enumerate_empty_readers() -> crate::devices::DeviceResult<Vec<String>> {
    Ok(Vec::new())
}

pub fn probe_volume(mount: &str) -> Option<String> {
    let path = Path::new(mount);
    is_importable_mount(path).then(|| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| mount.to_owned())
    })
}

pub fn enumerate_present_volumes() -> crate::devices::DeviceResult<Vec<(String, String)>> {
    let mounts = std::fs::read_dir("/Volumes").map_err(crate::devices::DeviceError::Io)?;
    let mut volumes = mounts
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let path = entry.path();
            let id = path.to_string_lossy().into_owned();
            probe_volume(&id).map(|name| (id, name))
        })
        .collect::<Vec<_>>();
    volumes.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(volumes)
}

fn is_importable_mount(path: &Path) -> bool {
    let output = Command::new("diskutil")
        .args(["info", "-plist"])
        .arg(path)
        .output();
    let Ok(output) = output else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let plist = String::from_utf8_lossy(&output.stdout);
    is_importable_plist(&plist)
}

fn is_importable_plist(plist: &str) -> bool {
    // External SSD/CFexpress readers can report RemovableMedia=false while
    // remaining ejectable. Internal is the hard exclusion; Disk Image is
    // filtered separately because macOS reports it as ejectable/removable.
    plist_bool(plist, "Internal") != Some(true)
        && plist_bool(plist, "Ejectable") == Some(true)
        && !plist_string(plist, "BusProtocol")
            .is_some_and(|protocol| protocol.eq_ignore_ascii_case("Disk Image"))
}

fn plist_bool(plist: &str, key: &str) -> Option<bool> {
    let (_, after_key) = plist.split_once(&format!("<key>{key}</key>"))?;
    let value = after_key.trim_start();
    if value.starts_with("<true/>") {
        Some(true)
    } else if value.starts_with("<false/>") {
        Some(false)
    } else {
        None
    }
}

fn plist_string<'a>(plist: &'a str, key: &str) -> Option<&'a str> {
    let (_, after_key) = plist.split_once(&format!("<key>{key}</key>"))?;
    let value = after_key.trim_start();
    let start = value.strip_prefix("<string>")?;
    let end = start.find("</string>")?;
    Some(&start[..end])
}

#[cfg(test)]
mod tests {
    use super::{is_importable_plist, plist_bool};

    #[test]
    fn parses_diskutil_boolean_values() {
        let plist = "<key>RemovableMedia</key><true/><key>DiskImage</key><false/>";
        assert_eq!(plist_bool(plist, "RemovableMedia"), Some(true));
        assert_eq!(plist_bool(plist, "DiskImage"), Some(false));
        assert_eq!(plist_bool(plist, "Missing"), None);
    }

    #[test]
    fn filters_disk_images_even_when_macos_marks_them_removable() {
        let image = concat!(
            "<key>RemovableMedia</key><true/><key>Internal</key><false/>",
            "<key>Ejectable</key><true/><key>BusProtocol</key><string>Disk Image</string>"
        );
        let card = concat!(
            "<key>RemovableMedia</key><true/><key>Internal</key><false/>",
            "<key>Ejectable</key><true/><key>BusProtocol</key><string>USB</string>"
        );
        let internal = concat!(
            "<key>RemovableMedia</key><false/><key>Internal</key><true/>",
            "<key>Ejectable</key><false/><key>BusProtocol</key><string>PCI</string>"
        );
        assert!(!is_importable_plist(image));
        assert!(is_importable_plist(card));
        assert!(!is_importable_plist(internal));
    }

    #[test]
    fn accepts_ejectable_external_media_and_rejects_non_ejectable_fixed_media() {
        let card = concat!(
            "<key>RemovableMedia</key><true/>",
            "<key>Internal</key><false/>",
            "<key>Ejectable</key><true/>",
            "<key>BusProtocol</key><string>USB</string>"
        );
        let fixed = concat!(
            "<key>RemovableMedia</key><true/>",
            "<key>Internal</key><false/>",
            "<key>Ejectable</key><false/>",
            "<key>BusProtocol</key><string>USB</string>"
        );
        assert!(is_importable_plist(card));
        assert!(!is_importable_plist(fixed));
    }

    #[test]
    fn accepts_external_fixed_media_when_removable_media_flag_is_false() {
        let external_ssd = concat!(
            "<key>RemovableMedia</key><false/>",
            "<key>Internal</key><false/>",
            "<key>Ejectable</key><true/>",
            "<key>BusProtocol</key><string>USB</string>"
        );
        assert!(is_importable_plist(external_ssd));
    }
}
