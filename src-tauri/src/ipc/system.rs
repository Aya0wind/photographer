//! 系统集成命令（M8 视频：HEVC 缺解码器时的系统播放回退支点；
//! 2026-09-21 右键菜单后端：批量复制到剪贴板 + 资源管理器定位）。
//!
//! - `open_with_system(path)`：ShellExecuteW "open"——交给系统默认程序
//!   （视频 = 默认播放器，照片 = 查看器）。不用 shell 插件（多一份权限面
//!   与依赖），直调 windows crate。路径不存在返回明确错误；ShellExecute
//!   失败（无关联程序等）映射为可读错误。
//! - `clipboard_copy_files(paths)`：文件列表入系统剪贴板（CF_HDROP，
//!   DROPFILES + 双 NUL 结尾宽字符列表）。纯 Win32 clipboard API +
//!   STA CoInitializeEx（run_blocking 线程内，幂等）。
//! - `reveal_in_explorer(paths)`：按父目录分组 SHOpenFolderAndSelectItems
//!   （每组一窗选中），PIDL 解析失败的文件跳过，组 API 失败回退
//!   `explorer /select,"path"`。

use tauri::State;

use super::{run_blocking, SharedState};

/// 用系统默认程序打开文件（open 语义：视频走默认播放器）。
/// 运行在 `run_blocking` 后台线程（绝不碰 UI/main 线程）。
pub fn fetch_open_with_system(path: &str) -> Result<(), String> {
    let target = std::path::Path::new(path);
    if !target.is_file() {
        return Err(format!("文件不存在: {path}"));
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        let to_wide = |s: &str| -> Vec<u16> {
            std::ffi::OsStr::new(s)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect()
        };
        let verb = to_wide("open");
        let file = to_wide(path);
        // 返回值 > 32（传统 HINSTANCE 判定）= 成功；≤32 为错误码
        let code = unsafe {
            ShellExecuteW(
                None,
                PCWSTR(verb.as_ptr()),
                PCWSTR(file.as_ptr()),
                None,
                None,
                SW_SHOWNORMAL,
            )
        };
        let code = code.0 as usize;
        if code > 32 {
            return Ok(());
        }
        let reason = match code {
            0 => "系统内存/资源不足",
            2 => "未找到关联程序（该扩展名没有默认打开方式）",
            3 => "路径无效",
            5 => "访问被拒绝（权限/占用）",
            8 => "打开程序数超限",
            11 | 12 => "可执行文件无效",
            15..=30 => "驱动器/关联程序异常",
            31 => "该文件类型没有关联的打开程序",
            _ => "系统打开失败",
        };
        Err(format!("系统打开失败（码 {code}）：{reason}"))
    }

    #[cfg(not(windows))]
    {
        // 非 Windows（CI 编译面）：open 语义交给 xdg-open 等价物
        let status = std::process::Command::new("xdg-open")
            .arg(target)
            .status()
            .map_err(|e| format!("打开失败: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("xdg-open 退出码 {:?}", status.code()))
        }
    }
}

/// 用系统默认程序打开文件（snake_case 命令，camelCase 负载 path）。
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
        .filter(|p| seen.insert(p.to_ascii_lowercase()))
        .cloned()
        .collect();
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
    #[cfg(windows)]
    {
        clipboard_copy_hdrop(&files)?;
        Ok(files.len() as u32)
    }
    #[cfg(not(windows))]
    {
        let _ = files;
        Err("复制到剪贴板仅支持 Windows".into())
    }
}

/// CF_HDROP 常量（windows crate 未导出；WinUser.h 固定值 15）。
#[cfg(windows)]
const CF_HDROP: u32 = 15;

