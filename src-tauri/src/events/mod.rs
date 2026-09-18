//! 领域事件总线：模块间解耦通信的唯一通道（spec §8）。
//!
//! 共享领域类型（SourceKind/AssetKind/FileState/JobStats）也定义在此，
//! 因为它们同时被 events/db/import/devices 多模块引用。
//! Rust 内部用 tokio::sync::broadcast 分发；tauri 启动后由转发任务
//! 把 `AppEvent` 序列化 emit 给前端（`app://event`），IPC 层负责接线。

// M1 骨架：消费者（db/import/devices 实现与 IPC 转发任务）落地后移除此 allow。
#![allow(dead_code)]

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// broadcast 通道容量：高频 FileProgress 场景下足够吸收消费抖动。
pub const CHANNEL_CAPACITY: usize = 1024;

// ---------------------------------------------------------------------------
// 共享领域类型
// ---------------------------------------------------------------------------

/// 设备源类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    /// 卷设备（读卡器/U盘，盘符）
    Volume,
    /// WPD/MTP 设备（相机直连）
    Mtp,
}

/// 资产类型（classify 的产出，assets.kind）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AssetKind {
    Photo,
    Raw,
    Video,
    Other,
}

/// job_files.state 领域枚举（db 与导入引擎共用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileState {
    Pending,
    Copying,
    Verified,
    Skipped,
    Failed,
}

/// 导入会话统计（SessionFinished 载荷 + 总结弹窗数据源）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobStats {
    pub total_files: u64,
    pub done_files: u64,
    pub skipped_duplicates: u64,
    pub failed_files: u64,
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub elapsed_ms: u64,
    pub bytes_per_sec: f64,
}

// ---------------------------------------------------------------------------
// 事件（扁平单枚举，serde tag=type，前端做可辨识联合处理）
// ---------------------------------------------------------------------------

/// 领域事件。字段 camelCase 序列化；`type` 为判别字段。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AppEvent {
    // 设备
    DeviceArrived {
        id: String,
        kind: SourceKind,
        name: String,
    },
    DeviceRemoved {
        id: String,
    },
    /// 拔线/掉盘：导入任务应自动暂停
    DeviceUnavailable {
        id: String,
    },

    // 导入
    ImportSessionStarted {
        job_id: i64,
        total_files: u64,
        total_bytes: u64,
    },
    /// 高频事件，发布侧必须用 Throttle 节流（≥100ms 或 ≥20 文件合并）
    ImportFileProgress {
        job_id: i64,
        done_files: u64,
        done_bytes: u64,
        current_file: String,
        bytes_per_sec: f64,
    },
    ImportPaused {
        job_id: i64,
    },
    ImportResumed {
        job_id: i64,
    },
    ImportCancelled {
        job_id: i64,
    },
    ImportSessionFinished {
        job_id: i64,
        stats: JobStats,
    },
    ImportFileCompleted {
        job_id: i64,
        src: String,
        dst: String,
        state: FileState,
    },

    // 错误
    AppError {
        level: String,
        message: String,
        recoverable: bool,
    },
}

// ---------------------------------------------------------------------------
// 总线与节流
// ---------------------------------------------------------------------------

/// Clone 即可跨线程/跨模块共享。
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<AppEvent>,
}

impl EventBus {
    pub fn new() -> Self {
        let (sender, _rx) = broadcast::channel(CHANNEL_CAPACITY);
        Self { sender }
    }

    /// 发布事件。无订阅者/接收者落后被裁剪时静默忽略，绝不报错。
    pub fn publish(&self, event: AppEvent) {
        let _ = self.sender.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AppEvent> {
        self.sender.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// 简单时间节流器：FileProgress 等高频事件的发布闸门。
pub struct Throttle {
    last: Option<Instant>,
    min_interval: Duration,
}

impl Throttle {
    pub fn new(min_interval: Duration) -> Self {
        Self {
            last: None,
            min_interval,
        }
    }

    /// 距上次放行超过 min_interval 才放行。
    pub fn should_fire(&mut self) -> bool {
        let now = Instant::now();
        match self.last {
            Some(last) if now.duration_since(last) < self.min_interval => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_and_subscribe_roundtrip() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        bus.publish(AppEvent::DeviceRemoved { id: "E:".into() });
        let received = rx.blocking_recv().expect("should receive");
        assert_eq!(received, AppEvent::DeviceRemoved { id: "E:".into() });
    }

    #[test]
    fn publish_without_subscribers_is_silent() {
        let bus = EventBus::new();
        bus.publish(AppEvent::DeviceRemoved { id: "E:".into() });
        // 不 panic 即通过
    }

    #[test]
    fn slow_subscriber_gets_lagged_not_panic() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let last_id = "last".to_string();
        for i in 0..CHANNEL_CAPACITY + 10 {
            let id = if i == CHANNEL_CAPACITY + 9 {
                last_id.clone()
            } else {
                format!("E{i}")
            };
            bus.publish(AppEvent::DeviceRemoved { id });
        }
        // 滞后接收者拿到 Lagged 错误而非 panic；跳过被裁剪的事件后仍能收到最新事件
        let mut got_last = false;
        for _ in 0..CHANNEL_CAPACITY + 20 {
            match rx.try_recv() {
                Ok(AppEvent::DeviceRemoved { id }) if id == last_id => {
                    got_last = true;
                    break;
                }
                Ok(_) | Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(broadcast::error::TryRecvError::Closed) => panic!("sender dropped early"),
                Err(broadcast::error::TryRecvError::Empty) => break,
            }
        }
        assert!(got_last, "落后订阅者最终应收到最新事件（允许中途 Lagged）");
    }

    #[test]
    fn throttle_fires_once_within_interval() {
        let mut t = Throttle::new(Duration::from_millis(100));
        assert!(t.should_fire(), "首次必须放行");
        assert!(!t.should_fire(), "间隔内必须拦下");
    }

    #[test]
    fn event_serializes_with_camel_case_tag() {
        let ev = AppEvent::DeviceArrived {
            id: "E:".into(),
            kind: SourceKind::Volume,
            name: "SD 卡".into(),
        };
        let json = serde_json::to_value(&ev).expect("serialize");
        assert_eq!(json["type"], "deviceArrived");
        assert_eq!(json["kind"], "volume");
    }

    #[test]
    fn file_state_serializes_camel_case() {
        assert_eq!(
            serde_json::to_value(FileState::Verified).unwrap(),
            "verified"
        );
    }
}
