//! One durable, asset-level dependency queue for all indexing capabilities.
//! Events reduce latency; pending rows and execution receipts survive restarts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};

use crate::ai::ModelManager;
use crate::db::{Db, IndexTaskRow};
use crate::events::{AppEvent, EventBus};
use crate::settings::AiSettings;
use crate::tasks::{TaskControls, TaskSupervisor};

static INFERENCE: Mutex<()> = Mutex::new(());

const KINDS: &[&str] = &["thumb", "exif", "phash", "blur", "eyes", "ai", "face"];

#[derive(Clone)]
struct Config {
    manager: Option<ModelManager>,
    bus: EventBus,
    ai: AiSettings,
}

struct Queue {
    config: Mutex<Config>,
    signal: Mutex<u64>,
    ready: Condvar,
    started: std::sync::atomic::AtomicBool,
    aggregates_dirty: std::sync::atomic::AtomicBool,
    reconciled: Mutex<std::time::Instant>,
}

fn queues() -> &'static Mutex<HashMap<PathBuf, Arc<Queue>>> {
    static QUEUES: OnceLock<Mutex<HashMap<PathBuf, Arc<Queue>>>> = OnceLock::new();
    QUEUES.get_or_init(Default::default)
}

fn queue(path: &Path) -> Arc<Queue> {
    queues()
        .lock()
        .expect("index queues poisoned")
        .entry(path.to_owned())
        .or_insert_with(|| {
            let mut ai = AiSettings::default();
            ai.enable_clip = false;
            ai.enable_face = false;
            Arc::new(Queue {
                config: Mutex::new(Config {
                    manager: None,
                    bus: EventBus::new(),
                    ai,
                }),
                signal: Mutex::new(0),
                ready: Condvar::new(),
                started: std::sync::atomic::AtomicBool::new(false),
                aggregates_dirty: std::sync::atomic::AtomicBool::new(true),
                reconciled: Mutex::new(std::time::Instant::now() - Duration::from_secs(1)),
            })
        })
        .clone()
}

pub fn is_running(path: &Path) -> bool {
    queue(path)
        .started
        .load(std::sync::atomic::Ordering::SeqCst)
}

pub fn notify(path: &Path) {
    let queue = queue(path);
    *queue.signal.lock().expect("index signal poisoned") += 1;
    queue.ready.notify_all();
}

pub fn models_changed() {
    let paths = queues()
        .lock()
        .expect("index queues poisoned")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for path in paths {
        notify(&path);
    }
}

fn enabled(kind: &str, ai: &AiSettings) -> bool {
    match kind {
        "ai" => ai.enable_clip,
        "face" => ai.enable_face,
        "eyes" | "blur" => ai.enable_clip || ai.enable_face,
        _ => true,
    }
}

/// Every entry point updates the same desired channels, without changing pauses.
pub fn configure(path: &Path, manager: &ModelManager, bus: &EventBus, ai: &AiSettings) {
    let queue = queue(path);
    let barrier = super::image_index_lock(path);
    let _guard = barrier
        .write()
        .expect("index configuration barrier poisoned");
    let snapshot = manager.snapshot();
    snapshot.set_ai_params(crate::ai::AiIndexParams {
        embed_input_size: ai.embed_input_size,
        face_detect_threshold: ai.face_detect_threshold,
        face_cluster_threshold: ai.face_cluster_threshold,
        use_gpu: ai.use_gpu,
        quality_tier: crate::ai::QualityTier::from_setting(&ai.quality_tier).unwrap_or_default(),
    });
    *queue.config.lock().expect("index config poisoned") = Config {
        manager: Some(snapshot),
        bus: bus.clone(),
        ai: ai.clone(),
    };
    if let Ok(db) = crate::ipc::open_library_db(path) {
        for kind in KINDS {
            let _ = db.0.execute(
                "UPDATE index_channels SET enabled=?2 WHERE kind=?1",
                params![kind, enabled(kind, ai)],
            );
        }
        if let Err(error) = reconcile(&db, &queue.config.lock().expect("index config poisoned")) {
            eprintln!("索引目标对账失败: {error}");
        }
    }
    notify(path);
}

