//! 导入命名工具（2026-10-09 单数据库多照片库 §三 定案后仅剩三件）：
//! 拍摄时间归一（EXIF 优先回退 mtime）、Windows 文件名段清理、冲突递增
//! 唯一路径。目录/文件名**模板引擎已退役**——落盘布局固定纯时间
//! `{照片库}/{拍摄年}/{拍摄月}/{原文件名}`（见 `import::engine::pipeline`），
//! 不再可配置。

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

/// 拍摄时间归一：EXIF DateTimeOriginal 优先，缺失回退文件 mtime。
/// 纯时间布局的年/月段与 `assets.captured_at` 落库共用此口径。
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
/// `a_2.txt`…；`dir` 不存在或无冲突时直接 `join`。导入重名策略的唯一实现
/// （§三「重名沿用现有 rename 策略」）。
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
