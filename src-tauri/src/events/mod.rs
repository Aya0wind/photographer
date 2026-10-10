//! 领域事件总线：模块间解耦通信的唯一通道（spec §8）。
//!
//! 共享领域类型（SourceKind/AssetKind/FileState/JobStats）也定义在此，
//! 因为它们同时被 events/db/import/devices 多模块引用。
//! Rust 内部用 tokio::sync::broadcast 分发；tauri 启动后由转发任务
//! 把 `AppEvent` 序列化 emit 给前端（`app://event`），IPC 层负责接线。

use std::sync::Arc;
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
    /// 本地文件夹（M2“从文件夹导入”，id 为 `FOLDER:<绝对路径>`）
    Folder,
}

/// 资产类型（classify 的产出，assets.kind）。`Ord` 供 BTreeMap 统计键使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AssetKind {
    Photo,
    Raw,
    /// 仅用于读取旧版本留下的资产行；扫描和导入不再产生此类型。
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
    /// move 模式下成功删源的文件数（copy 恒 0）。
    pub moved: u64,
    /// move 模式下删源失败数（不影响 done_files，仅告警）。
    pub source_delete_failed: u64,
}

/// 安全清卡统计（M2 F1，CleanFinished 载荷）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanStats {
    /// 成功删除的源文件数。
    pub deleted: u64,
    /// 复验不一致或删除失败的文件数。
    pub failed: u64,
    /// 释放的字节数。
    pub freed_bytes: u64,
    /// 逐文件失败原因（与 failed 对应）。
    pub errors: Vec<String>,
}

/// 设备文件条目 DTO（DeviceFilesProgress 载荷；导入向导源树/勾选表数据）。
/// 定义在 events 层供各处共享（ipc 层重导出保持 `ipc::FileEntryDto` 路径兼容）；
/// mtime 为 RFC3339 字符串。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntryDto {
    pub id: String,
    pub rel_path: String,
    pub size: u64,
    pub mtime: String,
}

// ---------------------------------------------------------------------------
// 事件（扁平单枚举，serde tag=type，前端做可辨识联合处理）
// ---------------------------------------------------------------------------