/// Narrow legacy wakeups join the same queue; their algorithms no longer own pools.
pub fn request_ai(path: &Path, manager: &ModelManager, bus: &EventBus, kind: &str) {
    let mut config = queue(path)
        .config
        .lock()
        .expect("index config poisoned")
        .clone();
    match kind {
        "ai" => config.ai.enable_clip = true,
        "face" => config.ai.enable_face = true,
        _ => {}
    }
    configure(path, manager, bus, &config.ai);
}

fn settings_key(config: &Config, kind: &str) -> String {
    let version = match kind {
        "thumb" => "analysis-1024-v1",
        "exif" => "exif-v5",
        "phash" => "phash-v1",
        "blur" => crate::ai::selection::BLUR_ALGO_VERSION,
        "eyes" => crate::ai::selection::EYES_ALGO_VERSION,
        _ => "inference-v1",
    };
    let params = match kind {
        "eyes" | "blur" => crate::ipc::indexing::selection_params_fingerprint(&config.ai),
        "ai" => crate::ipc::indexing::semantic_params_fingerprint(&config.ai),
        "face" => crate::ipc::indexing::face_params_fingerprint(&config.ai),
        _ => 0,
    };
    format!("{version}:{params}")
}

fn reconcile(db: &Db, config: &Config) -> rusqlite::Result<()> {
    let now = crate::db::now_rfc3339();
    // Full-content hashes are no longer indexing targets or dedup prerequisites.
    db.0.execute(
        "DELETE FROM index_tasks WHERE kind='hash' AND state!='done'",
        [],
    )?;
    db.0.execute(
        "INSERT OR IGNORE INTO index_tasks(kind,asset_id,state,attempts,created_at,updated_at)
         SELECT c.kind,a.id,'pending',0,?1,?1 FROM assets a CROSS JOIN index_channels c
         WHERE c.enabled=1 AND a.kind IN ('photo','raw') AND a.in_trash=0",
        [&now],
    )?;
    for kind in KINDS {
        let key = settings_key(config, kind);
        // Only non-running owners can be invalidated. Configuration uses the write barrier.
        db.0.execute(
            "UPDATE index_tasks SET state='pending',attempts=0,updated_at=?2
             WHERE kind=?1 AND state!='running' AND id IN (
               SELECT t.id FROM index_tasks t JOIN assets a ON a.id=t.asset_id
               LEFT JOIN index_task_context x ON x.task_id=t.id
               WHERE x.task_id IS NULL OR x.settings_key!=?3 OR x.source_key!=a.path||':'||a.size||':'||a.mtime)",
            params![kind, now, key],
        )?;
        db.0.execute(
            "INSERT INTO index_task_context(task_id,source_key,settings_key)
             SELECT t.id,a.path||':'||a.size||':'||a.mtime,?2 FROM index_tasks t
             JOIN assets a ON a.id=t.asset_id WHERE t.kind=?1 AND t.state!='running'
             ON CONFLICT(task_id) DO UPDATE SET source_key=excluded.source_key,
               settings_key=excluded.settings_key,retry_at=CASE
               WHEN index_task_context.settings_key!=excluded.settings_key OR
                    index_task_context.source_key!=excluded.source_key THEN 0 ELSE index_task_context.retry_at END
             WHERE index_task_context.source_key!=excluded.source_key OR
                   index_task_context.settings_key!=excluded.settings_key",
            params![kind,key])?;
    }
    // A dependency failure is visible and finite; successful repair releases it.
    db.0.execute(
        "UPDATE index_task_context SET error='dependency failed' WHERE task_id IN (
        SELECT t.id FROM index_tasks t WHERE t.state='pending' AND EXISTS (
        SELECT 1 FROM index_dependencies d JOIN index_tasks p ON p.kind=d.prerequisite
        AND p.asset_id=t.asset_id WHERE d.kind=t.kind AND p.state='failed'))",
        [],
    )?;
    db.0.execute(
        "UPDATE index_tasks SET state='failed' WHERE state='pending' AND EXISTS (
        SELECT 1 FROM index_dependencies d JOIN index_tasks p ON p.kind=d.prerequisite
        AND p.asset_id=index_tasks.asset_id WHERE d.kind=index_tasks.kind AND p.state='failed')",
        [],
    )?;
    db.0.execute(
        "UPDATE index_tasks SET state='pending',attempts=0 WHERE state='failed'
        AND id IN (SELECT task_id FROM index_task_context WHERE error='dependency failed')
        AND NOT EXISTS (SELECT 1 FROM index_dependencies d LEFT JOIN index_tasks p
        ON p.kind=d.prerequisite AND p.asset_id=index_tasks.asset_id
        WHERE d.kind=index_tasks.kind AND (p.id IS NULL OR p.state!='done'))",
        [],
    )?;
    Ok(())
}

