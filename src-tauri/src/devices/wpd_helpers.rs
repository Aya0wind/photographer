//! WPD 数据格式和错误策略；无平台调用。
use super::DeviceError;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
/// WPD 也会暴露盘符（包括空卡槽）；这些由文件系统卷通道统一管理。
pub fn is_volume_alias(name: &str) -> bool {
    let name = name.trim().as_bytes();
    matches!(name.len(), 2 | 3)
        && name[0].is_ascii_alphabetic()
        && name[1] == b':'
        && (name.len() == 2 || matches!(name[2], b'\\' | b'/'))
}

/// MTP 层级路径拼接（统一 `/` 分隔）。
pub fn join_rel_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}

/// OLE 自动化日期（自 1899-12-30T00:00:00Z 的天数，含小数）→ UTC 时间。
/// OLE 序列 25569.0 恰为 Unix 纪元（1970-01-01）。
pub fn ole_date_to_utc(days: f64) -> Option<DateTime<Utc>> {
    if !days.is_finite() || !(-3_652_059.0..=3_652_059.0).contains(&days) {
        return None; // NaN/Inf/超 ±10000 年（chrono 表示范围余量）
    }
    let base = NaiveDate::from_ymd_opt(1899, 12, 30)?
        .and_hms_opt(0, 0, 0)?
        .and_utc();
    // 秒级精度足够（文件 mtime）；f64 在 4.6 万天尺度下分辨率远优于 1s
    let seconds = chrono::Duration::try_seconds((days * 86_400.0).round() as i64)?;
    base.checked_add_signed(seconds)
}

/// WPD 日期字符串解析：RFC3339 / ISO 无时区 / 空格分隔 / 基本格式
/// （`20240102T030405`）。无时区一律按 UTC。
pub fn parse_wpd_date_string(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S", "%Y%m%dT%H%M%S"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(naive.and_utc());
        }
    }
    None
}

/// Windows FILETIME（自 1601-01-01T00:00:00Z 的 100ns 计数）→ UTC 时间。
pub fn filetime_to_utc(low: u32, high: u32) -> Option<DateTime<Utc>> {
    let ticks = ((high as u64) << 32) | low as u64;
    let seconds = (ticks / 10_000_000) as i64;
    let nanos = ((ticks % 10_000_000) * 100) as u32;
    let unix = seconds.checked_sub(11_644_473_600)?; // 1601→1970 纪元差
    DateTime::from_timestamp(unix, nanos)
}

/// 枚举期可跳过的对象级错误（跳过并计数，不中断整卷枚举）：
/// - `AccessDenied`：受限对象/子树（相机受保护目录、并发会话被拒）；
/// - 「对象缺文件名/基本属性」：无名内部对象（播放列表/系统对象——
///   2026-09-18 真机：ILCE-7RM5 的 o18179B 导致整树失败、设备注册不上）。
///
/// 仍致命：会话丢失/拔线/枚举接口失败等（「要么完整要么报错」）。
pub fn is_object_level_skip(err: &DeviceError) -> bool {
    matches!(err, DeviceError::AccessDenied)
        || matches!(err, DeviceError::Other(msg) if msg.starts_with("对象缺"))
}

/// CoInitializeEx 返回码 → apartment guard 决策（三态语义，纯函数单测覆盖）。
///
/// - S_OK（0）：本线程首次初始化成功 → `Ok(true)`（drop 时配对 CoUninitialize）；
/// - S_FALSE（1）：线程已初始化（他人持有引用）→ `Ok(false)`：沿用现有
///   apartment，**绝不 CoUninitialize**（不注销他人的初始化）；
/// - RPC_E_CHANGED_MODE（0x80010106）：线程已按**其他线程模型**初始化——
///   典型场景是 tao 事件循环把 Tauri IPC 命令线程（主线程）初始化为 STA。
///   → `Ok(false)`：沿用现有 apartment 继续调用（WPD 在 STA 线程上合法
///   可用，Windows Explorer 本身就是 STA 用 WPD），同样绝不 uninit；
/// - 其余：`Err(hr)` 上抛（调用方转设备错误语义）。
///
/// 返回值 `Ok(owned)` 的 owned=true 表示本 guard 拥有这次初始化。
pub fn com_apartment_owned(hr: i32) -> Result<bool, i32> {
    match hr as u32 {
        0 => Ok(true),            // S_OK
        1 => Ok(false),           // S_FALSE
        0x8001_0106 => Ok(false), // RPC_E_CHANGED_MODE（线程为 STA）
        _ => Err(hr),
    }
}
