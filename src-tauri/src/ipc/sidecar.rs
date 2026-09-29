//! DB 更新后的 XMP 后台同步：逐文件容错，首个错误和失败总数分别上报。

use super::AppState;
use crate::events::AppEvent;

pub(super) struct XmpSync {
    pub task: String,
    pub subject: &'static str,
    pub failure: &'static str,
}

pub(super) fn spawn_xmp_sync<T: Send + 'static>(
    state: &AppState,
    report: XmpSync,
    targets: Vec<T>,
    apply: impl Fn(T) -> Result<(), String> + Send + 'static,
) {
    let bus = state.bus.clone();
    state.supervisor.spawn("xmp", report.task, move |_| {
        let mut failed = 0usize;
        for target in targets {
            if let Err(error) = apply(target) {
                failed += 1;
                if failed == 1 {
                    bus.publish(AppEvent::AppError {
                        level: "warn".into(),
                        message: format!(
                            "{}已入库，但 XMP 边车{}：{error}",
                            report.subject, report.failure
                        ),
                        recoverable: true,
                    });
                }
            }
        }
        if failed > 1 {
            bus.publish(AppEvent::AppError {
                level: "warn".into(),
                message: format!(
                    "{}已入库，{failed} 个 XMP 边车{}",
                    report.subject, report.failure
                ),
                recoverable: true,
            });
        }
    });
}