fn model_ready(config: &Config, kind: &str) -> bool {
    match kind {
        "ai" => config
            .manager
            .as_ref()
            .is_some_and(ModelManager::semantic_ready),
        "face" => config
            .manager
            .as_ref()
            .is_some_and(ModelManager::face_models_ready),
        "eyes" => config
            .manager
            .as_ref()
            .is_some_and(ModelManager::selection_eyes_ready),
        _ => true,
    }
}

fn claim(db: &Db, kind: &str, owner: &str) -> rusqlite::Result<Option<IndexTaskRow>> {
    let tx = Transaction::new_unchecked(&db.0, TransactionBehavior::Immediate)?;
    let id: Option<i64> = tx
        .query_row(
            "SELECT t.id FROM index_tasks t JOIN index_channels c ON c.kind=t.kind
         JOIN index_task_context x ON x.task_id=t.id JOIN assets a ON a.id=t.asset_id
         WHERE t.kind=?1 AND t.state='pending' AND c.enabled=1 AND c.paused=0
         AND x.retry_at<=unixepoch() AND a.missing=0 AND a.in_trash=0
         AND NOT EXISTS (SELECT 1 FROM index_dependencies d
           LEFT JOIN index_tasks p ON p.kind=d.prerequisite AND p.asset_id=t.asset_id
           WHERE d.kind=t.kind AND (p.id IS NULL OR p.state!='done'))
         ORDER BY t.updated_at,t.id LIMIT 1",
            [kind],
            |r| r.get(0),
        )
        .optional()?;
    let Some(id) = id else {
        return Ok(None);
    };
    tx.execute(
        "UPDATE index_tasks SET state='running',updated_at=?2 WHERE id=?1",
        params![id, crate::db::now_rfc3339()],
    )?;
    tx.execute(
        "UPDATE index_task_context SET owner=?2,error=NULL WHERE task_id=?1",
        params![id, owner],
    )?;
    let task = tx.query_row(
        "SELECT id,kind,asset_id,state,attempts,created_at,updated_at FROM index_tasks WHERE id=?1",
        [id],
        crate::db::map_index_task,
    )?;
    tx.commit()?;
    Ok(Some(task))
}

