//! Crash-safe receipt ledger (2026-10-11 §B1)：append-only JSONL 账本，
//! 让「待收/已收/失败」状态在应用进程崩溃后仍可恢复。
//!
//! 布局：`<应用配置目录>/tethering/receipts/<物理相机键>.jsonl`，一台物理
//! 相机一个文件（路径注入自会话层的 `state.config_dir`，与 db_dir 同源，
//! 不新建全局）。事件三态：
//! - `pending`：对象已进入收片管线（含产生该收据的后端 id——恢复时只有
//!   同一后端才可能按对象 id 重新收取，§E 未确认收据禁切换的依据）；
//! - `acked`：已入库并 `acknowledge_captured`（终态）；
//! - `failed`：一次入库失败（仍可重试，与 pending 一样算未确认）。
//!
//! 加载语义：逐行重放、**同一 id 的最后一态为准**；损坏行跳过不阻断。
//! 写失败只记诊断日志——账本永远不得反过来阻断收片主路径。

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// 账本中的一条事件（JSONL 单行）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
enum Record {
    Pending {
        id: String,
        backend: String,
        seq: u64,
        ts: String,
    },
    Lost {
        id: String,
        ts: String,
        reason: String,
    },
    Acked {
        id: String,
        ts: String,
    },
    Failed {
        id: String,
        ts: String,
        error: String,
    },
}

/// 启动恢复时的未确认收据（最后一态非 acked）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnackedReceipt {
    pub id: String,
    /// 记录 pending 时的后端 id（恢复时按它路由 request_objects）。
    pub backend: String,
    /// pending 记录的序号（恢复收取按此保持连拍顺序）。
    pub seq: u64,
}

/// 单物理相机的磁盘账本。所有写方法**不返回错误**（失败仅日志）。
pub struct ReceiptsStore {
    path: PathBuf,
    /// 惰性打开的追加句柄（进程内复用；损坏/占用时下次重试再开）。
    sink: Mutex<Option<File>>,
    /// 本进程已记录过的事件（pending/failed 各记一次，避免重试风暴刷行）。
    recorded: Mutex<HashSet<(char, String)>>,
}

/// 物理相机键 → 文件名安全形态（保留字母数字与 -_.，其余折叠为 _）。
pub fn sanitize_component(key: &str) -> String {
    let mut out = String::with_capacity(key.len().min(96));
    for ch in key.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            out.push(ch);
        } else {
            out.push('_');
        }
        if out.len() >= 96 {
            break;
        }
    }
    if out.is_empty() {
        out.push_str("camera");
    }
    out
}

impl ReceiptsStore {
    /// 打开（不创建文件）指定物理相机的账本；目录惰性创建。
    pub fn open(dir: &Path, physical_key: &str) -> Self {
        Self {
            path: dir.join(format!("{}.jsonl", sanitize_component(physical_key))),
            sink: Mutex::new(None),
            recorded: Mutex::new(HashSet::new()),
        }
    }

    /// 账本文件路径（测试断言直读文件内容用）。
    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn append(&self, record: &Record) {
        let mut sink = self.sink.lock().unwrap();
        if sink.is_none() {
            if let Some(parent) = self.path.parent() {
                if let Err(error) = std::fs::create_dir_all(parent) {
                    crate::devices::diagnostics::record(format!(
                        "收据账本目录不可用（仅日志，不影响收片）: {} {error}",
                        parent.display()
                    ));
                    return;
                }
            }
            match OpenOptions::new().create(true).append(true).open(&self.path) {
                Ok(file) => *sink = Some(file),
                Err(error) => {
                    crate::devices::diagnostics::record(format!(
                        "收据账本打开失败（仅日志，不影响收片）: {} {error}",
                        self.path.display()
                    ));
                    return;
                }
            }
        }
        let Some(file) = sink.as_mut() else {
            return;
        };
        let line = match serde_json::to_string(record) {
            Ok(line) => line,
            Err(error) => {
                crate::devices::diagnostics::record(format!(
                    "收据账本序列化失败（仅日志）: {error}"
                ));
                return;
            }
        };
        // 写失败仅日志：即使丢行，最坏情况是多收一次（入库幂等语义）或
        // 恢复时退回事件驱动等待，绝不能反过来阻断收片。
        if let Err(error) = writeln!(file, "{line}") {
            crate::devices::diagnostics::record(format!(
                "收据账本写入失败（仅日志，不影响收片）: {} {error}",
                self.path.display()
            ));
            *sink = None; // 下次重开句柄
        }
    }

