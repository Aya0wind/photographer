//! IPC 文件夹源与目录树浏览：folder_scan 注册（幂等 id）、device_files、
//! 经 import_start 的 move 管线，fs_list_dirs 懒加载（层级/隐藏属性/
//! 路径形态保留回归）。

mod common;

pub use common::{db, devices, events, import, ipc, metadata, migrate, settings, tasks, thumbs};

use std::fs;
use std::path::Path;
use std::time::Duration;

use common::{build_many, ipc_plan, state_with_library, wait_done};
use events::FileState;
use import::engine::ImportMode;
use ipc::{files_by_id, list_dirs, scan_folder, start_import};

/// 与 `\\?\` 前缀无关的 canonical 路径字符串（期望 id 用）。
fn plain_canonical(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string()
}

#[test]
fn folder_scan_registers_source_then_files_and_move_import_work() {
    let folder = tempfile::tempdir().unwrap();
    build_many(folder.path(), 3);
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let state = state_with_library(db_dir.path(), folder.path(), Duration::from_millis(1));
    // 只留 folder_scan 注册路径（清掉预注册的卷源）
    state.devices.lock().unwrap().clear();

    // 不存在路径 → Err
    assert!(scan_folder(&state, r"C:\definitely\not\here").is_err());

    // 扫描注册：id 为 FOLDER:<canonical>，快照统计正确
    let snapshot = scan_folder(&state, folder.path().to_str().unwrap()).unwrap();
    assert_eq!(
        snapshot.id,
        format!(
            "FOLDER:{}",
            plain_canonical(&fs::canonicalize(folder.path()).unwrap())
        )
    );
    assert_eq!(snapshot.kind, devices::SourceKind::Folder);
    assert_eq!(snapshot.new_files, 3, "空库应全部为新文件");
    assert!(snapshot.bytes_total > 0);

    // 路径变体（尾部 /.）→ 同一 id（幂等注册，不重复建条目）
    let variant = folder.path().join(".");
    let again = scan_folder(&state, variant.to_str().unwrap()).unwrap();
    assert_eq!(again.id, snapshot.id);
    assert_eq!(state.devices.lock().unwrap().len(), 1);

    // 注册后 device_files 可用（前端向导走同一命令）
    let files = files_by_id(&state, &snapshot.id).unwrap();
    assert_eq!(files.len(), 3);
    assert!(files.iter().all(|f| f.rel_path.starts_with("DCIM/")));

    // 走现有 import_start 管线（move 模式）→ 源删除、目标入库
    let mut p = ipc_plan(&state, target.path());
    p.source_id = snapshot.id.clone();
    p.mode = ImportMode::Move;
    let job_id = start_import(&state, p).unwrap();
    assert!(
        wait_done(&state, Duration::from_secs(15)),
        "move 导入应完成"
    );
    let db = ipc::active_library_db(&state).unwrap();
    let states: Vec<FileState> = db
        .all_job_files(job_id)
        .unwrap()
        .into_iter()
        .map(|r| r.state)
        .collect();
    assert!(states.iter().all(|s| *s == FileState::Verified));
    let assets: i64 =
        db.0.query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0))
            .unwrap();
    assert_eq!(assets, 3);
    // 源 jpg 全部删除（move 经 IPC 的 ArcSource 转发生效；空目录清理后
    // DCIM 可能整体移除，按全树扫描断言）
    let remaining = walkdir::WalkDir::new(folder.path())
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "jpg"))
        .count();
    assert_eq!(remaining, 0, "move 后源不应残留 jpg");
    assert!(folder.path().exists(), "源根保留");
}

