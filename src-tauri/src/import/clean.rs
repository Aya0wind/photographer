//! M2 F1 安全清卡（spec §5.11）：导入任务已校验入册且源仍在设备上的文件，
//! 删前逐文件复验（重读源 size + xxh64 与 journal 指纹一致才删），每文件
//! 落日志，删除走 `DeviceSource::delete`（MTP 源即 WPD Delete）。
//!
//! 数据安全优先：复验不一致（卡上文件在导入后被改动/损坏）绝不删除，
//! 记入 errors 并计 failed；资产库本身不受清卡影响（删的是设备源文件）。

use std::collections::HashMap;
use std::io::Read;

use serde::{Deserialize, Serialize};
use xxhash_rust::xxh64::Xxh64;

use crate::db::{Db, JobFileRow};
use crate::devices::{DeviceError, DeviceSource};
use crate::events::{AppEvent, CleanStats, EventBus};

/// 清卡候选（IPC 契约，camelCase）：已校验入册且源仍在设备的文件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanCandidateDto {
    /// 设备内文件标识（与 journal src 一致）。
    pub src: String,
    /// 设备上的相对路径（展示用）。
    pub rel_path: String,
    pub size: u64,
    /// 库内资产 id（journal dst 按路径映射）。
    pub asset_id: i64,
}

/// 清卡结果（IPC 契约，camelCase）。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanResultDto {
    pub deleted: u64,
    pub failed: u64,
    pub freed_bytes: u64,
    pub errors: Vec<String>,
}

/// 列出可安全清理的候选：journal 中 state=verified、源仍在设备、
/// 且能映射到库内资产（dst → assets.id）的文件。
pub fn clean_candidates(
    db: &Db,
    source: &dyn DeviceSource,
    job_id: i64,
) -> Result<Vec<CleanCandidateDto>, String> {
    let rows = journal_verified_rows(db, job_id)?;
    let entries: HashMap<String, String> = source
        .list()
        .map_err(|e| format!("枚举设备文件失败: {e}"))?
        .into_iter()
        .map(|e| (e.id, e.rel_path))
        .collect();
    let mut out = Vec::new();
    for row in rows {
        let Some(rel_path) = entries.get(&row.src) else {
            continue; // 源已不在设备上
        };
        let Some(asset_id) = db
            .asset_id_by_path(&row.dst)
            .map_err(|e| format!("查询资产失败: {e}"))?
        else {
            continue; // 资产不在库（理论不可达：verified 必已入库）
        };
        out.push(CleanCandidateDto {
            src: row.src,
            rel_path: rel_path.clone(),
            size: row.size,
            asset_id,
        });
    }
    Ok(out)
}

/// 执行清卡：逐文件复验（重读源 size+xxh64 对比 journal 指纹）→ 一致才删。
/// 发布 cleanStarted/cleanFinished 事件；每文件写日志。
pub fn clean_apply(
    db: &Db,
    bus: &EventBus,
    source: &dyn DeviceSource,
    job_id: i64,
) -> Result<CleanResultDto, String> {
    let candidates = clean_candidates(db, source, job_id)?;
    let fingerprint: HashMap<String, JobFileRow> = db
        .all_job_files(job_id)
        .map_err(|e| format!("读取 journal 失败: {e}"))?
        .into_iter()
        .map(|row| (row.src.clone(), row))
        .collect();

    let count = candidates.len() as u64;
    let bytes = candidates.iter().map(|c| c.size).sum();
    let _ = db.append_log(
        "info",
        Some(job_id),
        &format!("清卡开始：{count} 个候选，共 {bytes} 字节"),
    );
    bus.publish(AppEvent::CleanStarted {
        job_id,
        count,
        bytes,
    });

    let mut result = CleanResultDto::default();
    for candidate in &candidates {
        let Some(row) = fingerprint.get(&candidate.src) else {
            continue; // 候选生成后 journal 行消失（理论不可达）
        };
        match reverify(source, &candidate.src, row) {
            Ok(()) => match source.delete(&candidate.src) {
                Ok(()) => {
                    result.deleted += 1;
                    result.freed_bytes += candidate.size;
                    let _ = db.append_log(
                        "info",
                        Some(job_id),
                        &format!(
                            "已删除（复验一致）: {}: {} 字节",
                            candidate.rel_path, candidate.size
                        ),
                    );
                }
                Err(err) => {
                    result.failed += 1;
                    let error = format!("删除失败 {}: {err}", candidate.rel_path);
                    let _ = db.append_log("error", Some(job_id), &error);
                    result.errors.push(error);
                }
            },
            Err(reason) => {
                // 复验不一致：绝不删除
                result.failed += 1;
                let error = format!("复验不一致，已跳过 {reason}");
                let _ = db.append_log("warn", Some(job_id), &error);
                result.errors.push(error);
            }
        }
    }

    let _ = db.append_log(
        "info",
        Some(job_id),
        &format!(
            "清卡结束：删除 {}，失败 {}，释放 {} 字节",
            result.deleted, result.failed, result.freed_bytes
        ),
    );
    let stats = CleanStats {
        deleted: result.deleted,
        failed: result.failed,
        freed_bytes: result.freed_bytes,
        errors: result.errors.clone(),
    };
    bus.publish(AppEvent::CleanFinished { job_id, stats });
    Ok(result)
}

/// journal 中 verified 的行（任务不存在报错）。
fn journal_verified_rows(db: &Db, job_id: i64) -> Result<Vec<JobFileRow>, String> {
    db.job_device(job_id)
        .map_err(|e| e.to_string())?
        .ok_or("任务不存在")?;
    Ok(db
        .all_job_files(job_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|row| row.state == crate::events::FileState::Verified)
        .collect())
}

/// 删前复验：重读源全流，size 与 xxh64 都必须与 journal 指纹一致。
/// journal 无指纹（理论不可达：verified 行必有 xxhash）按不一致处理。
fn reverify(source: &dyn DeviceSource, src: &str, row: &JobFileRow) -> Result<(), String> {
    let Some(expected_xxh) = row.xxhash else {
        return Err(format!("{src}: journal 缺少指纹"));
    };
    let mut reader = source.stream(src).map_err(stream_err)?;
    let mut xxh = Xxh64::new(0);
    let mut size: u64 = 0;
    let mut buf = vec![0u8; 8 * 1024 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                xxh.update(&buf[..n]);
                size += n as u64;
            }
            Err(e) => return Err(stream_err(DeviceError::Io(e))),
        }
    }
    let actual = xxh.digest();
    if size != row.size || actual != expected_xxh {
        return Err(format!(
            "{src}: 期望 (size={}, xxh={expected_xxh})，实得 (size={size}, xxh={actual})",
            row.size
        ));
    }
    Ok(())
}

fn stream_err(err: DeviceError) -> String {
    format!("重读源失败: {err}")
}