/// Win32 剪贴板 CF_HDROP 写入（OpenClipboard + EmptyClipboard +
/// SetClipboardData(HGLOBAL DROPFILES)；CloseClipboard 必走——guard 兜底）。
#[cfg(windows)]
fn clipboard_copy_hdrop(files: &[String]) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{GlobalFree, HANDLE};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GHND};
    use windows::Win32::UI::Shell::DROPFILES;

    // STA COM 初始化（幂等：S_FALSE=本线程已 STA 也算成功，配对 CoUninitialize；
    // 失败不阻断——OpenClipboard/SetClipboardData 是纯 Win32 路径，仅 OLE
    // 剪贴板变体才强制 STA，此处按调用约定保守初始化）
    let co_hr = unsafe {
        windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        )
    };
    let co_ok = co_hr.is_ok();

    // 结构布局：DROPFILES 头（宽字符标记 fWide=TRUE）+ 每路径一个宽 NUL
    // 结尾串 + 列表整体再补一个 L'\0'（double-null-terminated）
    let to_wide = |p: &str| -> Vec<u16> {
        std::ffi::OsStr::new(p)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    let wide_list: Vec<Vec<u16>> = files.iter().map(|p| to_wide(p)).collect();
    let bytes =
        std::mem::size_of::<DROPFILES>() + wide_list.iter().map(|w| w.len() * 2).sum::<usize>() + 2; // 末尾列表终结 L'\0'

    // 打开剪贴板（他进程占用时短重试；None = 不关联窗口）
    let mut opened = false;
    for _ in 0..5 {
        if unsafe { OpenClipboard(None) }.is_ok() {
            opened = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    if !opened {
        return Err("剪贴板被其他程序占用，请重试".into());
    }
    // 无论后续成败必须 CloseClipboard（作用域守卫）
    struct CloseGuard;
    impl Drop for CloseGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseClipboard();
            };
        }
    }
    let _guard = CloseGuard;

    unsafe {
        EmptyClipboard().map_err(|e| format!("清空剪贴板失败: {e}"))?;
        let handle = GlobalAlloc(GHND, bytes).map_err(|e| format!("分配剪贴板内存失败: {e}"))?;
        let ptr = GlobalLock(handle);
        if ptr.is_null() {
            let _ = GlobalFree(Some(handle));
            return Err("锁定剪贴板内存失败".into());
        }
        // 写 DROPFILES 头（pFiles=偏移 20、fWide=TRUE）+ 宽字符路径列表。
        // DROPFILES 是 packed 结构（1 字节对齐）——经 write_unaligned 写入，
        // 不可对 packed 字段取引用
        let header = DROPFILES {
            pFiles: std::mem::size_of::<DROPFILES>() as u32,
            pt: windows::Win32::Foundation::POINT { x: 0, y: 0 },
            fNC: BOOL(0),
            fWide: BOOL(1),
        };
        std::ptr::write_unaligned(ptr.cast::<DROPFILES>(), header);
        // 游标显式 *mut u16：copy_nonoverlapping 按**元素**计数——裸
        // *mut c_void 会退化成按字节拷贝（实测曾把宽字符列表截半）
        let mut cursor = ptr.byte_add(std::mem::size_of::<DROPFILES>()).cast::<u16>();
        for wide in &wide_list {
            std::ptr::copy_nonoverlapping(wide.as_ptr(), cursor, wide.len());
            cursor = cursor.add(wide.len());
        }
        *cursor = 0; // 列表终结 L' '（double-null-terminated）
        let _ = GlobalUnlock(handle);

        // 成功后系统接管 HGLOBAL（不可 GlobalFree）；失败必须释放
        if let Err(e) = SetClipboardData(CF_HDROP, Some(HANDLE(handle.0))) {
            let _ = GlobalFree(Some(handle));
            return Err(format!("写入剪贴板失败: {e}"));
        }
    }
    if co_ok {
        unsafe { windows::Win32::System::Com::CoUninitialize() };
    }
    Ok(())
}