#[test]
fn fs_list_dirs_lists_child_dirs_one_level() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("photos").join("2024")).unwrap();
    fs::create_dir_all(root.path().join("empty_dir")).unwrap();
    fs::create_dir_all(root.path().join(".dotted")).unwrap();
    fs::create_dir_all(root.path().join("$RECYCLE.BIN")).unwrap();
    fs::create_dir_all(root.path().join("System Volume Information")).unwrap();
    fs::write(root.path().join("afile.jpg"), b"x").unwrap(); // 文件不得出现
                                                             // 隐藏属性目录（0x2）应被过滤
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN};
        let hidden = root.path().join("hidden_attr");
        fs::create_dir_all(&hidden).unwrap();
        let wide: Vec<u16> = hidden
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        unsafe { SetFileAttributesW(PCWSTR(wide.as_ptr()), FILE_ATTRIBUTE_HIDDEN) }
            .expect("set hidden attr");
    }

    let entries = list_dirs(Some(root.path().to_str().unwrap()));
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        ["empty_dir", "photos"],
        "文件/点前缀/黑名单/隐藏属性不出现"
    );

    // hasSubdirs 浅探测 + 绝对路径 + camelCase 负载
    let photos = entries.iter().find(|e| e.name == "photos").unwrap();
    assert!(photos.has_subdirs, "photos 下有 2024");
    assert!(Path::new(&photos.path).is_absolute(), "路径必须绝对化");
    let empty = entries.iter().find(|e| e.name == "empty_dir").unwrap();
    assert!(!empty.has_subdirs);
    let json = serde_json::to_value(photos).unwrap();
    assert!(json.get("hasSubdirs").is_some(), "camelCase: {json}");
}

#[test]
fn fs_list_dirs_invalid_parent_is_empty_and_roots_listed() {
    // 非法/不存在 parent → 空数组不报错
    assert!(list_dirs(Some(r"C:\definitely\not\here")).is_empty());

    // None → 盘符根：真实系统上 C:\ 必在（只断言存在，不遍历内容）
    let drives = list_dirs(None);
    assert!(
        drives.iter().any(|d| d.path == "C:\\"),
        "应包含 C:\\ : {drives:?}"
    );
    assert!(drives.iter().all(|d| d.name.ends_with(':')));
    assert!(drives.iter().all(|d| d.path.ends_with('\\')));
}

#[test]
fn fs_list_dirs_preserves_parent_path_form() {
    // 回归（映射盘 bug，2026-09-18）：条目 path 必须与父路径同形态原样拼接，
    // 不得 canonicalize——否则映射盘 Y:\DCIM 被解析成 \\?\UNC\... 再剥前缀，
    // 产出残缺的 "UNC\192.168.31.103\..."。用 verbatim 父路径本地复现
    // “形态转换”这一类破坏：canonicalize 会把 \\?\C:\... 变回 C:\...，
    // 修复后必须保留 verbatim 形态。
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("photos").join("2024")).unwrap();

    let verbatim_parent = format!(r"\\?\{}", root.path().display());
    let entries = list_dirs(Some(&verbatim_parent));
    let photos = entries
        .iter()
        .find(|e| e.name == "photos")
        .expect("verbatim 父路径应可枚举");
    assert_eq!(
        photos.path,
        format!(r"{}\photos", verbatim_parent),
        "条目 path 必须原样拼接父路径（不转换形态）: {:?}",
        entries
    );
    // 子树探测也用同形态路径（否则 hasSubdirs 恒 false —— Y:\照片 展开为空的根因）
    assert!(photos.has_subdirs, "探测子目录不得因形态转换失败");
    // 绝不出现剥坏前缀的 UNC\ 残缺形态
    assert!(!photos.path.starts_with(r"UNC\"));

    // 普通形态父路径（带尾分隔符）同样原样系（幂等展开）
    let plain_with_slash = format!(r"{}\", root.path().display());
    let entries = list_dirs(Some(&plain_with_slash));
    let photos = entries.iter().find(|e| e.name == "photos").unwrap();
    assert_eq!(photos.path, format!(r"{}\photos", root.path().display()));
    assert!(photos.has_subdirs);
}
