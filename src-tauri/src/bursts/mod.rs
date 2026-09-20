//! 连拍分组引擎（M6，用户定案 2026-09-21）：双因子 = 时间链 × pHash 场景链，
//! 不做清晰度/最佳帧。
//!
//! ## 算法（单遍链式扫描）
//! 按 (camera, captured_at) 升序遍历「已算 pHash 的 photo/raw」：
//! 相邻对同时满足 ①captured_at 间隔 ≤ gap_ms ②pHash 汉明 ≤ hamming_max
//! ③当前组长 < 100 → 同链；任一不满足断链开新链。成员数 ≥ min_size 的
//! 链落 bursts + burst_id；不足的链不落组（burst_id NULL）。
//!
//! ## RAW+JPG 孪生
//! 配对资产**两边都算 pHash**（查重等场景要用），分组时跳过孪生：
//! pair_asset_id 非空且自己是 RAW 的不入链，JPG 代表入链（链上无重复帧）。
//!
//! ## 重组语义
//! 清空旧组整体重写（事务）：参数变更只重组、**不重算 pHash**（指纹机制
//! 的 burst= 路）。触发点：pHash 回填完成 / 导入完成 / 参数指纹变更。

use crate::db::Db;
use crate::events::{AppEvent, EventBus};

/// 分组参数（settings.ai.burst_* 的快照；小 Copy 便于穿线程）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BurstParams {
    pub gap_ms: u32,
    pub hamming_max: u8,
    pub min_size: u32,
}

impl BurstParams {
    pub fn from_settings(ai: &crate::settings::AiSettings) -> Self {
        Self {
            gap_ms: ai.burst_gap_ms,
            hamming_max: ai.burst_hamming_max,
            min_size: ai.burst_min_size,
        }
    }
}

/// 单链上限（防长曝光/机位不动的时间序列吞掉整个下午）。
const MAX_CHAIN: usize = 100;

/// 链上一帧（扫描行）。
struct Frame {
    asset_id: i64,
    phash: u64,
    captured_ms: Option<i64>,
    captured_text: Option<String>,
}

/// 重组全部连拍组（事务：清 bursts + burst_id 复位 → 重写）。
/// 返回 (组数, 入组资产数)。库内无 phash 行时为 no-op 清空。
pub fn regroup_bursts(db: &Db, params: &BurstParams) -> Result<(u64, u64), String> {
    // 扫描行：已算 pHash 的 photo/raw，去 RAW 孪生（JPG 代表）
    let rows = db.burst_scan_rows().map_err(|e| e.to_string())?;
    let mut frames: Vec<Frame> = Vec::with_capacity(rows.len());
    for (asset_id, phash, captured_at, kind, pair) in rows {
        if kind == "raw" && pair.is_some() {
            continue; // RAW 孪生跳链：JPG 代表入链
        }
        let captured_ms = captured_at
            .as_deref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|t| t.timestamp_millis());
        frames.push(Frame {
            asset_id,
            phash: phash as u64,
            captured_ms,
            captured_text: captured_at,
        });
    }

    // 单遍链式分组
    let mut chains: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for i in 0..frames.len() {
        if !current.is_empty() {
            let last = current[current.len() - 1];
            let linked = current.len() < MAX_CHAIN && link_ok(&frames[last], &frames[i], params);
            if !linked {
                chains.push(std::mem::take(&mut current));
            }
        }
        current.push(i);
    }
    if !current.is_empty() {
        chains.push(current);
    }

    // 落库（事务：清旧 + 写新）
    let groups: Vec<&Vec<usize>> = chains
        .iter()
        .filter(|c| c.len() as u32 >= params.min_size.max(1))
        .collect();
    let in_bursts: u64 = groups.iter().map(|c| c.len() as u64).sum();
    db.write_bursts(
        &groups
            .iter()
            .map(|c| {
                c.iter()
                    .map(|&i| (frames[i].asset_id, frames[i].captured_text.clone()))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|e| e.to_string())?;
    Ok((groups.len() as u64, in_bursts))
}

/// 相邻帧是否同链：时间链 + 场景链（captured 缺失视为断链——无法定序）。
fn link_ok(a: &Frame, b: &Frame, params: &BurstParams) -> bool {
    let (Some(ta), Some(tb)) = (a.captured_ms, b.captured_ms) else {
        return false;
    };
    let gap = tb.saturating_sub(ta).unsigned_abs();
    if gap > u64::from(params.gap_ms) {
        return false;
    }
    crate::metadata::phash::hamming(a.phash, b.phash) <= u32::from(params.hamming_max)
}

/// 重组后台入口（导入/回填/指纹触发）：开库 → 重组 → 事件回报。
pub fn regroup_kick(
    db_dir: std::path::PathBuf,
    params: BurstParams,
    bus: &EventBus,
    supervisor: &std::sync::Arc<crate::tasks::TaskSupervisor>,
) {
    let bus = bus.clone();
    supervisor.spawn("bursts", "regroup".into(), move |_| {
        let Ok(db) = crate::ipc::open_library_db(&db_dir) else {
            return;
        };
        match regroup_bursts(&db, &params) {
            Ok((groups, photos)) => {
                if groups > 0 {
                    bus.publish(AppEvent::BurstsRegrouped { groups, photos });
                }
            }
            Err(error) => eprintln!("[bursts] 重组失败: {error}"),
        }
    });
}
