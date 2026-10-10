//! 系统集成命令：默认程序打开文件、文件管理器打开目录、剪贴板与文件定位。
//!
//! - `open_with_system(path)`：Windows 目录使用 ShellExecuteW explore，
//!   macOS 目录使用 Finder；文件按系统默认关联打开。缺失路径返回明确错误。
//! - `clipboard_copy_files(paths)`：文件列表入系统剪贴板（CF_HDROP，
//!   DROPFILES + 双 NUL 结尾宽字符列表）。纯 Win32 clipboard API +
//!   STA CoInitializeEx（run_blocking 线程内，幂等）。
//! - `reveal_in_explorer(paths)`：按父目录分组 SHOpenFolderAndSelectItems
//!   （每组一窗选中），PIDL 解析或组 API 失败回退
//!   `explorer /select,"path"`。

#[cfg(test)]
use crate::platform::group_by_parent;
#[cfg(all(test, windows))]
use crate::platform::{child_pidls, ShellApartment};
use tauri::State;

use super::{run_blocking, SharedState};

/// 打开目录使用系统文件管理器；文件使用默认关联程序。
/// 运行在 `run_blocking` 后台线程（绝不碰 UI/main 线程）。
fn validate_open_resource(path: &str) -> Result<crate::platform::ResourceRef<'_>, String> {
    let resource = crate::platform::ResourceRef::parse(path);
    if let crate::platform::ResourceRef::LocalPath(path) = resource {
        let metadata=std::fs::metadata(path).map_err(|error| format!("路径不存在或不可访问: {path} ({error})"))?;
        if !metadata.is_file() && !metadata.is_dir() {
            return Err(format!("无法打开此路径: {path}"));
        }
    }
    Ok(resource)
}

pub fn fetch_open_with_system(path: &str) -> Result<(), String> {
    crate::platform::open_with_system(validate_open_resource(path)?)
}

/// 打开文件或目录（snake_case 命令，camelCase 负载 path）。
#[tauri::command]
pub async fn open_with_system(state: State<'_, SharedState>, path: String) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |_| fetch_open_with_system(&path)).await
}

// ---------------------------------------------------------------------------
// 右键菜单批量支点（2026-09-21）：剪贴板 CF_HDROP + Explorer 单窗多选
// ---------------------------------------------------------------------------

/// 文件路径校验（共用）：空列表 → 明确错误；不存在的文件全量列出
/// （剪贴板语义要求全有或全无，不部分复制）。
fn validate_file_args(paths: &[String]) -> Result<Vec<String>, String> {
    if paths.is_empty() {
        return Err("文件列表为空".into());
    }
    // 去重保序（前端多选可能带重复项）
    let mut seen = std::collections::HashSet::new();
    let unique: Vec<String> = paths
        .iter()
        .filter(|p| seen.insert(p.as_str()))
        .cloned()
        .collect();
    for path in &unique {
        crate::platform::ResourceRef::parse(path).local_file()?;
    }
    let missing: Vec<&str> = unique
        .iter()
        .filter(|p| !std::path::Path::new(p.as_str()).is_file())
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        return Err(format!("文件不存在: {}", missing.join(", ")));
    }
    Ok(unique)
}

/// 把文件列表复制到系统剪贴板（CF_HDROP）。返回入剪贴板的文件数。
/// 校验失败（空列表/任一不存在）→ 明确错误并**不动剪贴板**（全有或全无）。
/// 在 `run_blocking` 后台线程调用；剪贴板打开带短重试（他进程占用时）。
pub fn fetch_clipboard_copy_files(paths: &[String]) -> Result<u32, String> {
    let files = validate_file_args(paths)?;
    crate::platform::clipboard_copy_files(&files)?;
    Ok(files.len() as u32)
}

