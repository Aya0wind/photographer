//! 索引任务系统（M3.5，导入/索引任务分离，用户定案 2026-09-19）。
//!
//! 导入流水线只做复制/移动+校验+入册（快，不夹杂计算）；「已入册但索引
//! 不全」的资产由本模块的库级后台 worker 补齐——缩略图三档生成先行，
//! EXIF/AI 待办预留（kind 路由）。任务持久化在 `index_tasks`（见
//! db::migrations 0004），程序重启自动恢复：running 复位 pending → 派
//! worker 续跑。
//!
//! ## 通道设计（M4 AI 的接入点，v1 只落 CPU 通道）
//! - **CPU 通道**（v1）：thumb/exif 任务，worker 数 = 物理核心数全量
//!   （turbojpeg SIMD 缩放解码近线性扩展）；与按需缩略图队列共享
//!   `thumbs` 的全局 permit 池——主图兜底并发时天然互让，不超发。
//! - **GPU 通道**（预留，M4 AI 落地）：`ai` 类任务（CLIP 向量推理）在
//!   DirectML(ort) 初始化成功时必走 GPU（不是可选），失败自动落 CPU 并
//!   提示；`settings.ai.use_gpu` = 「允许使用 GPU」（默认 true，关掉强制
//!   纯 CPU）。GPU 推理与 CPU 缩略图互不抢核（kind 天然分工）。
//! - 调度：默认 AfterImport+立即（导入完成即刻全速跑完）；IdleOnly/Manual
//!   保留可选。`ai.cpu_limit_percent` 只约束未来 AI 推理，索引类不受限。
//! - 优先级：导入 > 主图缩略图兜底 > 索引批量（共享 permit 池天然实现）。

use std::path::{Path, PathBuf};

use rusqlite::params;

use crate::db::Db;
use crate::events::{AppEvent, EventBus};
use crate::thumbs::SIZE_TIERS;

/// 缩略图任务：生成全部低清档才算 done；任一档失败 → 资产
/// thumb_state=2（permanent-none，前端占位兜底）+ 任务按 attempts 策略重试。
/// RAW 只跑 256/512（内嵌 JPEG 源，快）；查看器高清走 raw-embed 直出档
/// （毫秒级 IO），2048 显影档改为查看器按需兜底——索引期不再为每张 RAW
/// 花 3-8s 去马赛克（33 张 RAW 的库索引被拖慢数分钟的根因）。
fn process_thumb_task(db: &Db, db_dir: &Path, asset_id: i64) -> bool {
    let Some((path, kind)) = asset_thumb_target(db, asset_id) else {
        // 资产已被删除（级联应清任务，防御性兜底）：按完成收尾
        return true;
    };
    if !matches!(kind.as_str(), "photo" | "raw") || !crate::thumbs::is_decodable(Path::new(&path)) {
        // 视频/其他/不可解码：永久占位（不占重试额度）
        let _ = db.set_thumb_state(asset_id, 2);
        return true;
    }
    let src = PathBuf::from(&path);
    let tiers: &[u16] = if kind == "raw" {
        &[256, 512]
    } else {
        SIZE_TIERS
    };
    for tier in tiers {
        if crate::thumbs::thumb_file(db_dir, &src, *tier).is_none() {
            let _ = db.set_thumb_state(asset_id, 2);
            return false;
        }
    }
    let _ = db.set_thumb_state(asset_id, 1);
    true
}