fn source_ready(db: &Db, asset: i64) -> bool {
    let source: Option<(String, i64, String)> =
        db.0.query_row(
            "SELECT path,size,mtime FROM assets WHERE id=?1",
            [asset],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .ok()
        .flatten();
    source.is_some_and(|(path, size, mtime)| {
        std::fs::metadata(path).is_ok_and(|m| {
            m.len() == size as u64
                && m.modified().ok().is_some_and(|time| {
                    chrono::DateTime::<chrono::Utc>::from(time)
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                        == mtime
                })
        })
    })
}

fn defer(db: &Db, id: i64, reason: &str) -> rusqlite::Result<()> {
    let tx = Transaction::new_unchecked(&db.0, TransactionBehavior::Immediate)?;
    tx.execute("UPDATE index_tasks SET state='pending' WHERE id=?1", [id])?;
    tx.execute(
        "UPDATE index_task_context SET owner=NULL,error=?2,retry_at=unixepoch()+5 WHERE task_id=?1",
        params![id, reason],
    )?;
    tx.commit()
}

fn execute(db: &Db, path: &Path, config: &Config, task: &IndexTaskRow) -> bool {
    match task.kind.as_str() {
        "thumb" => super::process_thumb_task(db, path, task.asset_id),
        "exif" => super::process_exif_task(db, task.asset_id),
        "phash" => super::process_phash_task(db, path, task.asset_id),
        "blur" => crate::ai::selection::process_blur_task(
            db,
            path,
            task.asset_id,
            f64::from(config.ai.blur_soft_threshold),
        ),
        "eyes" => config.manager.as_ref().is_some_and(|manager| {
            let classifier = crate::ai::selection::FacemeshEarClassifier::new(manager);
            crate::ai::selection::process_eyes_task(db, path, manager, &classifier, task.asset_id)
        }),
        "ai" => config.manager.as_ref().is_some_and(|manager| {
            crate::ai::semantic::process_ai_batch(db, path, manager, &[task.asset_id])[0].is_ok()
        }),
        "face" => config.manager.as_ref().is_some_and(|manager| {
            crate::ai::face::process_face_task(db, path, manager, task.asset_id)
        }),
        _ => false,
    }
}

fn finish(db: &Db, task: &IndexTaskRow, owner: &str, ok: bool) -> rusqlite::Result<()> {
    let tx = Transaction::new_unchecked(&db.0, TransactionBehavior::Immediate)?;
    let valid: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM index_task_context x
        JOIN index_tasks t ON t.id=x.task_id JOIN assets a ON a.id=t.asset_id
        WHERE t.id=?1 AND x.owner=?2 AND x.source_key=a.path||':'||a.size||':'||a.mtime)",
        params![task.id, owner],
        |r| r.get(0),
    )?;
    if valid {
        let attempts = if ok { 0 } else { task.attempts + 1 };
        let state = if ok {
            "done"
        } else if attempts >= 3 {
            "failed"
        } else {
            "pending"
        };
        tx.execute(
            "UPDATE index_tasks SET state=?2,attempts=?3,updated_at=?4 WHERE id=?1",
            params![task.id, state, attempts, crate::db::now_rfc3339()],
        )?;
        tx.execute(
            "UPDATE index_task_context SET owner=NULL,retry_at=?2,error=?3 WHERE task_id=?1",
            params![
                task.id,
                if ok {
                    0
                } else {
                    chrono::Utc::now().timestamp() + (1i64 << attempts.min(6))
                },
                if ok { None } else { Some("processing failed") }
            ],
        )?;
    } else {
        tx.execute(
            "UPDATE index_tasks SET state='pending',attempts=0 WHERE id=?1
             AND EXISTS(SELECT 1 FROM index_task_context WHERE task_id=?1 AND owner=?2)",
            params![task.id, owner],
        )?;
        tx.execute(
            "UPDATE index_task_context SET owner=NULL WHERE task_id=?1 AND owner=?2",
            params![task.id, owner],
        )?;
    }
    tx.commit()
}

