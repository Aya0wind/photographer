//! Ordered, acknowledged camera receipts. Notifications do not consume files.
use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

struct Pending {
    id: String,
    due: Instant,
    attempts: u32,
}
#[derive(Default)]
pub struct Receipts {
    pending: VecDeque<Pending>,
    known: HashSet<String>,
    sequence: u64,
}
impl Receipts {
    pub fn enqueue(&mut self, id: String, now: Instant) {
        if !self.known.insert(id.clone()) {
            return;
        }
        self.sequence += 1;
        self.pending.push_back(Pending {
            id,
            due: now,
            attempts: 0,
        });
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    /// The oldest receipt gates later ones, preserving burst order through retries.
    pub fn next(&mut self, now: Instant) -> Option<String> {
        let item = self.pending.front_mut()?;
        if item.due > now {
            return None;
        }
        item.attempts = item.attempts.saturating_add(1);
        let delay = 1u64 << item.attempts.min(5);
        item.due = now + Duration::from_secs(delay);
        Some(item.id.clone())
    }
    pub fn acknowledge(&mut self, id: &str) {
        self.pending.retain(|item| item.id != id);
    }
    /// 是否仍有未确认收据（测试断言/诊断面；生产判定走磁盘账本）。
    #[allow(dead_code)]
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
    /// 待接收对象数（会话快照「N 待收」的真值来源）。
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_jpg_and_bursts_remain_fifo_until_commit() {
        let now = Instant::now();
        let mut queue = Receipts::default();
        for id in ["raw1", "jpg1", "raw2", "jpg2"] {
            queue.enqueue(id.into(), now);
        }
        assert_eq!(queue.next(now).as_deref(), Some("raw1"));
        assert!(queue.next(now).is_none());
        assert_eq!(
            queue.next(now + Duration::from_secs(2)).as_deref(),
            Some("raw1")
        );
        for id in ["raw1", "jpg1", "raw2", "jpg2"] {
            queue.acknowledge(id);
            if id != "jpg2" {
                assert!(queue.next(now + Duration::from_secs(10)).is_some());
            }
        }
        assert!(!queue.has_pending());
    }
    #[test]
    fn duplicated_notification_does_not_duplicate_a_committed_file() {
        let now = Instant::now();
        let mut queue = Receipts::default();
        queue.enqueue("a".into(), now);
        queue.enqueue("a".into(), now);
        assert_eq!(queue.sequence(), 1);
        queue.acknowledge("a");
        queue.enqueue("a".into(), now);
        assert!(!queue.has_pending());
    }
}