/// 领域事件。字段 camelCase 序列化；`type` 为判别字段。
/// 注意：枚举级 rename_all 只作用于变体名（tag 值）；变体字段必须用
/// rename_all_fields（serde ≥1.0.184）。曾因丢失后者导致 job_id 等
/// snake_case 字段直达前端（真机：进度卡全程空白）——下方有回归测试钉死。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AppEvent {
    DeviceFilesProgress {
        id: String,
        files: Vec<FileEntryDto>,
    },
    /// 内部系统通知；不转发给界面。最终在线状态由设备编排器发布。
    DeviceTopologyChanged {
        id: String,
        arrived: bool,
    },
    DeviceScanFailed {
        id: String,
        message: String,
    },
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
    /// 设备扫描完成（热插拔触发或 device_scan 命令）：携带统计快照
    DeviceScanned {
        id: String,
        name: String,
        kind: SourceKind,
        snapshot: crate::devices::orchestrator::DeviceSnapshot,
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
        /// 已结算字节（完成+跳过+失败）——进度条口径：重复文件走 skip
        /// 时不推进 done_bytes，若按 done 计算进度条会原地不动（真机坑）。
        settled_bytes: u64,
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

    /// 事件链路自检（event_ping 命令 → 转发器 → 前端）：携带时间戳供
    /// 前端回显对账。诊断专用，不参与业务。
    Probe {
        ts: String,
    },

    // 缩略图按需管线（M3）
    /// 后台缩略图生成完成（asset_thumb_get 未命中入队后的回执）：path 为缓存文件
    /// 绝对路径；生成失败（超时/不可解码）为 null——前端据此停止等待重试。
    ThumbnailReady {
        asset_id: i64,
        size: u16,
        path: Option<String>,
    },

    // 安全清卡（M2 F1）
    /// 清卡开始：候选数与总字节（删前逐文件复验）。
    CleanStarted {
        job_id: i64,
        count: u64,
        bytes: u64,
    },
    CleanFinished {
        job_id: i64,
        stats: CleanStats,
    },

    // 目录迁移（M3，spec §5.11：dbDir 两阶段 / photoRoot switch|migrate）
    /// 迁移开始：kind = "dbDir" | "photoRoot"。
    MigrationStarted {
        kind: String,
        total_bytes: u64,
    },
    /// 迁移进度（发布侧节流 ≥100ms）：current 为当前文件名/相对路径。
    MigrationProgress {
        done_bytes: u64,
        current: String,
    },
    /// 迁移收尾：ok=是否全部成功；failed=失败文件数（dbDir 失败已回滚
    /// 不留脏；photoRoot 失败可断点恢复）。
    MigrationFinished {
        ok: bool,
        failed: u64,
    },

    // AI 模型在线下载（M4 前置）
    /// 下载进度（1s 节流）。
    AiModelDownloadProgress {
        id: String,
        done_bytes: u64,
        total_bytes: u64,
    },
    /// 下载收尾：ok + 失败原因（取消/校验失败/两源不可用）。
    AiModelDownloadFinished {
        id: String,
        ok: bool,
        error: Option<String>,
    },

    // 索引任务（导入/索引分离，M3.5）
    /// 启动恢复：检测到上次中断的索引待办（running 已复位 pending），
    /// worker 自动续跑。
    IndexTaskResumed {
        pending: u64,
    },
    /// 索引任务进度（kind = "ai" 等；done/total 为本轮任务数）。
    IndexTaskProgress {
        kind: String,
        done: u64,
        total: u64,
    },
    /// 语义索引一轮回填收尾（done=本轮新嵌入数）。done>0 时前端自动重建
    /// 智能相册标签索引（此前只能手动去设置点「根据标签重建」；且早于语义
    /// 完成建过的索引会把 0 命中缓存住——收尾全量刷新一并修复）。
    AiIndexFinished {
        done: u64,
    },

    // 连拍分组（M6）
    /// 重组完成：groups = 组数，photos = 入组资产数（设置页/画廊刷新数据源）。
    BurstsRegrouped {
        groups: u64,
        photos: u64,
    },

    // 监视文件夹（F4 v1）
    /// 轮询发现新文件并已发起自动入册：folder = 监视目录，files = 新文件数。
    WatchFolderImported {
        folder: String,
        files: u64,
    },

    // 导出任务（阶段 D 基础编辑与导出；任务真值在 export_job 表，事件只做
    // 进度/收尾通知——UI 重启后从库里读历史/终态）
    /// 单文件导出阶段推进：phase = "render"|"encode"|"write"|"register"。
    ExportTaskProgress {
        job_id: i64,
        asset_id: i64,
        phase: String,
    },
    /// 导出收尾：ok + 输出路径（album 模式附带新资产 id）+ 失败原因。
    ExportTaskFinished {
        job_id: i64,
        asset_id: i64,
        ok: bool,
        output_path: Option<String>,
        new_asset_id: Option<i64>,
        error: Option<String>,
    },

    // 联机拍摄（阶段 E-1「WPD 零驱动联拍」；导入落库走既有导入管线，
    // 本事件只通知「新对象已到达」——前端接 tethering_camera_list /
    // camera_probe / camera_capture 命令组）
    /// 拍摄产生的新对象已到达相机（OBJECT_ADDED → Advise 管线）。
    TetheringObjectAdded {
        pnp_id: String,
        object_name: String,
        object_size: u64,
    },

    TetheringPhotoAdded {
        session_id: String,
        library_id: String,
        album_id: i64,
        asset_id: i64,
        name: String,
    },
    TetheringStatus {
        session_id: String,
        connected: bool,
        error: Option<String>,
    },
    TetheringSettingsChanged {
        session_id: String,
    },

    // 拍摄地图（geo 模块）：下载/回填进度与索引刷新
    /// 地理数据管线进度（stage = download/load/backfill；done/total 按阶段语义：
    /// download 为文件数、backfill 为资产数）。
    MapGeoProgress {
        stage: String,
        done: u64,
        total: u64,
        message: Option<String>,
    },
    /// 地区索引有新真值：回填批次完成 / 单资产编辑联动重索引 / 数据包更新重刷。
    MapRegionsUpdated,

    // 数据库注册表（2026-10-09 多数据库修正；命令见 ipc/databases.rs）
    /// 数据库注册表或激活数据库变更（新建/切换/移除）：前端全量刷新——
    /// 重拉 database_list、照片库列表与画廊等一切库内数据（换库语义）。
    DatabasesChanged,

    // 照片库登记表（2026-10-09 单数据库多照片库定案；命令骨架见
    // ipc/photo_library.rs，实装 M1/M2）
    /// photos_libraries 变更（新建/移除登记/在线状态翻转）：存储页与
    /// 导入目标选择器重拉 photo_library_list。
    PhotoLibrariesChanged,
    /// 照片库扫描任务进度（从文件夹建立的批量登记 + 增量扫描共用；发布侧节流）。
    LibraryScanProgress {
        library_id: String,
        registered: u64,
        total: u64,
    },
    /// 照片库扫描收尾：registered=本轮登记数（含 missing 重绑），skipped=
    /// 去重/冷却跳过数；cross_library_duplicates=§四 收尾总结「N 张与
    /// 其他照片库内容相同」（跨库照常登记不去重，计数是 registered 子集；
    /// 零/未核对不携带）。
    LibraryScanFinished {
        library_id: String,
        registered: u64,
        skipped: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        cross_library_duplicates: Option<u64>,
    },
    /// 库扫描 presence 精准事件（AfterFrame changed-media 借鉴）：本轮判回
    /// 在线的资产集合——missing 重绑/恢复/重算落定、缺席账结清（文件回位）
    /// 与整库 offline→online 翻转。前端画廊按瓦片精准刷新缺失角标（缩略图
    /// 管线的 missing 终态随之失效重查），不做全量重拉。
    AssetsPresenceChanged {
        library_id: String,
        asset_ids: Vec<i64>,
    },

    // 相册导出为文件夹（M6，Photo Hub → LR 互操作；命令骨架见 ipc/album_export.rs）
    /// 导出任务进度：done=已导出资产数，total=相册内待导出数。
    AlbumExportProgress {
        task_id: i64,
        done: u64,
        total: u64,
    },
    /// 导出收尾：ok=是否全部成功，exported=导出文件数，linked=其中硬链接数。
    AlbumExportFinished {
        task_id: i64,
        ok: bool,
        exported: u64,
        linked: u64,
        error: Option<String>,
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

// ---------------------------------------------------------------------------
// 事件转发核心（bus → 前端通道；lib.rs 经 TaskSupervisor 起线程调用）
// ---------------------------------------------------------------------------

/// 单次转发一行事件并计数（每 100 条打一行活性日志）。
/// 返回 false 表示通道已关闭（发送端全部消亡，正常退出）。
fn forward_once(
    event: AppEvent,
    emit: &Arc<dyn Fn(&AppEvent) + Send + Sync>,
    forwarded: &mut u64,
) -> bool {
    emit(&event);
    *forwarded += 1;
    if forwarded.is_multiple_of(100) {
        eprintln!("事件转发已启动并转发 {forwarded} 条（活性心跳）");
    }
    true
}

/// 转发循环：阻塞收事件 → emit。Lagged（消费落后被裁剪）跳过继续；
/// Closed（无发送者）正常返回。
fn forward_loop(
    rx: &mut broadcast::Receiver<AppEvent>,
    emit: &Arc<dyn Fn(&AppEvent) + Send + Sync>,
) {
    let mut forwarded = 0u64;
    loop {
        match rx.blocking_recv() {
            Ok(event) => {
                let _ = forward_once(event, emit, &mut forwarded);
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                eprintln!("事件转发落后被裁剪 {n} 条（继续）");
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// 带自恢复的转发监督循环：emit 侧 panic 被捕获 → on_panic 上报 →
/// 退避 1s 重启循环（转发器死亡 = 前端全盲，绝不静默退出）。
/// 通道正常关闭时返回。
pub fn forward_supervised(
    rx: &mut broadcast::Receiver<AppEvent>,
    emit: Arc<dyn Fn(&AppEvent) + Send + Sync>,
    on_panic: impl Fn(String),
) {
    let mut restarts = 0u32;
    loop {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            forward_loop(rx, &emit);
        }));
        match outcome {
            Ok(()) => return, // Closed：正常退出
            Err(payload) => {
                restarts += 1;
                let detail = if let Some(s) = payload.downcast_ref::<&str>() {
                    (*s).to_string()
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "未知 panic 载荷".to_string()
                };
                let msg = format!("事件转发器 panic（第 {restarts} 次自恢复）: {detail}");
                eprintln!("{msg}");
                on_panic(msg);
                std::thread::sleep(std::time::Duration::from_secs(1)); // 退避，防风暴
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

    #[test]
    fn assets_presence_changed_serializes_camel_case() {
        let ev = AppEvent::AssetsPresenceChanged {
            library_id: "lib-1".into(),
            asset_ids: vec![7, 12],
        };
        let json = serde_json::to_value(&ev).expect("serialize");
        assert_eq!(json["type"], "assetsPresenceChanged");
        assert_eq!(json["libraryId"], "lib-1");
        assert_eq!(json["assetIds"], serde_json::json!([7, 12]));
    }
}