/// 在资源管理器中定位文件（多文件单窗选中）：按父目录分组，每组
/// SHOpenFolderAndSelectItems（父目录 PIDL + 子项 PIDL 列表）；
/// SHParseDisplayName 或组 API 失败回退
/// `explorer /select,"path"` 逐文件。返回定位成功的文件数。
pub fn fetch_reveal_in_explorer(paths: &[String]) -> Result<u32, String> {
    // reveal 不要求全有或全无：不存在的文件在 PIDL 解析层自然跳过，
    // 但**全部**不存在时是调用方错误 → 明确报错
    for path in paths {
        crate::platform::ResourceRef::parse(path).local_file()?;
    }
    let existing: Vec<String> = paths
        .iter()
        .filter(|p| std::path::Path::new(p.as_str()).is_file())
        .cloned()
        .collect();
    if existing.is_empty() {
        let missing = validate_file_args(paths).unwrap_err();
        return Err(missing);
    }
    crate::platform::reveal_files(&existing)
}

/// 文件列表复制到系统剪贴板（snake_case 命令）。
#[tauri::command]
pub async fn clipboard_copy_files(
    state: State<'_, SharedState>,
    paths: Vec<String>,
) -> Result<u32, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |_| fetch_clipboard_copy_files(&paths)).await
}

