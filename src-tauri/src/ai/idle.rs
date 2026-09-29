//! AI 模型/向量索引空闲卸载（2026-09-29 内存审计落地项 A）。
//!
//! 实测：一次语义搜索把 CLIP 双塔 + usearch 索引拉进内存后 Rust 进程
//! 24MB → 342MB 物理 / 971MB 提交，且原实现（OnceLock 静态槽）永不释放。
//! 本模块提供「使用即记录 + 空闲超时统一释放」：
//! - `touch()`：模型/索引的每次真实使用打点（ensure/open 入口调用）；
//! - 空闲 ≥ 超时（默认 10 分钟，`PHOTO_HUB_AI_IDLE_SECS` 可调，便于测试）
//!   后由 supervisor 看护线程清空各静态槽——会话/索引 drop 即归还 OS；
//! - 下次使用按原惰性加载路径重载（冷启动 ~1-2s）。
//!
//! 安全性：各槽都是 `Mutex<Option<Session>>` 形态且推理全程持锁（借用
//! 检查器保证 session 借用不超出锁守卫生命周期），release 取同一把锁，
//! 只能在两次推理之间发生；usearch 池是 `Arc` 共享，在途查询持引用，
//! 清池不砸运行中的搜索。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

static LAST_USE: AtomicU64 = AtomicU64::new(0);
static RELEASED: AtomicBool = AtomicBool::new(false);

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 模型/索引使用打点（ensure/open 入口调用；开销一次原子 store）。
pub fn touch() {
    LAST_USE.store(now_secs(), Ordering::Relaxed);
    RELEASED.store(false, Ordering::Relaxed);
}

/// 距上次使用的秒数（从未使用过返回 u64::MAX）。
pub fn idle_secs() -> u64 {
    let last = LAST_USE.load(Ordering::Relaxed);
    if last == 0 {
        u64::MAX
    } else {
        now_secs().saturating_sub(last)
    }
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
    !RELEASED.load(Ordering::Relaxed) && idle_secs() < u64::MAX && idle_secs() >= unload_timeout_secs()
}

/// 清空全部 AI 静态槽（各模块 release 取锁置空；在途推理不受影响）。
pub fn release_all() {
    super::embed::release();
    super::face::release();
    super::selection::release();
    super::semantic::release();
    RELEASED.store(true, Ordering::Relaxed);
}

/// 启动空闲看护线程（app setup 时挂一次；每分钟检查）。
pub fn spawn_idle_unloader(supervisor: &std::sync::Arc<crate::tasks::TaskSupervisor>) {
    supervisor.spawn("ai", "model-idle-unloader".into(), |controls| {
        while !controls.is_cancelled() {
            std::thread::sleep(Duration::from_secs(60));
            if controls.is_cancelled() {
                break;
            }
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
        // 从未使用：idle_secs = MAX，不应卸载（无模型可卸）
        assert_eq!(idle_secs(), u64::MAX);
        // 打点后未超时不应触发；超时应触发；释放后不再重复触发
        touch();
        assert!(idle_secs() < unload_timeout_secs() || unload_timeout_secs() == 0);
        assert!(!should_unload());
        release_all();
        assert!(!should_unload());
        // 再使用后恢复可卸载资格
        touch();
        assert!(!should_unload());
    }
}
