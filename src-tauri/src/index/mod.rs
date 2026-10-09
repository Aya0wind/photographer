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
use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::params;

use crate::db::Db;
use crate::events::{AppEvent, EventBus};
use crate::thumbs::SIZE_TIERS;

/// Rebuild excludes active image-task writes. Workers keep their existing
/// pause/resume controls and continue against the newly queued tasks afterwards.
pub fn image_index_lock(db_dir: &Path) -> std::sync::Arc<std::sync::RwLock<()>> {
    static LOCKS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, std::sync::Arc<std::sync::RwLock<()>>>>,
    > = std::sync::OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(Default::default)
        .lock()
        .expect("image index locks poisoned");
    locks.entry(db_dir.to_owned()).or_default().clone()
}

/// 缩略图任务：生成全部低清档才算 done；档位失败时区分两种归因——
/// **源缺失**（第三方移动/删除）是暂态：不落 thumb_state、保持 0，按
/// attempts 策略重试，文件移回后自然完成（2026-09-28 边界修复：此前
/// 缺失也置 2，真机实证文件移回后永久 unavailable 不自愈）；**文件在盘
/// 但出不了图**（解码失败）才置 2 永久占位。RAW 只跑 256/512（内嵌
/// JPEG 源，快）；查看器高清走 raw-embed 直出档（毫秒级 IO），2048
/// 显影档改为查看器按需兜底——索引期不再为每张 RAW 花 3-8s 去马赛克
/// （33 张 RAW 的库索引被拖慢数分钟的根因）。
fn process_thumb_task(db: &Db, db_dir: &Path, asset_id: i64) -> bool {
    let Some((path, kind)) = asset_thumb_target(db, asset_id) else {
        // 资产已被删除（级联应清任务，防御性兜底）：按完成收尾
        return true;
    };
    if !matches!(kind.as_str(), "photo" | "raw") || !crate::thumbs::is_decodable(Path::new(&path)) {
        // 其他类型/不可解码：永久占位（不占重试额度）。
        let _ = db.set_thumb_state(asset_id, 2);
        return true;
    }
    let src = PathBuf::from(&path);
    let tiers: &[u16] = if kind == "raw" {
        &[256, 512]
    } else {
        SIZE_TIERS
    };
    // 缺失是暂态（文件可能移回）：任务失败重试即可，不毒化 thumb_state
    let src_missing = !src.exists();
    for tier in tiers {
        if crate::thumbs::thumb_file(db_dir, &src, *tier).is_none() {
            if !src_missing {
                // 文件在盘仍出不了图（解码失败）：真永久占位
                let _ = db.set_thumb_state(asset_id, 2);
            }
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
/// 认领优先级：exif（量小且解锁筛选 UI）→ hash（毫秒级，解锁精确查重
/// 层）→ phash → thumb（数量大头）。每 worker 一次认领一条，认领经
/// SQLite 写锁串行化，不重不漏。
fn step(db: &Db, db_dir: &Path) -> bool {
    let _permit =
        crate::tasks::index_budget::budget(crate::tasks::index_budget::Domain::Image).acquire(1);
    let barrier = image_index_lock(db_dir);
    let _guard = barrier.read().expect("image index barrier poisoned");
    let task = ["exif", "thumb", "hash", "phash", "blur"]
        .iter()
        .find_map(|kind| db.claim_index_task(kind).ok().flatten())
        .or_else(|| db.claim_index_task("thumb").ok().flatten());
    let Some(task) = task else {
        return false;
    };
    let ok = match task.kind.as_str() {
        "thumb" => process_thumb_task(db, db_dir, task.asset_id),
        "exif" => process_exif_task(db, task.asset_id),
        "phash" => process_phash_task(db, db_dir, task.asset_id),
        "hash" => process_hash_task(db, task.asset_id),
        // Selection source-v2: high-resolution regional diagnostics; no focus
        // verdict from an uncalibrated Laplacian/background score.
        "blur" => crate::ai::selection::process_blur_task(
            db,
            db_dir,
            task.asset_id,
            crate::ai::selection::blur_soft_threshold(),
        ),
        // eyes 通道由 ai::selection::run_eyes_backfill 并发消费（依赖
        // SCRFD + 闭眼分类器，index worker 无模型会话，不在此认领）
        _ => false,
    };
    let _ = db.finish_index_task(task.id, ok);
    true
}

/// pHash 任务（M6 连拍分组前置）：256 档缩略图 → 灰度 → DCT → 64-bit 落库。
/// 文件消失按完成收尾（不占重试额度）；解码失败走 attempts 封顶。
fn process_phash_task(db: &Db, db_dir: &Path, asset_id: i64) -> bool {
    let Some((path, kind)) = asset_thumb_target(db, asset_id) else {
        return true; // 资产已删除：防御兜底
    };
    if !matches!(kind.as_str(), "photo" | "raw") {
        return true;
    }
    let Some(phash) = crate::metadata::phash::compute_phash(db_dir, Path::new(&path)) else {
        return false;
    };
    if db.set_phash(asset_id, phash).is_err() {
        return false;
    }
    // 近重复分桶增量维护（F8：4×16-bit 多探针）
    db.insert_similar_buckets(asset_id, phash).is_ok()
}

/// EXIF 深提取任务（gen-3）：读文件头 ≤1MB → 全量字段落库（深字段 +
/// lens/0004 拍摄参数——老库这些列从未有人写，真机 2026-09-20 发现）。
/// 文件消失（外部库被移走/导入源清理）按完成收尾不占重试额度；
/// 解码失败走 attempts 封顶策略（与 thumb 一致）。
fn process_exif_task(db: &Db, asset_id: i64) -> bool {
    let Some((path, kind)) = asset_thumb_target(db, asset_id) else {
        return true; // 资产已删除：级联应清任务，防御兜底
    };
    if !matches!(kind.as_str(), "photo" | "raw") {
        return true; // 非图片无 EXIF 深提取可言
    }
    let Ok(head) = read_head(&path) else {
        return true; // 文件不可读（外部库被移走等）：不再重试
    };
    let meta = crate::metadata::exif_lite::parse(&head);
    let deep_ok = db.update_asset_deep_exif(asset_id, &meta).is_ok();
    // LR 存量评分/颜色标签回填：边车 xmp:Rating / xmp:Label → DB（只读边车
    // 不回写，与应用内写入方向相反，无循环；DB 已有值不覆盖——应用内值优先）
    let sidecar_text =
        std::fs::read_to_string(crate::metadata::xmp::sidecar_path(Path::new(&path)))
            .unwrap_or_default();
    if let Some(stars) = crate::metadata::xmp::sidecar_rating(&sidecar_text) {
        if stars > 0
            && db
                .asset_rating_of(asset_id)
                .ok()
                .flatten()
                .is_none_or(|r| r == 0)
        {
            let _ = db.set_asset_rating(asset_id, i64::from(stars));
        }
    }
    if let Some(label) = crate::metadata::xmp::sidecar_label(&sidecar_text) {
        let unset: Option<String> =
            db.0.query_row(
                "SELECT color_label FROM assets WHERE id = ?1",
                params![asset_id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        if unset.is_none() {
            let _ = db.0.execute(
                "UPDATE assets SET color_label = ?2 WHERE id = ?1",
                params![asset_id, label],
            );
        }
    }
    deep_ok
}

/// 读文件头（≤1MB，与导入管线 HEAD_MAX 同口径）。
fn read_head(path: &str) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut head = Vec::with_capacity(256 * 1024);
    file.by_ref().take(1024 * 1024).read_to_end(&mut head)?;
    Ok(head)
}

/// 哈希补算任务（M8-②）：rename 快道/历史遗留的 xxhash=0 哨兵 → 全文件
/// 流式 xxh64 补齐（精确查重层 (size, xxhash) 随之就位）。文件消失按完成
/// 收尾；读失败走 attempts 封顶。
fn process_hash_task(db: &Db, asset_id: i64) -> bool {
    let Some((path, xxhash)) =
        db.0.query_row(
            "SELECT path, xxhash FROM assets WHERE id = ?1",
            [asset_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
        )
        .ok()
    else {
        return true; // 资产已删除：防御兜底
    };
    if xxhash != 0 {
        return true; // 已补算（幂等）
    }
    let Ok(mut file) = std::fs::File::open(&path) else {
        return true; // 文件不可读（外部库被移走等）：不再重试
    };
    use std::io::Read;
    let mut hasher = xxhash_rust::xxh64::Xxh64::new(0);
    let mut chunk = vec![0u8; 8 * 1024 * 1024];
    loop {
        match file.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => hasher.update(&chunk[..n]),
            Err(_) => return false,
        }
    }
    db.set_xxhash(asset_id, hasher.digest()).is_ok()
}

/// 单 worker 循环：跑到队列空，返回处理数。暂停旗（导入让路闸）置位时
/// 步进间挂起等待，恢复后续跑；取消旗直接退出。
fn worker_loop(db_dir: &Path, paused: &AtomicBool, cancelled: &AtomicBool) -> u64 {
    let Ok(db) = crate::ipc::open_library_db(db_dir) else {
        return 0;
    };
    let mut count = 0u64;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        if paused.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(100));
            continue;
        }
        if !step(&db, db_dir) {
            break;
        }
        count += 1;
    }
    count
}

/// 跑完当前全部可处理任务（pending；failed 不自动重试），返回处理数。
/// worker 数 = 物理核心数全量；每 worker 独立 Db 连接（WAL + busy_timeout），
/// 认领经 SQLite 写锁串行化（UPDATE..RETURNING 单行独占，不重不漏）。
pub fn run_pending(db_dir: &Path, workers: usize) -> u64 {
    run_pending_gated(
        db_dir,
        workers,
        std::sync::Arc::new(AtomicBool::new(false)),
        std::sync::Arc::new(AtomicBool::new(false)),
    )
}

/// 带暂停/取消旗的 [`run_pending`]（supervisor 任务体接线用）。
pub fn run_pending_gated(
    db_dir: &Path,
    workers: usize,
    paused: std::sync::Arc<AtomicBool>,
    cancelled: std::sync::Arc<AtomicBool>,
) -> u64 {
    let workers = workers.max(1);
    let mut processed = 0u64;
    let handles: Vec<_> = (0..workers)
        .map(|n| {
            let db_dir = db_dir.to_path_buf();
            let paused = std::sync::Arc::clone(&paused);
            let cancelled = std::sync::Arc::clone(&cancelled);
            std::thread::Builder::new()
                .name(format!("index-worker-{n}"))
                .spawn(move || worker_loop(&db_dir, &paused, &cancelled))
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
    crate::tasks::index_parallelism()
}

/// 导入完成后的钩子：派一个 supervisor 任务把当前待办全速跑完。
/// job 不等它（importSessionFinished 只代表文件入库）。任务体接线暂停/
/// 取消旗（导入开始时 supervisor.pause_kind("index") 让路）。
pub fn kick(db_dir: PathBuf, supervisor: &std::sync::Arc<crate::tasks::TaskSupervisor>) {
    let _ = supervisor.spawn_coalesced(
        "index",
        format!("index-worker-pool:{}", db_dir.display()),
        move |controls| {
            run_pending_gated(
                &db_dir,
                worker_count(),
                controls.paused_flag(),
                controls.cancelled_flag(),
            );
        },
    );
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

// 代际回填共用入队、标记和消费流程；各入口保留代际名与后续业务动作。
fn run_generation_refresh(
    db_dir: &Path,
    bus: &EventBus,
    marker: &Path,
    requeue: impl FnOnce(&Db) -> rusqlite::Result<u64>,
) {
    let pending = std::fs::create_dir_all(db_dir)
        .ok()
        .and_then(|_| crate::ipc::open_library_db(db_dir).ok())
        .and_then(|db| requeue(&db).ok())
        .unwrap_or(0);
    let _ = std::fs::write(marker, b"");
    if pending > 0 {
        bus.publish(AppEvent::IndexTaskResumed { pending });
        run_pending(db_dir, worker_count());
    }
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
        run_generation_refresh(&db_dir, &bus, &marker, |db| {
            db.requeue_thumb_tasks_for_raw()
        });
    });
}

/// pHash 代际自愈（gen-1，migration 0012 配套）：存量资产补算 pHash，
/// 完成后连拍重组（同链触发：导入完成 / 参数指纹变更）。标记防每启动重排。
pub fn refresh_phash_for_generation(
    db_dir: PathBuf,
    bus: &EventBus,
    supervisor: &std::sync::Arc<crate::tasks::TaskSupervisor>,
    params: crate::bursts::BurstParams,
) {
    let marker = db_dir.join("phash-gen-1.marker");
    if marker.is_file() {
        return;
    }
    let bus = bus.clone();
    let supervisor = std::sync::Arc::clone(supervisor);
    let spawn_handle = std::sync::Arc::clone(&supervisor);
    spawn_handle.spawn("index", "phash-gen1-regen".into(), move |_| {
        let supervisor = std::sync::Arc::clone(&supervisor);
        run_generation_refresh(&db_dir, &bus, &marker, |db| {
            db.requeue_phash_tasks_for_all()
        });
        crate::bursts::regroup_kick(db_dir, params, &bus, &supervisor);
    });
}

/// 哈希补算代际自愈（gen-1，migration 0014 配套）：xxhash=0 哨兵的存量
/// 资产（rename 快道遗留/历史）一次性补算。标记防每启动重排。
pub fn refresh_hash_for_generation(
    db_dir: PathBuf,
    bus: &EventBus,
    supervisor: &std::sync::Arc<crate::tasks::TaskSupervisor>,
) {
    let marker = db_dir.join("hash-gen-1.marker");
    if marker.is_file() {
        return;
    }
    let bus = bus.clone();
    supervisor.spawn("index", "hash-gen1-regen".into(), move |_| {
        run_generation_refresh(&db_dir, &bus, &marker, |db| db.requeue_hash_tasks_for_all());
    });
}

/// 选片分析代际自愈（gen-1，0021 配套）：eyes/blur 任务账一次性建档
/// （新导入经 insert_asset_on 自动登记；存量资产由此补种）。dbDir 标记
/// 防每次启动重排；指纹变更重建走 ipc::indexing（selection 通道）。
pub fn refresh_selection_for_generation(
    db_dir: PathBuf,
    bus: &EventBus,
    supervisor: &std::sync::Arc<crate::tasks::TaskSupervisor>,
) {
    let marker = db_dir.join("selection-gen-1.marker");
    if marker.is_file() {
        return;
    }
    let bus = bus.clone();
    supervisor.spawn("index", "selection-gen1-regen".into(), move |_| {
        run_generation_refresh(&db_dir, &bus, &marker, |db| {
            let eyes = db.create_eyes_tasks_for_unindexed()?;
            let blur = db.create_blur_tasks_for_unindexed()?;
            Ok(eyes + blur)
        });
    });
}

/// EXIF 深提取代际自愈（gen-2，migration 0008 配套；gen-5 宽高分层修复
/// 2026-09-21）：0008 新增的 10 列存量资产全空，启动时一次性重排全部
/// photo/raw 的 exif 任务回填（导入管线只对新导入生效）。gen-5 一并重排
/// ——尼康 NEF 存量宽高是内嵌缩略图尺寸（640×424），新策略从 SubIFD
/// 主图 IFD 取本体尺寸（6064×4040），COALESCE 覆盖旧错值。
/// dbDir 标记 `exif-gen-5.marker` 防每次启动重排；新导入资产不走此链
/// （入库时已带深提取字段）。
pub fn refresh_exif_for_generation(
    db_dir: PathBuf,
    bus: &EventBus,
    supervisor: &std::sync::Arc<crate::tasks::TaskSupervisor>,
) {
    let marker = db_dir.join("exif-gen-5.marker");
    if marker.is_file() {
        return;
    }
    let bus = bus.clone();
    supervisor.spawn("index", "exif-gen5-regen".into(), move |_| {
        run_generation_refresh(&db_dir, &bus, &marker, |db| db.requeue_exif_tasks_for_all());
    });
}
