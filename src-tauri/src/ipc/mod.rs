//! IPC 命令层，按命名空间拆分模块。

pub mod settings;

use std::path::PathBuf;
use std::sync::Mutex;

use crate::settings::Settings;

/// 全局应用状态：内存中的设置快照 + 配置目录（app_config_dir）。
pub struct AppState {
    pub settings: Mutex<Settings>,
    pub config_dir: PathBuf,
}
