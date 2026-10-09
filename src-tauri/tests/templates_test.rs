//! 导入命名工具（模板引擎退役后仅存三件，2026-10-09 §三定案）：拍摄
//! 时间归一（EXIF 优先回退 mtime——纯时间布局年/月段的口径）、Windows
//! 文件名段清理、冲突递增唯一路径。旧目录/文件名模板（render_dir/
//! render_name/RenderCtx）随纯时间布局退役。
//!
//! lib.rs 中 `mod import;` 为私有模块（不可改），此处经 `#[path]` 将源文件
//! 直接编译进测试 crate。

mod common;

pub use common::{
    ai, bursts, db, devices, events, geo, import, index, ipc, metadata, platform, scan, settings,
    tasks, tethering, thumbs,
};

#[path = "../src/import/templates.rs"]
mod templates;

use templates::{resolve_captured, sanitize_component, unique_path};

/// EXIF 优先、mtime 回退（纯时间布局年/月段与 assets.captured_at 同口径）。
#[test]
fn resolve_captured_prefers_exif_then_mtime() {
    let exif = common::utc(2026, 6, 28, 15, 30, 0);
    let mtime = common::utc(2020, 1, 2, 3, 4, 5);
    assert_eq!(resolve_captured(Some(exif), mtime), exif, "EXIF 优先");
    assert_eq!(resolve_captured(None, mtime), mtime, "缺失回退 mtime");
}

/// Windows 非法字符与控制字符折叠为 `_`，合法字符（含中文/空格）保留。
#[test]
fn sanitize_component_folds_illegal_chars() {
    assert_eq!(sanitize_component("a:b*c?d\"e<f>g|h"), "a_b_c_d_e_f_g_h");
    assert_eq!(sanitize_component("春节 拍摄"), "春节 拍摄");
    assert_eq!(
        sanitize_component(&format!("ctrl{}x", char::from_u32(1).unwrap())),
        "ctrl_x",
        "控制字符折叠"
    );
    assert_eq!(sanitize_component(""), "");
}

/// 冲突递增：`a.txt` 被占 → `a_1.txt`、`a_2.txt`；无冲突直返；点开头
/// 主名为空的文件不误切扩展名。
#[test]
fn unique_path_appends_numeric_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.txt");
    std::fs::write(&a, b"1").unwrap();
    assert_eq!(
        unique_path(dir.path(), "a.txt"),
        dir.path().join("a_1.txt"),
        "首个冲突追加 _1"
    );
    std::fs::write(dir.path().join("a_1.txt"), b"2").unwrap();
    assert_eq!(
        unique_path(dir.path(), "a.txt"),
        dir.path().join("a_2.txt"),
        "连环冲突递增"
    );
    // 无冲突直返；点开头（主名为空 → 整体视作主名，不误切扩展名）
    assert_eq!(unique_path(dir.path(), "b.txt"), dir.path().join("b.txt"));
    std::fs::write(dir.path().join(".hidden"), b"h").unwrap();
    assert_eq!(
        unique_path(dir.path(), ".hidden"),
        dir.path().join(".hidden_1")
    );
}