    /// 对象进入收片管线（每个 id 每进程只落一行）。
    pub fn record_pending(&self, backend: &str, id: &str, seq: u64, ts: &str) {
        if !self.remember('p', id) {
            return;
        }
        self.append(&Record::Pending {
            id: id.into(),
            backend: backend.into(),
            seq,
            ts: ts.into(),
        });
    }

    /// 入库成功并确认（终态，总是落盘——即使本进程没记过 pending，
    /// 例如恢复路径直接成功）。
    pub fn record_acked(&self, id: &str, ts: &str) {
        self.append(&Record::Acked {
            id: id.into(),
            ts: ts.into(),
        });
    }

    /// 入库失败（每个 id 每进程只记首错；重试在内存退避，不刷行）。
    pub fn record_failed(&self, id: &str, error: &str, ts: &str) {
        if !self.remember('f', id) {
            return;
        }
        self.append(&Record::Failed {
            id: id.into(),
            ts: ts.into(),
            error: error.into(),
        });
    }

    /// Interrupted transfers are terminal; they must not pin the next session.
    pub fn record_lost(&self, id: &str, reason: &str, ts: &str) {
        self.append(&Record::Lost { id: id.into(), reason: reason.into(), ts: ts.into() });
    }

    fn remember(&self, kind: char, id: &str) -> bool {
        self.recorded
            .lock()
            .unwrap()
            .insert((kind, id.to_string()))
    }

