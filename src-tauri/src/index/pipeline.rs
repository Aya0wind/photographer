//! 统一索引管线（2026-10-12 定案）：触发、追加、顺序、并发的唯一契约。
//!
//! # 触发（什么时候开始跑）
//!
//! | 触发点 | 入口 | 唤醒池 |
//! |---|---|---|
//! | 导入收尾（done_files>0） | [`enqueue_import_backlog`] + [`kick`] ALL | 图片 + 选片 |
//! | 联拍收片入册 | 任务建档（插入播种 thumb；收片建选片账）+ [`kick`] ALL | 图片 + 选片 |
//! | 应用启动（代际自愈） | 各 refresh_*_for_generation 建档后 [`kick`] | 按开关 |
//! | 模型就绪 watch（facemesh/语义/人脸） | 各自 kick_*_if_ready | 选片/语义/人脸 |
//! | 手动触发（设置页/任务抽屉） | index_kick_now(kind) | 按通道 |
//! | 重建完成 | kick_rebuilt_channel | 按通道 |
//!
//! 一切 kick 共享导入在场让路闸（[`crate::index::import_in_flight`]）：
//! 导入期间不启动，导入收尾统一唤醒。
//!
//! # 追加（导入中待处理图片什么时候进队列）
//!
//! - **缩略图**：资产插入时即播种 pending 任务（每文件即时，不等导入结束）。
//! - **EXIF**：导入流读首段时同步提取落库，无任务行。
//! - **phash / eyes / blur**：导入收尾（done_files>0）一次性幂等建档
//!   （[`enqueue_import_backlog`]）；eyes/blur 仅 AI 开启的库建。
//!
//! # 顺序（各索引先后）
//!
//! - 图片池内认领优先级：exif（解锁筛选）→ hash（毫秒级查重层）→ phash
//!   → thumb（数量大头）。
//! - **选片（eyes/blur）在缩略图之后**：认领门控 `thumb_state != 0`
//!   （[`crate::db::Db::claim_selection_task_after_thumb`]）——选片的输入
//!   就是缩略图缓存，门控保证源图只被缩略图任务解码一次；门控等待中
//!   选片 worker 小睡重试，缩略图推进后自然接上。缩略图永久失败
//!   （thumb_state=2）的资产照常认领，按不可用收尾。
//! - eyes 另受模型就绪门控（未装 → 任务 pending，「等待模型」提示）。
//!
//! # 并发（worker 充分利用，跨池不互相等）
//!
//! - 两个消费池各自独立 coalesced 派发：图片池（thumb/exif/hash/phash，
//!   supervisor kind "index"）与选片池（blur/eyes，kind "selection"）。
//! - 预算域分离（`index_budget::Domain::{Image,Selection}`，各 = 物理核半
//!   数满额）：两池各自排队发牌，**不同照片的任务不在同一个 FIFO 票序里
//!   互等**。
//! - 暂停语义按池：`index_task_pause/resume(pool)`；导入让路闸两池全停。
//! - 同库并发安全：认领经 SQLite 写锁串行（UPDATE..RETURNING 单行独占）；
//!   重建持 `image_index_lock` 写闸等在途读闸收尾。
//!
//! # 解码共享（避免重复读嵌入图）
//!
//! - 源图的解码只发生在缩略图任务（逐档 turbojpeg DCT 缩放解码 /
//!   RAW 内嵌直出）；选片分析读缩略图缓存：照片读 2048 档（内存降
//!   1024），RAW 读 1024 内嵌直出档——门控 + 档位对齐后照片路径零额外
//!   源解码。
use std::path::PathBuf;

use crate::db::Db;

/// 消费池选择（kick 用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pools(u8);

impl Pools {
    pub const IMAGE: Self = Self(1);
    pub const SELECTION: Self = Self(2);
    pub const ALL: Self = Self(3);

    fn has(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

/// 导入收尾的一次性任务追加（幂等）：phash 补差 + AI 开启库的选片账
/// （eyes/blur）。缩略图任务在插入时已播种，此处不重复。
pub fn enqueue_import_backlog(db: &Db, selection_enabled: bool) {
    let _ = db.create_phash_tasks_for_unindexed();
    if selection_enabled {
        let _ = db.create_eyes_tasks_for_unindexed();
        let _ = db.create_blur_tasks_for_unindexed();
    }
}

/// 统一唤醒入口（导入在场让路）：按池踢发图片索引与选片索引（blur）。
/// eyes 的模型门控在 [`crate::ai::selection::kick_eyes_if_ready`] 入口内
/// （需要 ModelManager/EventBus，由各触发点携带调用）——模型就绪 watch、
/// 导入收尾、启动钩子三处即全部场景。幂等（coalesced）：已在跑的池置
/// dirty 重排水，未跑的池派发。
pub fn kick(
    db_dir: PathBuf,
    supervisor: &std::sync::Arc<crate::tasks::TaskSupervisor>,
    pools: Pools,
) {
    if pools.has(Pools::IMAGE) {
        crate::index::kick(db_dir.clone(), supervisor);
    }
    if pools.has(Pools::SELECTION) {
        crate::ai::selection::kick_selection_index(db_dir, supervisor);
    }
}

/// AI 开启判定（选片账建档/kick 的统一口径）。
pub fn selection_enabled(ai: &crate::settings::AiSettings) -> bool {
    ai.enable_clip || ai.enable_face
}
