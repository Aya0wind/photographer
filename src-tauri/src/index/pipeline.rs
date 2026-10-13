//! Asset-level targets and all wakeups use the durable dependency scheduler.
//! Importing committed photos and indexing may overlap.
use std::path::PathBuf;

/// 消费池选择（kick 用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pools(u8);

impl Pools {
    pub const IMAGE: Self = Self(1);
    pub const SELECTION: Self = Self(2);
    pub const ALL: Self = Self(3);
}

/// All legacy pool wakeups converge on the same scheduler.
pub fn kick(
    db_dir: PathBuf,
    supervisor: &std::sync::Arc<crate::tasks::TaskSupervisor>,
    pools: Pools,
) {
    let _ = pools;
    crate::index::scheduler::kick(db_dir, supervisor);
}

/// AI 开启判定（选片账建档/kick 的统一口径）。
pub fn selection_enabled(ai: &crate::settings::AiSettings) -> bool {
    ai.enable_clip || ai.enable_face
}