fn worker(path: &Path, queue: &Queue, controls: &TaskControls, offset: usize) {
    let Ok(db) = crate::ipc::open_library_db(path) else {
        return;
    };
    let owner = uuid::Uuid::new_v4().to_string();
    struct Claims<'a>(&'a Db, &'a str);
    impl Drop for Claims<'_> {
        fn drop(&mut self) {
            let _ = self.0.0.execute("UPDATE index_tasks SET state='pending' WHERE state='running' AND id IN (SELECT task_id FROM index_task_context WHERE owner=?1)",[self.1]);
            let _ = self.0 .0.execute(
                "UPDATE index_task_context SET owner=NULL WHERE owner=?1",
                [self.1],
            );
        }
    }
    let _claims = Claims(&db, &owner);
    let mut turn = offset;
    loop {
        if controls.is_cancelled() || !path.join("library.db").is_file() {
            break;
        }
        if controls.is_paused() {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        if let Ok(mut reconciled) = queue.reconciled.try_lock() {
            if reconciled.elapsed() >= Duration::from_secs(1) {
                let barrier = super::image_index_lock(path);
                let _guard = barrier.read().expect("index reconciliation poisoned");
                let config = queue.config.lock().expect("index config poisoned").clone();
                if let Err(error) = reconcile(&db, &config) {
                    eprintln!("索引对账失败: {error}");
                }
                *reconciled = std::time::Instant::now();
            }
        }
        let barrier = super::image_index_lock(path);
        let mut permit =
            crate::tasks::index_budget::budget(crate::tasks::index_budget::Domain::Image)
                .acquire(1);
        let mut guard = barrier.read().expect("index barrier poisoned");
        let mut config = queue.config.lock().expect("index config poisoned").clone();
        let mut task = None;
        let mut inference = None;
        for n in 0..KINDS.len() {
            let kind = KINDS[(turn + n) % KINDS.len()];
            if enabled(kind, &config.ai) && model_ready(&config, kind) {
                if matches!(kind, "ai" | "face" | "eyes") {
                    match INFERENCE.try_lock() {
                        Ok(guard) => inference = Some(guard),
                        Err(_) => continue,
                    }
                }
                match claim(&db, kind, &owner) {
                    Ok(Some(found)) => {
                        turn = (turn + n + 1) % KINDS.len();
                        task = Some(found);
                        break;
                    }
                    Ok(None) => {
                        inference.take();
                    }
                    Err(error) => {
                        inference.take();
                        eprintln!("索引认领失败: {error}");
                        break;
                    }
                }
            }
        }
        if let Some(task) = task {
            if inference.is_some() {
                // Reserve the session's native CPU threads, including GPU fallback.
                // Release the source barrier before waiting for a larger permit.
                drop(guard);
                drop(permit);
                permit =
                    crate::tasks::index_budget::budget(crate::tasks::index_budget::Domain::Image)
                        .acquire(ModelManager::embed_intra_threads());
                guard = barrier.read().expect("index barrier poisoned");
                config = queue.config.lock().expect("index config poisoned").clone();
                let revision: String =
                    db.0.query_row(
                        "SELECT settings_key FROM index_task_context WHERE task_id=?1",
                        [task.id],
                        |r| r.get(0),
                    )
                    .unwrap_or_default();
                if revision != settings_key(&config, &task.kind)
                    || !model_ready(&config, &task.kind)
                {
                    let _ = defer(&db, task.id, "configuration changed");
                    continue;
                }
            }
            if matches!(task.kind.as_str(), "thumb" | "exif") && !source_ready(&db, task.asset_id) {
                let _ = defer(&db, task.id, "source unavailable or writing");
                continue;
            }
            if matches!(task.kind.as_str(), "blur" | "eyes" | "face" | "ai")
                && super::artifacts::analysis(&db, path, task.asset_id).is_none()
            {
                // The producer owns rebuilding lost/corrupt analysis inputs.
                // Bound repairs with the consumer's attempts; broken caches cannot loop forever.
                if task.attempts < 2 {
                    if let Ok((source, mtime)) = db.0.query_row(
                        "SELECT path,mtime FROM assets WHERE id=?1",
                        [task.asset_id],
                        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                    ) {
                        if let Some(file) =
                            crate::thumbs::cached_for_asset(path, Path::new(&source), 1024, &mtime)
                        {
                            let _ = std::fs::remove_file(file);
                        }
                    }
                    let _ = db.0.execute("UPDATE index_tasks SET state='pending',attempts=0 WHERE kind='thumb' AND asset_id=?1 AND state!='running'",[task.asset_id]);
                    let _ = db.0.execute("UPDATE index_task_context SET retry_at=0 WHERE task_id IN (SELECT id FROM index_tasks WHERE kind='thumb' AND asset_id=?1)",[task.asset_id]);
                }
                let _ = finish(&db, &task, &owner, false);
                notify(path);
                continue;
            }
            let mut batch = vec![task];
            if batch[0].kind == "ai"
                && config.manager.as_ref().is_some_and(|manager| {
                    crate::ai::prefer_batch_inference(
                        manager.ai_params().use_gpu,
                        crate::ai::semantic_model_ids(manager.ai_params().quality_tier)[0],
                    )
                })
            {
                let pixels = usize::from(config.ai.embed_input_size).pow(2) * 3 * 4;
                let cap =
                    (32 * 1024 * 1024 / pixels.max(1)).clamp(1, crate::ai::semantic::AI_BATCH);
                while batch.len() < cap {
                    match claim(&db, "ai", &owner) {
                        Ok(Some(task)) => batch.push(task),
                        _ => break,
                    }
                }
            }
            let results = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::ai::selection::with_parameters(&config.ai, || {
                    if batch[0].kind == "ai" {
                        if let Some(manager) = &config.manager {
                            return crate::ai::semantic::process_ai_batch(
                                &db,
                                path,
                                manager,
                                &batch.iter().map(|t| t.asset_id).collect::<Vec<_>>(),
                            )
                            .into_iter()
                            .map(|r| r.is_ok())
                            .collect();
                        }
                    }
                    batch
                        .iter()
                        .map(|task| execute(&db, path, &config, task))
                        .collect::<Vec<bool>>()
                })
            }))
            .unwrap_or_else(|_| vec![false; batch.len()]);
            let completed = results.iter().filter(|ok| **ok).count() as u64;
            for (task, ok) in batch.iter().zip(results) {
                if let Err(error) = finish(&db, task, &owner, ok) {
                    eprintln!("索引结算失败（保留 running 供恢复）: {error}");
                    return;
                }
                if ok && matches!(task.kind.as_str(), "exif" | "phash") {
                    queue
                        .aggregates_dirty
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
                config.bus.publish(AppEvent::IndexTaskProgress {
                    kind: task.kind.clone(),
                    done: db
                        .0
                        .query_row(
                            "SELECT COUNT(*) FROM index_tasks WHERE kind=?1 AND state='done'",
                            [&task.kind],
                            |r| r.get(0),
                        )
                        .unwrap_or(0),
                    total: db
                        .0
                        .query_row(
                            "SELECT COUNT(*) FROM index_tasks WHERE kind=?1",
                            [&task.kind],
                            |r| r.get(0),
                        )
                        .unwrap_or(0),
                });
            }
            if batch[0].kind == "ai" {
                config
                    .bus
                    .publish(AppEvent::AiIndexFinished { done: completed });
            }
            drop(guard);
            notify(path);
            continue;
        }
        if queue
            .aggregates_dirty
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            let waiting:bool=db.0.query_row("SELECT EXISTS(SELECT 1 FROM index_tasks t JOIN index_channels c ON c.kind=t.kind JOIN assets a ON a.id=t.asset_id WHERE t.kind IN ('exif','phash') AND t.state IN ('pending','running') AND c.enabled=1 AND c.paused=0 AND a.missing=0 AND a.in_trash=0)",[],|r|r.get(0)).unwrap_or(true);
            if !waiting
                && queue
                    .aggregates_dirty
                    .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                match crate::bursts::regroup_bursts(
                    &db,
                    &crate::bursts::BurstParams::from_settings(&config.ai),
                ) {
                    Ok((groups, photos)) => config
                        .bus
                        .publish(AppEvent::BurstsRegrouped { groups, photos }),
                    Err(error) => {
                        queue
                            .aggregates_dirty
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                        eprintln!("连拍重组失败: {error}");
                    }
                }
            }
        }
        drop(guard);
        drop(permit);
        let signal = queue.signal.lock().expect("index signal poisoned");
        let _ = queue
            .ready
            .wait_timeout(signal, Duration::from_secs(1))
            .expect("index wait poisoned");
    }
}

