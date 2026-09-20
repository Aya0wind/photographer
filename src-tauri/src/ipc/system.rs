//! 系统集成命令（M8 视频：HEVC 缺解码器时的系统播放回退支点）。
//!
//! `open_with_system(path)`：ShellExecuteW "open"——交给系统默认程序
//! （视频 = 默认播放器，照片 = 查看器）。不用 shell 插件（多一份权限面
//! 与依赖），直调 windows crate。路径不存在返回明确错误；ShellExecute
//! 失败（无关联程序等）映射为可读错误。

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
}
