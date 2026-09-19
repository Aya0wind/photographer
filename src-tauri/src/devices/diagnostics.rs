//! 设备诊断日志：后台写入，IPC/消息泵只投递，磁盘故障不能阻塞应用。
use std::sync::{mpsc, OnceLock};
static LOG: OnceLock<mpsc::SyncSender<String>> = OnceLock::new();

#[allow(dead_code)] // 集成测试模块树不启动应用
pub fn init(config_dir: &std::path::Path) {
    let directory = config_dir.join("logs");
    let (tx, rx) = mpsc::sync_channel::<String>(1024);
    if LOG.set(tx).is_err() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("device-log".into())
        .spawn(move || {
            use std::io::Write;
            if std::fs::create_dir_all(&directory).is_err() {
                return;
            }
            let path = directory.join(format!(
                "devices-{}-{}.log",
                chrono::Local::now().format("%Y%m%d-%H%M%S"),
                std::process::id()
            ));
            let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            else {
                return;
            };
            let mut bytes = 0usize;
            while let Ok(line) = rx.recv() {
                if bytes > 4 * 1024 * 1024 {
                    let _ = file.set_len(0);
                    bytes = 0;
                }
                let _ = writeln!(file, "{line}");
                bytes += line.len() + 1;
            }
        });
    record(format!("app started pid={}", std::process::id()));
}

pub fn record(message: impl AsRef<str>) {
    let line = format!(
        "{} [{}] {}",
        chrono::Local::now().to_rfc3339(),
        std::thread::current().name().unwrap_or("unnamed"),
        message.as_ref()
    );
    eprintln!("{line}");
    if let Some(tx) = LOG.get() {
        let _ = tx.try_send(line);
    }
}