pub fn kick(path: PathBuf, supervisor: &Arc<TaskSupervisor>) {
    let queue = queue(&path);
    notify(&path);
    if queue
        .started
        .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        return;
    }
    // This is a process-start recovery, never a database-switch recovery.
    if let Ok(db) = crate::ipc::open_library_db(&path) {
        let _ = db.reclaim_running_index_tasks();
        let config = queue.config.lock().expect("index config poisoned").clone();
        let _ = reconcile(&db, &config);
    }
    supervisor.spawn(
        "index",
        format!("index-scheduler:{}", path.display()),
        move |controls| {
            struct Running<'a>(&'a Queue);
            impl Drop for Running<'_> {
                fn drop(&mut self) {
                    self.0
                        .started
                        .store(false, std::sync::atomic::Ordering::SeqCst);
                }
            }
            let _running = Running(&queue);
            std::thread::scope(|scope| {
                for n in 0..crate::tasks::index_parallelism() {
                    let path = &path;
                    let queue = &queue;
                    let controls = &controls;
                    scope.spawn(move || worker(path, queue, controls, n));
                }
            });
        },
    );
}

pub fn pause(path: &Path, selection: bool, paused: bool) -> Result<(), String> {
    let db = crate::ipc::open_library_db(path)?;
    db.0.execute(
        "UPDATE index_channels SET paused=?1 WHERE
        (kind IN ('eyes','blur'))=?2",
        params![paused, selection],
    )
    .map_err(|e| e.to_string())?;
    notify(path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(count: i64) -> (tempfile::TempDir, Db, Config) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("library.db")).unwrap();
        let config = Config {
            manager: None,
            bus: EventBus::new(),
            ai: AiSettings::default(),
        };
        db.0.execute("UPDATE index_channels SET enabled=1 WHERE kind='blur'", [])
            .unwrap();
        for id in 1..=count {
            db.0.execute("INSERT INTO assets(id,path,filename,size,mtime,xxhash,kind,source,created_at) VALUES (?1,?2,?2,1,'2026-01-01T00:00:00.000Z',0,'photo','scan','now')",params![id,format!("{id}.jpg")]).unwrap();
        }
        reconcile(&db, &config).unwrap();
        (dir, db, config)
    }

    #[test]
    fn ten_ready_photos_release_consumers_without_waiting_for_fifty() {
        let (_dir, db, _config) = fixture(50);
        assert!(claim(&db, "blur", "consumer").unwrap().is_none());
        for _ in 0..10 {
            let task = claim(&db, "thumb", "producer").unwrap().unwrap();
            finish(&db, &task, "producer", true).unwrap();
        }
        for _ in 0..10 {
            let task = claim(&db, "blur", "consumer").unwrap().unwrap();
            finish(&db, &task, "consumer", true).unwrap();
        }
        assert!(claim(&db, "blur", "consumer").unwrap().is_none());
        assert_eq!(db.pending_index_task_count("thumb").unwrap(), 40);
    }

    #[test]
    fn stale_owner_cannot_settle_a_reclaimed_task() {
        let (_dir, db, config) = fixture(1);
        let old = claim(&db, "thumb", "old").unwrap().unwrap();
        assert_eq!(db.reclaim_running_index_tasks().unwrap(), 1);
        reconcile(&db, &config).unwrap();
        let fresh = claim(&db, "thumb", "new").unwrap().unwrap();
        finish(&db, &old, "old", true).unwrap();
        let state: String =
            db.0.query_row(
                "SELECT state FROM index_tasks WHERE id=?1",
                [fresh.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "running");
        finish(&db, &fresh, "new", true).unwrap();
        assert_eq!(db.pending_index_task_count("thumb").unwrap(), 0);
    }

    #[test]
    fn dependency_failure_is_finite_and_repair_releases_it() {
        let (_dir, db, config) = fixture(1);
        let producer = claim(&db, "thumb", "worker").unwrap().unwrap();
        db.0.execute(
            "UPDATE index_tasks SET state='failed' WHERE id=?1",
            [producer.id],
        )
        .unwrap();
        reconcile(&db, &config).unwrap();
        let state: String =
            db.0.query_row("SELECT state FROM index_tasks WHERE kind='blur'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(state, "failed");
        db.retry_failed_index_tasks("thumb").unwrap();
        let repair = claim(&db, "thumb", "worker").unwrap().unwrap();
        finish(&db, &repair, "worker", true).unwrap();
        reconcile(&db, &config).unwrap();
        assert!(claim(&db, "blur", "consumer").unwrap().is_some());
    }

    #[test]
    fn pause_and_offline_sources_survive_reconciliation() {
        let (_dir, db, config) = fixture(2);
        db.0.execute("UPDATE index_channels SET paused=1 WHERE kind='thumb'", [])
            .unwrap();
        reconcile(&db, &config).unwrap();
        assert!(claim(&db, "thumb", "worker").unwrap().is_none());
        db.0.execute("UPDATE index_channels SET paused=0 WHERE kind='thumb'", [])
            .unwrap();
        db.0.execute("UPDATE assets SET missing=1 WHERE id=1", [])
            .unwrap();
        let task = claim(&db, "thumb", "worker").unwrap().unwrap();
        assert_eq!(task.asset_id, 2);
        assert!(!source_ready(&db, task.asset_id));
        defer(&db, task.id, "offline").unwrap();
        assert!(claim(&db, "thumb", "worker").unwrap().is_none());
    }

    #[test]
    fn source_or_settings_change_invalidates_completed_consumers() {
        let (_dir, db, mut config) = fixture(1);
        let thumb = claim(&db, "thumb", "producer").unwrap().unwrap();
        finish(&db, &thumb, "producer", true).unwrap();
        let blur = claim(&db, "blur", "consumer").unwrap().unwrap();
        finish(&db, &blur, "consumer", true).unwrap();
        config.ai.blur_soft_threshold += 1.0;
        reconcile(&db, &config).unwrap();
        assert!(claim(&db, "blur", "consumer").unwrap().is_some());
        db.reclaim_running_index_tasks().unwrap();
        db.0.execute("UPDATE assets SET size=2 WHERE id=1", [])
            .unwrap();
        reconcile(&db, &config).unwrap();
        assert!(claim(&db, "blur", "consumer").unwrap().is_none());
        assert!(claim(&db, "thumb", "producer").unwrap().is_some());
    }

    #[test]
    fn concurrent_connections_never_claim_the_same_photo() {
        let (dir, db, _config) = fixture(50);
        let file = dir.path().join("library.db");
        let claimed = Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for n in 0..8 {
                let file = &file;
                let claimed = &claimed;
                scope.spawn(move || {
                    let db = Db::open(file).unwrap();
                    let owner = format!("worker-{n}");
                    while let Some(task) = claim(&db, "thumb", &owner).unwrap() {
                        claimed.lock().unwrap().push(task.asset_id);
                        finish(&db, &task, &owner, true).unwrap();
                    }
                });
            }
        });
        let mut ids = claimed.into_inner().unwrap();
        ids.sort_unstable();
        assert_eq!(ids, (1..=50).collect::<Vec<_>>());
        assert_eq!(db.pending_index_task_count("thumb").unwrap(), 0);
    }
}
