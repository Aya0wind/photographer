//! AI 模型/向量索引空闲卸载（2026-09-29 内存审计落地项 A）。
//!
//! 实测：一次语义搜索把 CLIP 双塔 + usearch 索引拉进内存后 Rust 进程
//! 24MB → 342MB 物理 / 971MB 提交，且原实现（OnceLock 静态槽）永不释放。
//! 本模块提供「使用即记录 + 空闲超时统一释放」：
//! - `touch()`：模型/索引的每次真实使用打点（ensure/open 入口调用）；
//! - 空闲 ≥ 超时（默认 10 分钟，`PHOTO_HUB_AI_IDLE_SECS` 可调，便于测试）
//!   后由 supervisor 看护线程清空各静态槽——会话/索引 drop 释放其持有的资源；
//! - 下次使用按原惰性加载路径重载（冷启动 ~1-2s）。
//!
//! 安全性：各槽都是 `Mutex<Option<Session>>` 形态且推理全程持锁（借用
//! 检查器保证 session 借用不超出锁守卫生命周期），release 取同一把锁，
//! 只能在两次推理之间发生；usearch 池是 `Arc` 共享，在途查询持引用，
//! 清池不砸运行中的搜索。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Default)]
struct Clock {
    last_use: AtomicU64,
    used: AtomicU64,
    released: AtomicU64,
}
static CLOCK: Clock = Clock {
    last_use: AtomicU64::new(0),
    used: AtomicU64::new(0),
    released: AtomicU64::new(0),
};
impl Clock {
    fn touch(&self) {
        self.last_use.store(now_secs(), Ordering::SeqCst);
        self.used.fetch_add(1, Ordering::SeqCst);
    }
    fn idle_secs(&self) -> u64 {
        let last = self.last_use.load(Ordering::SeqCst);
        if last == 0 {
            u64::MAX
        } else {
            now_secs().saturating_sub(last)
        }
    }
    fn should_unload(&self, timeout: u64) -> bool {
        self.used.load(Ordering::SeqCst) != self.released.load(Ordering::SeqCst)
            && self.idle_secs() < u64::MAX
            && self.idle_secs() >= timeout
    }
    fn release_with(&self, release: impl FnOnce()) {
        let generation = self.used.load(Ordering::SeqCst);
        release();
        self.released.store(generation, Ordering::SeqCst);
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 模型/索引使用打点（ensure/open 入口调用）。
pub fn touch() {
    CLOCK.touch();
}

/// 空闲卸载超时（秒）：默认 600；`PHOTO_HUB_AI_IDLE_SECS` 覆盖（测试用）。
pub fn unload_timeout_secs() -> u64 {
    std::env::var("PHOTO_HUB_AI_IDLE_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(600)
}

/// 是否应执行一轮卸载：用过（加载过）且空闲超时，且上轮释放后没有新使用。
fn should_unload() -> bool {
    CLOCK.should_unload(unload_timeout_secs())
}

/// Drop cache ownership; borrowers retain their own references until completion.
pub fn release_all() {
    CLOCK.release_with(|| {
        super::embed::release();
        super::face::release();
        super::selection::release();
        super::semantic::release();
    });
    log::info!("AI 空闲模型与索引缓存已卸载");
}

/// 启动空闲看护线程（app setup 时挂一次；每 10 秒检查）。
pub fn spawn_idle_unloader(supervisor: &std::sync::Arc<crate::tasks::TaskSupervisor>) {
    supervisor.spawn("ai", "model-idle-unloader".into(), |controls| {
        while !controls.is_cancelled() {
            std::thread::sleep(Duration::from_secs(10));
            if controls.is_cancelled() {
                break;
            }
            super::face::release_idle_caches();
            crate::index::artifacts::release_idle();
            crate::edit::preview::release_expired();
            if should_unload() {
                release_all();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_gate_requires_use_and_timeout() {
        let clock = Clock::default();
        assert_eq!(clock.idle_secs(), u64::MAX);
        clock.touch();
        assert!(!clock.should_unload(600));
        clock.last_use.store(now_secs() - 601, Ordering::SeqCst);
        assert!(clock.should_unload(600));
        clock.release_with(|| {});
        assert!(!clock.should_unload(600));
    }

    #[test]
    fn use_during_release_remains_eligible_for_later_unload() {
        let clock = Clock::default();
        clock.touch();
        clock.release_with(|| clock.touch());
        clock.last_use.store(now_secs() - 601, Ordering::SeqCst);
        assert!(clock.should_unload(600));
        clock.release_with(|| {});
        assert!(!clock.should_unload(600));
    }
}