/// 按父目录分组（大小写不敏感归一：Windows 路径语义；保序）。
/// 目录不可解析（无父目录的根形态）按原样成组。纯函数供单测。
fn group_by_parent(paths: &[String]) -> Vec<(String, Vec<String>)> {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for path in paths {
        let p = std::path::Path::new(path);
        let parent = p
            .parent()
            .map(|d| d.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let slot = *index.entry(parent.clone()).or_insert_with(|| {
            groups.push((parent, Vec::new()));
            groups.len() - 1
        });
        groups[slot].1.push(path.clone());
    }
    groups
}

/// 在资源管理器中定位文件（多文件单窗选中）：按父目录分组，每组
/// SHOpenFolderAndSelectItems（父目录 PIDL + 子项 PIDL 列表）；
/// SHParseDisplayName 失败的文件跳过；组 API 失败回退
/// `explorer /select,"path"` 逐文件。返回定位成功的文件数。
pub fn fetch_reveal_in_explorer(paths: &[String]) -> Result<u32, String> {
    // reveal 不要求全有或全无：不存在的文件在 PIDL 解析层自然跳过，
    // 但**全部**不存在时是调用方错误 → 明确报错
    let existing: Vec<String> = paths
        .iter()
        .filter(|p| std::path::Path::new(p.as_str()).is_file())
        .cloned()
        .collect();
    if existing.is_empty() {
        let missing = validate_file_args(paths).unwrap_err();
        return Err(missing);
    }
    #[cfg(windows)]
    {
        reveal_grouped(&existing)
    }
    #[cfg(not(windows))]
    {
        let _ = existing;
        Err("资源管理器定位仅支持 Windows".into())
    }
}

/// Win32 分组定位实现：每组一次 SHOpenFolderAndSelectItems（单窗多选），
/// 失败回退 explorer /select。
#[cfg(windows)]
fn reveal_grouped(files: &[String]) -> Result<u32, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{ILFree, SHOpenFolderAndSelectItems, SHParseDisplayName};

    let to_wide = |s: &str| -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    /// 解析单个绝对路径为 PIDL（失败 None——SFGAO_FOLDER 仅目录层可用，
    /// 文件层用 0 请求纯解析）。
    unsafe fn parse_pidl(
        wide: &[u16],
    ) -> Option<*mut windows::Win32::UI::Shell::Common::ITEMIDLIST> {
        let mut pidl: *mut windows::Win32::UI::Shell::Common::ITEMIDLIST = std::ptr::null_mut();
        SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None).ok()?;
        if pidl.is_null() {
            return None;
        }
        Some(pidl)
    }

    let mut revealed: u32 = 0;
    for (parent, group) in group_by_parent(files) {
        let parent_wide = to_wide(&parent);
        let mut pidls: Vec<*const windows::Win32::UI::Shell::Common::ITEMIDLIST> = Vec::new();
        let mut parsed_files: Vec<&String> = Vec::new();
        unsafe {
            // 父目录 PIDL（组键）：解析失败整组回退 explorer /select
            if let Some(folder_pidl) = parse_pidl(&parent_wide) {
                for file in &group {
                    let file_wide = to_wide(file);
                    if let Some(pidl) = parse_pidl(&file_wide) {
                        pidls.push(pidl);
                        parsed_files.push(file);
                    }
                    // 解析失败的文件跳过（不回退：右键定位的容忍语义）
                }
                if pidls.is_empty() {
                    ILFree(Some(folder_pidl));
                } else {
                    let result = SHOpenFolderAndSelectItems(folder_pidl, Some(&pidls), 0);
                    ILFree(Some(folder_pidl));
                    for p in &pidls {
                        ILFree(Some(*p));
                    }
                    match result {
                        Ok(()) => revealed += parsed_files.len() as u32,
                        Err(_) => {
                            // 组 API 失败（Explorer 忙等）：逐文件 explorer /select
                            for file in &group {
                                if explorer_select_fallback(file) {
                                    revealed += 1;
                                }
                            }
                        }
                    }
                }
            } else {
                for file in &group {
                    if explorer_select_fallback(file) {
                        revealed += 1;
                    }
                }
            }
        }
    }
    Ok(revealed)
}

/// `explorer /select,"path"` 回退（尽力语义：explorer.exe 退出码不可靠，
/// spawn 成功即计入）。窗口不前台聚焦风险由 Explorer 自身决定。
#[cfg(windows)]
fn explorer_select_fallback(path: &str) -> bool {
    std::process::Command::new("explorer")
        .arg(format!("/select,{path}"))
        .spawn()
        .is_ok()
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

    #[test]
    fn missing_path_is_clear_error() {
        let err = fetch_open_with_system(r"Z:\definitely\not\here.mp4").unwrap_err();
        assert!(err.contains("不存在"), "明确报不存在: {err}");
    }

    #[test]
    fn directory_is_rejected_as_not_file() {
        let dir = tempfile::tempdir().unwrap();
        let err = fetch_open_with_system(dir.path().to_str().unwrap()).unwrap_err();
        assert!(err.contains("不存在"), "目录不当作可打开文件: {err}");
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
