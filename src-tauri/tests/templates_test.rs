//! 命名模板引擎测试（spec §5.2 / M1 T5）。
//! 令牌白名单与前端 src/features/onboarding/onboardingConfig.ts 的 TEMPLATE_TOKENS
//! 必须一致（含 MM-DD）。
//!
//! lib.rs 中 `mod import;` 为私有模块（不可改），此处经 `#[path]` 将源文件
//! 直接编译进测试 crate。

use std::fs;

use chrono::{TimeZone, Utc};

#[path = "../src/import/templates.rs"]
mod templates;

use templates::{
    render_dir, render_name, resolve_captured, sanitize_component, unique_path, RenderCtx,
    TemplateError,
};

fn ctx() -> RenderCtx {
    RenderCtx {
        captured_at: Utc.with_ymd_and_hms(2026, 9, 18, 14, 30, 5).unwrap(),
        camera: Some("EOS R5".to_string()),
        lens: Some("RF 24-70".to_string()),
        original_stem: "IMG_0001".to_string(),
        ext: "CR3".to_string(),
    }
}

#[test]
fn expands_all_tokens() {
    let template = "{YYYY}/{YY}/{MM}/{MM-DD}/{DD}/{HH}/{mm}/{ss}/{相机}/{镜头}/{原文件名}";
    assert_eq!(
        render_dir(template, &ctx()).unwrap(),
        "2026/26/09/09-18/18/14/30/05/EOS R5/RF 24-70/IMG_0001.CR3",
    );
}

#[test]
fn renders_date_with_literal_text() {
    assert_eq!(
        render_dir("照片-{YYYY}{MM}-{MM-DD}", &ctx()).unwrap(),
        "照片-202609-09-18",
    );
}

#[test]
fn render_name_keeps_ext_case() {
    let mut c = ctx();
    c.ext = "JpG".to_string();
    assert_eq!(render_name("{原文件名}", &c).unwrap(), "IMG_0001.JpG");
}

#[test]
fn empty_template_renders_empty() {
    assert_eq!(render_dir("", &ctx()).unwrap(), "");
    assert_eq!(render_name("", &ctx()).unwrap(), "");
}

#[test]
fn unknown_token_is_error() {
    let err = render_dir("{YYYY}/{FFF}", &ctx()).unwrap_err();
    assert!(matches!(err, TemplateError::UnknownToken(ref t) if t == "FFF"));
    // 与前端一致：嵌套/未闭合大括号按字面量处理，只有规整的 {x} 才是令牌
    assert_eq!(
        render_dir("{YYYY}/(未闭合", &ctx()).unwrap(),
        "2026/(未闭合"
    );
}

#[test]
fn camera_none_is_missing_context() {
    let mut c = ctx();
    c.camera = None;
    let err = render_dir("{YYYY}/{相机}", &c).unwrap_err();
    assert!(matches!(err, TemplateError::MissingContext(ref t) if t == "相机"));
    // 镜头同理
    let mut c = ctx();
    c.lens = None;
    let err = render_name("x-{镜头}", &c).unwrap_err();
    assert!(matches!(err, TemplateError::MissingContext(ref t) if t == "镜头"));
}

#[test]
fn sanitize_replaces_windows_illegal_chars() {
    assert_eq!(sanitize_component("a:b*c?d\"e<f>g|h"), "a_b_c_d_e_f_g_h");
    assert_eq!(sanitize_component("a\u{0007}b\u{001f}c"), "a_b_c");
    assert_eq!(sanitize_component("正常 name.JPEG"), "正常 name.JPEG");
}

#[test]
fn render_sanitizes_values_per_segment() {
    let mut c = ctx();
    c.camera = Some("Ni\"kon:|X".to_string());
    assert_eq!(render_dir("{YYYY}\\{相机}", &c).unwrap(), "2026\\Ni_kon__X");
    // 分隔符本身保留，不参与清理
    assert_eq!(render_dir("{YYYY}/{相机}", &c).unwrap(), "2026/Ni_kon__X");
}

#[test]
fn unique_path_increments_on_triple_conflict() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let root = dir.path();
    for name in ["a.txt", "a_1.txt", "a_2.txt"] {
        fs::write(root.join(name), b"x").expect("failed to write fixture file");
    }
    // 三连冲突：a.txt、a_1.txt、a_2.txt 均被占用 → a_3.txt
    assert_eq!(unique_path(root, "a.txt"), root.join("a_3.txt"));
    // 无冲突：原样返回
    assert_eq!(unique_path(root, "b.txt"), root.join("b.txt"));
    // 双扩展名：按最后一个点切分（stem=a.tar，ext=.gz）
    for name in ["a.tar.gz", "a.tar_1.gz"] {
        fs::write(root.join(name), b"x").expect("failed to write fixture file");
    }
    assert_eq!(unique_path(root, "a.tar.gz"), root.join("a.tar_2.gz"));
    // 目录不存在：纯计算，直接 join
    let missing = root.join("no_such_dir");
    assert_eq!(unique_path(&missing, "c.txt"), missing.join("c.txt"));
}

#[test]
fn resolve_captured_prefers_exif_else_mtime() {
    let exif_at = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
    let mtime = Utc.with_ymd_and_hms(2026, 9, 18, 0, 0, 0).unwrap();
    assert_eq!(resolve_captured(Some(exif_at), mtime), exif_at);
    assert_eq!(resolve_captured(None, mtime), mtime);
}
