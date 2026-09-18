//! M1 命名模板引擎（spec §5.2 / T5）：目录/文件名模板渲染、Windows 安全清理、
//! 冲突递增与拍摄时间回退。
//! 令牌白名单与前端 `src/features/onboarding/onboardingConfig.ts` 的
//! `TEMPLATE_TOKENS` 保持一致（含 MM-DD）。

// lib 侧 `mod import;` 为私有模块且导入流水线（T7）尚未接线，库内暂无调用方
// （集成测试经 #[path] 直接编译本文件）；T7 落地后删除此属性。
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

/// 模板渲染错误：令牌不在白名单 / 上下文字段缺失。
#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    /// 令牌不在白名单内。
    #[error("未知令牌: {{{0}}}")]
    UnknownToken(String),
    /// `{相机}`/`{镜头}` 存在但上下文值为 None（如截图、转码文件）。
    #[error("模板上下文缺失: {{{0}}}")]
    MissingContext(String),
}

/// 渲染上下文。`captured_at` 由调用方先行归一（`resolve_captured`：EXIF ?? mtime）。
pub struct RenderCtx {
    /// 拍摄时间（UTC，chrono 格式化取 %Y/%y/%m/%d/%H/%M/%S）。
    pub captured_at: DateTime<Utc>,
    /// 相机（EXIF Make + Model），可能缺失。
    pub camera: Option<String>,
    /// 镜头（EXIF LensModel），M1 lite 可能缺失。
    pub lens: Option<String>,
    /// 原文件主名（不含扩展名）。
    pub original_stem: String,
    /// 扩展名（不含点，保留大小写）。
    pub ext: String,
}

/// 渲染目录模板（如 `{YYYY}/{MM-DD}`）：逐段过 `sanitize_component`，保留 `/`、`\` 分隔。
pub fn render_dir(template: &str, ctx: &RenderCtx) -> Result<String, TemplateError> {
    render(template, ctx)
}

/// 渲染文件名模板（如 `{YYYY}{原文件名}`）。
pub fn render_name(template: &str, ctx: &RenderCtx) -> Result<String, TemplateError> {
    render(template, ctx)
}

/// 拍摄时间归一：EXIF DateTimeOriginal 优先，缺失回退文件 mtime。
pub fn resolve_captured(exif_at: Option<DateTime<Utc>>, mtime: DateTime<Utc>) -> DateTime<Utc> {
    exif_at.unwrap_or(mtime)
}

/// Windows 文件名段清理：非法字符 `:*?"<>|` 与控制字符替换为 `_`。
pub fn sanitize_component(s: &str) -> String {
    s.chars()
        .map(|c| if is_illegal(c) { '_' } else { c })
        .collect()
}

/// 冲突递增的唯一路径（纯计算，不创建文件）：`a.txt` 被占用 → `a_1.txt`、
/// `a_2.txt`…；`dir` 不存在或无冲突时直接 `join`。
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    // 按最后一个点切分扩展名（保留大小写）；无扩展名或点开头的主名为空则整体视作主名
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    for i in 1u32.. {
        let candidate = dir.join(format!("{stem}_{i}{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("u32 序列不可能全部被占用")
}

fn is_illegal(c: char) -> bool {
    matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()
}

fn render(template: &str, ctx: &RenderCtx) -> Result<String, TemplateError> {
    let raw = render_raw(template, ctx)?;
    Ok(sanitize_segments(&raw))
}

/// 展开令牌。`{相机}`/`{镜头}` 上下文为 None → `MissingContext`。
fn render_raw(template: &str, ctx: &RenderCtx) -> Result<String, TemplateError> {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::with_capacity(template.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '{' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // 与前端 onboardingConfig 的正则 /\{([^{}]*)\}/ 语义对齐：
        // 找到内层无 `{` 的 `}` 才构成令牌；嵌套 `{` 或未闭合均按字面量处理
        match chars[i + 1..].iter().position(|&c| c == '}' || c == '{') {
            Some(rel) if chars[i + 1 + rel] == '}' => {
                let token: String = chars[i + 1..i + 1 + rel].iter().collect();
                out.push_str(&expand_token(&token, ctx)?);
                i += rel + 2;
            }
            _ => {
                out.push('{');
                i += 1;
            }
        }
    }
    Ok(out)
}

fn expand_token(token: &str, ctx: &RenderCtx) -> Result<String, TemplateError> {
    match token {
        "YYYY" => Ok(ctx.captured_at.format("%Y").to_string()),
        "YY" => Ok(ctx.captured_at.format("%y").to_string()),
        "MM" => Ok(ctx.captured_at.format("%m").to_string()),
        "MM-DD" => Ok(ctx.captured_at.format("%m-%d").to_string()),
        "DD" => Ok(ctx.captured_at.format("%d").to_string()),
        "HH" => Ok(ctx.captured_at.format("%H").to_string()),
        "mm" => Ok(ctx.captured_at.format("%M").to_string()),
        "ss" => Ok(ctx.captured_at.format("%S").to_string()),
        "原文件名" => Ok(format!("{}.{}", ctx.original_stem, ctx.ext)),
        "相机" => ctx
            .camera
            .clone()
            .ok_or_else(|| TemplateError::MissingContext(token.to_string())),
        "镜头" => ctx
            .lens
            .clone()
            .ok_or_else(|| TemplateError::MissingContext(token.to_string())),
        _ => Err(TemplateError::UnknownToken(token.to_string())),
    }
}

/// 按路径段清理：保留 `/` 与 `\` 作为分隔，其余每段过 `sanitize_component`。
fn sanitize_segments(rendered: &str) -> String {
    let mut out = String::with_capacity(rendered.len());
    let mut segment = String::new();
    for c in rendered.chars() {
        if c == '/' || c == '\\' {
            out.push_str(&sanitize_component(&segment));
            out.push(c);
            segment.clear();
        } else {
            segment.push(c);
        }
    }
    out.push_str(&sanitize_component(&segment));
    out
}