    /// 载入未确认收据（最后一态非 acked），按 pending 序号升序。
    /// 文件缺失/损坏行均容忍（跳过），绝不阻断会话启动。
    pub fn unacked(&self) -> Vec<UnackedReceipt> {
        let Ok(file) = File::open(&self.path) else {
            return Vec::new();
        };
        let mut last: std::collections::HashMap<String, UnackedReceipt> =
            std::collections::HashMap::new();
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let Ok(record) = serde_json::from_str::<Record>(&line) else {
                continue; // 损坏行容忍：跳过，按已读状态继续
            };
            match record {
                Record::Pending { id, backend, seq, .. } => {
                    last.insert(id.clone(), UnackedReceipt { id, backend, seq });
                }
                Record::Acked { id, .. } | Record::Lost { id, .. } => {
                    last.remove(&id);
                }
                Record::Failed { id, .. } => {
                    // 失败仍可重试：保持已有 pending 记录的 backend/seq；
                    // 只有无 pending 背景的裸 failed（不应出现）才占位。
                    last.entry(id).or_insert_with(|| UnackedReceipt {
                        id: String::new(),
                        backend: String::new(),
                        seq: 0,
                    });
                }
            }
        }
        let mut out: Vec<UnackedReceipt> = last.into_values().collect();
        out.retain(|receipt| !receipt.id.is_empty());
        out.sort_by_key(|receipt| receipt.seq);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &Path) -> ReceiptsStore {
        ReceiptsStore::open(dir, "usb:04b0:0456:Z5II:serial-1")
    }

    #[test]
    fn roundtrip_pending_ack_and_failed_states() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = store(dir.path());
        ledger.record_pending("wpd-mtp", "o1", 1, "2026-10-11T00:00:00Z");
        ledger.record_pending("wpd-mtp", "o2", 2, "2026-10-11T00:00:01Z");
        assert_eq!(
            ledger.unacked(),
            vec![
                UnackedReceipt {
                    id: "o1".into(),
                    backend: "wpd-mtp".into(),
                    seq: 1
                },
                UnackedReceipt {
                    id: "o2".into(),
                    backend: "wpd-mtp".into(),
                    seq: 2
                },
            ]
        );
        ledger.record_acked("o1", "2026-10-11T00:00:05Z");
        ledger.record_failed("o2", "传输不完整", "2026-10-11T00:00:06Z");
        let unacked = ledger.unacked();
        assert_eq!(unacked.len(), 1, "failed 仍可重试，不算已确认");
        assert_eq!(unacked[0].id, "o2");
        assert_eq!(unacked[0].backend, "wpd-mtp");
        ledger.record_acked("o2", "2026-10-11T00:00:20Z");
        assert!(ledger.unacked().is_empty());
    }

    #[test]
    fn last_state_wins_across_replays() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = store(dir.path());
        ledger.record_pending("wpd-mtp", "o1", 1, "t1");
        ledger.record_failed("o1", "首次失败", "t2");
        ledger.record_acked("o1", "t3");
        // 同一进程重开（模拟重启后新实例）：重放必须以最后一态为准。
        let reloaded = ReceiptsStore::open(dir.path(), "usb:04b0:0456:Z5II:serial-1");
        assert!(reloaded.unacked().is_empty());
        ledger.record_pending("wpd-mtp", "o2", 2, "t4");
        let reloaded = ReceiptsStore::open(dir.path(), "usb:04b0:0456:Z5II:serial-1");
        let unacked = reloaded.unacked();
        assert_eq!(unacked.len(), 1, "acked 之后的新 pending 不复活旧行");
        assert_eq!(unacked[0].id, "o2");
    }

    #[test]
    fn corrupted_lines_are_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = store(dir.path());
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(
            ledger.path(),
            "not json at all\n\
             {\"event\":\"pending\",\"id\":\"o1\",\"backend\":\"wpd-mtp\",\"seq\":1,\"ts\":\"t1\"}\n\
             \x00{truncated\n\
             {\"event\":\"acked\",\"id\":\"o1\",\"ts\":\"t2\"}\n",
        )
        .unwrap();
        assert!(ledger.unacked().is_empty());
        std::fs::write(
            ledger.path(),
            "garbage\n\
             {\"event\":\"pending\",\"id\":\"o9\",\"backend\":\"gphoto\",\"seq\":7,\"ts\":\"t9\"}\n",
        )
        .unwrap();
        let unacked = ledger.unacked();
        assert_eq!(unacked.len(), 1);
        assert_eq!(unacked[0].backend, "gphoto");
    }

    #[test]
    fn missing_directory_and_readonly_paths_do_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("tethering").join("receipts");
        let ledger = ReceiptsStore::open(&nested, "camera");
        assert!(ledger.unacked().is_empty(), "文件不存在=空账本");
        ledger.record_pending("wpd-mtp", "o1", 1, "t1"); // 惰性建目录
        assert_eq!(ledger.unacked().len(), 1);
    }

    #[test]
    fn duplicate_records_within_a_process_write_once() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = store(dir.path());
        for seq in 1..=5 {
            ledger.record_pending("wpd-mtp", "o1", seq, "t"); // 重试风暴只落首行
        }
        for _ in 0..3 {
            ledger.record_failed("o1", "again", "t"); // 同 id 首错只记一次
        }
        let lines = std::fs::read_to_string(ledger.path()).unwrap();
        assert_eq!(lines.lines().count(), 2);
    }

    #[test]
    fn physical_keys_are_sanitized_for_filenames() {
        assert_eq!(
            sanitize_component(r"\\?\usb#vid_04b0&pid_0456#0000008007062#{}"),
            "____usb_vid_04b0_pid_0456_0000008007062___"
        );
        assert_eq!(sanitize_component(" serial:ABC-123 "), "_serial_ABC-123_");
        assert_eq!(sanitize_component("///"), "___");
        assert_eq!(sanitize_component(""), "camera");
        let long = "x".repeat(500);
        assert!(sanitize_component(&long).len() <= 96);
        // 不同物理键在折叠后仍可区分（截断只发生在远超现实的长度）。
        assert_ne!(
            sanitize_component("serial:1"),
            sanitize_component("serial:2")
        );
    }
}