/// 任务目标资产的 (path, kind)。
fn asset_thumb_target(db: &Db, asset_id: i64) -> Option<(String, String)> {
    db.0.query_row(
        "SELECT path, kind FROM assets WHERE id = ?1",
        params![asset_id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )
    .ok()
}

/// 消费一条任务（认领 → 处理 → 收尾）。返回 false 表示队列已空。
fn step(db: &Db, db_dir: &Path) -> bool {
    let task = match db.claim_index_task("thumb") {
        Ok(Some(t)) => t,
        Ok(None) => return false,
        Err(_) => return false,
    };
    let ok = match task.kind.as_str() {
        "thumb" => process_thumb_task(db, db_dir, task.asset_id),
        _ => false,
    };
    let _ = db.finish_index_task(task.id, ok);
    true
}

/// 单 worker 循环：跑到队列空，返回处理数。
fn worker_loop(db_dir: &Path) -> u64 {
    let Ok(db) = crate::ipc::open_library_db(db_dir) else {
        return 0;
    };
    let mut count = 0u64;
    while step(&db, db_dir) {
        count += 1;
    }
    count
}

/// 跑完当前全部可处理任务（pending；failed 不自动重试），返回处理数。
/// worker 数 = 物理核心数全量；每 worker 独立 Db 连接（WAL + busy_timeout），
/// 认领经 SQLite 写锁串行化（UPDATE..RETURNING 单行独占，不重不漏）。
pub fn run_pending(db_dir: &Path, workers: usize) -> u64 {
    let workers = workers.max(1);
    let mut processed = 0u64;
    let handles: Vec<_> = (0..workers)
        .map(|n| {
            let db_dir = db_dir.to_path_buf();
            std::thread::Builder::new()
                .name(format!("index-worker-{n}"))
                .spawn(move || worker_loop(&db_dir))
                .expect("spawn index worker")
        })
        .collect();
    for h in handles {
        if let Ok(n) = h.join() {
            processed += n;
        }
    }
    processed
}

/// worker 数（物理核心数全量；测试断言用）。
pub fn worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

/// 导入完成后的钩子：派一个 supervisor 任务把当前待办全速跑完。
/// job 不等它（importSessionFinished 只代表文件入库）。
pub fn kick(db_dir: PathBuf, supervisor: &std::sync::Arc<crate::tasks::TaskSupervisor>) {
    let _ = supervisor.spawn_unique("index", "index-worker-pool".into(), move |_| {
        run_pending(&db_dir, worker_count());
    });
}

/// 启动恢复：复位遗留 running → pending，返回待办数（pending+running）。
pub fn resume_pending(db_dir: &Path) -> u64 {
    let Ok(db) = crate::ipc::open_library_db(db_dir) else {
        return 0;
    };
    let _ = db.reclaim_running_index_tasks();
    db.pending_index_count().unwrap_or(0)
}

/// 启动恢复钩子：复位遗留 running → pending → 事件（indexTaskResumed）
/// → 派 worker 续跑。无待办不发事件。
pub fn resume_and_kick(
    db_dir: PathBuf,
    bus: &EventBus,
    supervisor: &std::sync::Arc<crate::tasks::TaskSupervisor>,
) {
    let bus = bus.clone();
    supervisor.spawn("index", "index-resume".into(), move |_| {
        let pending = resume_pending(&db_dir);
        if pending > 0 {
            bus.publish(AppEvent::IndexTaskResumed { pending });
        }
        run_pending(&db_dir, worker_count());
    });
}

/// RAW 缩略图源代际自愈（开发期直改数据，不留兼容包袱）：v1（第一段
/// 小预览）→ v2（最大段）后存量 RAW 缩略图全部偏糊，重排 thumb 任务重建。
/// dbDir 标记文件防每次启动重排；旧档位缓存文件成为孤儿（开发期不管，
/// 必要时手删 thumbs 目录整体重建）。
pub fn refresh_raw_thumbs_for_generation(
    db_dir: PathBuf,
    bus: &EventBus,
    supervisor: &std::sync::Arc<crate::tasks::TaskSupervisor>,
) {
    let marker = db_dir.join(format!(
        "thumbs-raw-gen-{}.marker",
        crate::thumbs::RAW_THUMB_GENERATION
    ));
    if marker.is_file() {
        return;
    }
    let bus = bus.clone();
    supervisor.spawn("index", "raw-thumbs-regen".into(), move |_| {
        let pending = std::fs::create_dir_all(&db_dir)
            .ok()
            .and_then(|_| {
                crate::ipc::open_library_db(&db_dir)
                    .ok()
                    .and_then(|db| db.requeue_thumb_tasks_for_raw().ok())
            })
            .unwrap_or(0);
        let _ = std::fs::write(&db_dir.join(format!(
            "thumbs-raw-gen-{}.marker",
            crate::thumbs::RAW_THUMB_GENERATION
        )), b"");
        if pending > 0 {
            bus.publish(AppEvent::IndexTaskResumed { pending });
            run_pending(&db_dir, worker_count());
        }
    });
}