/// 在资源管理器中定位（snake_case 命令）。
#[tauri::command]
pub async fn reveal_in_explorer(
    state: State<'_, SharedState>,
    paths: Vec<String>,
) -> Result<u32, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |_| fetch_reveal_in_explorer(&paths)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    const CF_HDROP: u32 = 15;

    #[test]
    fn missing_path_is_clear_error() {
        let err = fetch_open_with_system(r"Z:\definitely\not\here.mp4").unwrap_err();
        assert!(err.contains("不存在"), "明确报不存在: {err}");
    }

    #[test]
    fn existing_directory_is_accepted_as_open_target() {
        let dir = tempfile::tempdir().unwrap();
        let path=dir.path().to_str().unwrap();
        assert_eq!(validate_open_resource(path).unwrap(), crate::platform::ResourceRef::LocalPath(path));
    }

    // -----------------------------------------------------------------
    // clipboard_copy_files：参数校验（跨平台层）
    // -----------------------------------------------------------------

    #[test]
    fn clipboard_empty_list_is_error() {
        let err = fetch_clipboard_copy_files(&[]).unwrap_err();
        assert!(err.contains("为空"), "空列表明确报错: {err}");
    }

    #[test]
    fn clipboard_missing_files_all_listed() {
        let err = fetch_clipboard_copy_files(&[r"Z:\no\a.jpg".into(), r"Z:\no\b.jpg".into()])
            .unwrap_err();
        assert!(
            err.contains("a.jpg") && err.contains("b.jpg"),
            "缺失文件全列出: {err}"
        );
        assert!(err.contains("不存在"), "明确报不存在: {err}");
    }

    #[test]
    fn clipboard_missing_files_do_not_touch_clipboard() {
        // 部分存在也全有或全无：校验层先行报错（存在的文件不进入 Win32 路径）
        let dir = tempfile::tempdir().unwrap();
        let ok = dir.path().join("ok.jpg");
        std::fs::write(&ok, b"x").unwrap();
        let err = fetch_clipboard_copy_files(&[
            ok.to_string_lossy().to_string(),
            r"Z:\no\missing.jpg".into(),
        ])
        .unwrap_err();
        assert!(err.contains("missing.jpg"), "{err}");
        assert!(!err.contains("ok.jpg"), "缺失清单不含存在文件: {err}");
    }

    #[test]
    fn validation_deduplicates_exact_paths_without_collapsing_case() {
        let dir = tempfile::tempdir().unwrap();
        let lower = dir.path().join("a.jpg");
        let upper = dir.path().join("A.jpg");
        std::fs::write(&lower, b"lower").unwrap();
        std::fs::write(&upper, b"upper").unwrap();
        let lower = lower.to_string_lossy().into_owned();
        let upper = upper.to_string_lossy().into_owned();
        let result = validate_file_args(&[lower.clone(), upper.clone(), lower.clone()]).unwrap();
        assert_eq!(result, vec![lower, upper]);
    }

    #[test]
    fn document_uri_is_rejected_before_local_path_validation() {
        let uri = "content://media/external/images/media/1".to_string();
        let error = validate_file_args(&[uri.clone()]).unwrap_err();
        assert!(error.contains("URI"), "{error}");
        assert!(fetch_reveal_in_explorer(&[uri])
            .unwrap_err()
            .contains("URI"));
    }

    // -----------------------------------------------------------------
    // reveal_in_explorer：参数校验 + 分组纯函数
    // -----------------------------------------------------------------

    #[test]
    fn reveal_all_missing_is_error_listing_them() {
        let err = fetch_reveal_in_explorer(&[r"Z:\gone\a.jpg".into()]).unwrap_err();
        assert!(err.contains("不存在") && err.contains("a.jpg"), "{err}");
        let err = fetch_reveal_in_explorer(&[]).unwrap_err();
        assert!(err.contains("为空"), "空列表明确报错: {err}");
    }

    #[test]
    fn group_by_parent_merges_case_insensitive_and_keeps_order() {
        let groups = group_by_parent(&[
            r"C:\Photos\a.jpg".into(),
            r"D:\Other\b.jpg".into(),
            r"c:\photos\B.jpg".into(), // 同目录（大小写归一）→ 并入第一组
            r"C:\Photos\sub\c.jpg".into(),
        ]);
        assert_eq!(
            groups.len(),
            3,
            "三组: C-Photos / D-Other / sub: {groups:?}"
        );
        assert_eq!(groups[0].0, r"c:\photos");
        assert_eq!(
            groups[0].1,
            vec![
                r"C:\Photos\a.jpg".to_string(),
                r"c:\photos\B.jpg".to_string()
            ],
            "组内成员保持输入顺序，大小写不敏感合并"
        );
        assert_eq!(groups[1].0, r"d:\other");
    }

    #[test]
    fn group_by_parent_single_group_multi_select() {
        let files: Vec<String> = (0..20)
            .map(|i| format!(r"C:\Photos\img{i:02}.jpg"))
            .collect();
        let groups = group_by_parent(&files);
        assert_eq!(groups.len(), 1, "同目录 20 张 → 单组（单窗多选）");
        assert_eq!(groups[0].1.len(), 20);
    }

    /// 使用真实 Shell PIDL 验证：父目录 + 子项必须还原为原文件，而非重复
    /// 拼接完整路径。不会打开资源管理器，也不占用剪贴板。
    #[cfg(windows)]
    #[test]
    fn explorer_selection_uses_relative_children_with_unicode_spaces_and_commas() {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::UI::Shell::{
            Common::ITEMIDLIST, ILCombine, ILFree, ILIsEqual, SHParseDisplayName,
        };

        let _apartment = ShellApartment::initialize().unwrap();
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("照片 目录,子目录");
        std::fs::create_dir_all(&folder).unwrap();
        let files = [folder.join("照片 1,原图.jpg"), folder.join("second.raw")];
        for file in &files {
            std::fs::write(file, b"x").unwrap();
        }
        unsafe fn parse(path: &std::path::Path) -> *mut ITEMIDLIST {
            let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let mut pidl = std::ptr::null_mut();
            SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None).unwrap();
            assert!(!pidl.is_null());
            pidl
        }
        unsafe {
            let parent = parse(&folder);
            let absolute: Vec<*const ITEMIDLIST> =
                files.iter().map(|file| parse(file).cast_const()).collect();
            let children = child_pidls(&absolute);
            for (full, child) in absolute.iter().zip(children) {
                assert_ne!(*full, child, "不能把完整路径作为相对子项传入");
                let combined = ILCombine(Some(parent), Some(child));
                assert!(!combined.is_null());
                let matches = ILIsEqual(combined, *full).as_bool();
                ILFree(Some(combined));
                assert!(matches, "父目录 + 相对子项应还原原文件");
            }
            for pidl in absolute {
                ILFree(Some(pidl));
            }
            ILFree(Some(parent));
        }
    }

    // -----------------------------------------------------------------
    // 真剪贴板往返（#[ignore]：占用真实剪贴板，读后清空恢复，手动跑）
    // -----------------------------------------------------------------

    #[cfg(windows)]
    #[test]
    #[ignore = "占用真实系统剪贴板（读后恢复为空），需 --ignored 手动运行"]
    fn clipboard_hdrop_roundtrip_real() {
        use windows::Win32::Foundation::HGLOBAL;
        use windows::Win32::System::DataExchange::{
            CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable,
            OpenClipboard,
        };
        use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
        use windows::Win32::UI::Shell::DROPFILES;

        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("甲 a.jpg"); // 含空格 + 非 ASCII（宽字符路径验证）
        let b = dir.path().join("b.jpg");
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"y").unwrap();
        let expect: Vec<String> = vec![
            a.to_string_lossy().to_string(),
            b.to_string_lossy().to_string(),
        ];
        let count = fetch_clipboard_copy_files(&expect).unwrap();
        assert_eq!(count, 2);

        unsafe {
            assert!(
                OpenClipboard(None).is_ok(),
                "测试须打开剪贴板（占用则稍后重跑）"
            );
            struct CloseGuard;
            impl Drop for CloseGuard {
                fn drop(&mut self) {
                    unsafe {
                        let _ = CloseClipboard();
                    };
                }
            }
            let _g = CloseGuard;
            assert!(
                IsClipboardFormatAvailable(CF_HDROP).is_ok(),
                "CF_HDROP 应就位"
            );
            let handle = GetClipboardData(CF_HDROP).expect("GetClipboardData");
            let ptr = GlobalLock(HGLOBAL(handle.0));
            assert!(!ptr.is_null());
            // packed 结构：read_unaligned 读头 + 字段先拷贝到对齐局部量
            let header = std::ptr::read_unaligned(ptr.cast::<DROPFILES>());
            let f_wide = header.fWide;
            let p_files = header.pFiles;
            assert_eq!(f_wide.0, 1, "宽字符列表标记");
            assert_eq!(
                p_files as usize,
                std::mem::size_of::<DROPFILES>(),
                "路径区偏移"
            );
            // 解析 double-null-terminated 宽字符列表
            let mut cursor = ptr.byte_add(p_files as usize).cast::<u16>();
            let mut list: Vec<String> = Vec::new();
            loop {
                let mut w = Vec::new();
                while *cursor != 0 {
                    w.push(*cursor);
                    cursor = cursor.add(1);
                }
                if w.is_empty() {
                    break; // 连续 NUL = 列表终结
                }
                list.push(String::from_utf16_lossy(&w));
                cursor = cursor.add(1);
            }
            let _ = GlobalUnlock(HGLOBAL(handle.0));
            assert_eq!(list.len(), 2, "两条路径: {list:?}");
            assert_eq!(list, expect, "HGLOBAL 内容与输入逐字一致（含中文/空格）");
            // 恢复：清空剪贴板（礼貌收尾）
            let _ = EmptyClipboard();
        }
    }

    /// 真 Explorer 定位冒烟（#[ignore]：会打开真实资源管理器窗口）。
    /// 两个目录各 2 文件（2 组=2 窗），全部定位成功；再验单目录 5 文件单窗。
    #[cfg(windows)]
    #[test]
    #[ignore = "会弹出真实资源管理器窗口（两组=两窗），手动 --ignored 运行"]
    fn reveal_explorer_real_smoke() {
        let root = tempfile::tempdir().unwrap();
        let dir_a = root.path().join("组A");
        let dir_b = root.path().join("dir-b");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let mut paths = Vec::new();
        for d in [&dir_a, &dir_b] {
            for n in 0..2 {
                let f = d.join(format!("f{n}.jpg"));
                std::fs::write(&f, b"x").unwrap();
                paths.push(f.to_string_lossy().to_string());
            }
        }
        let revealed = fetch_reveal_in_explorer(&paths).unwrap();
        assert_eq!(revealed, 4, "四个真实文件全部定位成功");

        // 单目录 5 文件 → 单窗多选
        let mut five = Vec::new();
        for n in 0..5 {
            let f = dir_a.join(format!("m{n}.jpg"));
            std::fs::write(&f, b"x").unwrap();
            five.push(f.to_string_lossy().to_string());
        }
        let revealed = fetch_reveal_in_explorer(&five).unwrap();
        assert_eq!(revealed, 5);
    }
}
