//! 地理数据包下载（supervisor 后台任务，UI 零阻塞）：
//! 1. 世界两包（Natural Earth 50m admin0/admin1，GitHub raw）
//! 2. 中国包（DataV 递归：100000 省级 full → 逐省 → 逐市，~375 个小文件）
//! 完成后自动接力：树加载 → regions 入库 → 回填（见 backfill::ensure_backfill）。
//! 进度经 `MapGeoProgress { stage: "download", done, total }` 广播。

use std::io::Read;
use std::sync::atomic::Ordering;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::events::{AppEvent, EventBus};
use crate::tasks::TaskSupervisor;

use super::{
    geo_state, packages_installed, GeoPhase, DATAV_BASE, DATAV_CHINA_CODE, PACKAGE_WORLD_ADM0,
    PACKAGE_WORLD_ADM1,
};

fn agent() -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(30))
            .build()
    })
}

/// 下载到临时文件再原子改名：中断不留下半截文件被加载器误读。
fn download_to(path: &Path, url: &str) -> Result<(), String> {
    let tmp = path.with_extension("part");
    let mut reader = agent()
        .get(url)
        .call()
        .map_err(|e| format!("请求失败 {url}: {e}"))?
        .into_reader();
    let mut buf = Vec::new();
    reader
        .read_to_end(&mut buf)
        .map_err(|e| format!("下载中断 {url}: {e}"))?;
    if buf.len() < 64 {
        return Err(format!("响应过短（{} 字节），疑似源失效: {url}", buf.len()));
    }
    std::fs::write(&tmp, &buf).map_err(|e| format!("写盘失败 {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("落盘失败 {}: {e}", path.display()))
}

#[derive(serde::Deserialize)]
struct DatavFull {
    features: Vec<serde_json::Value>,
}

/// 递归拉取 DataV 文件族，返回本次拉取的文件数（进度计数用）。
/// 全量约 375 个小文件（34 省 + ~340 市）；每文件落 `datav-<adcode>.json`。
fn download_datav_recursive(
    dir: &Path,
    adcode: &str,
    level: u8,
    cancel: &dyn Fn() -> bool,
    on_file: &dyn Fn(),
) -> Result<usize, String> {
    if cancel() {
        return Err("已取消".into());
    }
    let path = dir.join(format!("datav-{adcode}.json"));
    if !path.is_file() {
        let url = format!("{DATAV_BASE}/{adcode}_full.json");
        download_to(&path, &url)?;
        on_file(); // 每文件推进度（市县在递归深处，不回调进度会长时间不动）
    }
    let raw = std::fs::read(&path).map_err(|e| format!("读回失败: {e}"))?;
    let full: DatavFull =
        serde_json::from_slice(&raw).map_err(|e| format!("DataV JSON 无效（{adcode}）: {e}"))?;
    let mut count = 1usize;
    for feature in &full.features {
        let props = feature.get("properties");
        let Some(code) = props.and_then(|p| super::prop_code(p, "adcode")) else {
            continue;
        };
        // 九段线等界线要素（100000_JD）不是行政区：跳过
        if !code.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        // 语义驱动递归（2026-09-29 实测）：只有 city 有子级文件；district
        // 请求 _full 必 404（东城区/西湖区实测），不发废请求
        let child_is_city = props
            .and_then(|p| p.get("level"))
            .and_then(|v| v.as_str())
            == Some("city");
        if !child_is_city {
            continue;
        }
        // 市县文件容错：重试一次仍失败则跳过该地区（上级数据仍可用）
        match download_datav_recursive(dir, &code, level + 1, cancel, on_file) {
            Ok(n) => count += n,
            Err(err) => eprintln!("DataV 子包跳过（{code}）: {err}"),
        }
    }
    Ok(count)
}

/// DataV 全树文件数上限（进度 total 用；粗估值足够 UI 展示）。
const DATAV_ESTIMATED_FILES: u32 = 400;

/// 启动地理数据下载 + 后续管线。已在下载/已就绪时幂等返回。
pub fn start_download(db_dir: PathBuf, bus: Arc<EventBus>, supervisor: &Arc<TaskSupervisor>) {
    {
        let mut state = geo_state().lock().unwrap_or_else(|e| e.into_inner());
        // 只挡并发重复（下载中/管线中）；Ready 也允许再触发——上次中断可能
        // 只下了部分文件（文件存在即跳过，天然断点续传，补齐后指纹变化全量重刷）
        if matches!(
            state.phase,
            GeoPhase::Downloading { .. } | GeoPhase::Backfilling { .. } | GeoPhase::Loading
        ) {
            return;
        }
        state.phase = GeoPhase::Downloading {
            done: 0,
            total: DATAV_ESTIMATED_FILES,
        };
    }
    let bus = (*bus).clone();
    supervisor.spawn("geo", "geo-download".into(), move |controls| {
        let cancel = move || controls.is_cancelled();
        // 启动即广播：引导页凭这条事件切到进度态（不等首个包完成）
        bus.publish(AppEvent::MapGeoProgress {
            stage: "downloading".into(),
            done: 0,
            total: DATAV_ESTIMATED_FILES as u64,
            message: None,
        });
        let result = run_download(&db_dir, &bus, &cancel);
        let mut state = geo_state().lock().unwrap_or_else(|e| e.into_inner());
        match result {
            Ok(()) => {
                // 接力：加载树 → 回填（backfill 内部再推进 phase）
                drop(state);
                super::backfill::run_pipeline(&db_dir, &bus, &cancel);
            }
            Err(err) => {
                state.phase = GeoPhase::Failed(err.clone());
                bus.publish(AppEvent::MapGeoProgress {
                    stage: "failed".into(),
                    done: 0,
                    total: 0,
                    message: Some(err),
                });
            }
        }
    });
}

fn run_download(db_dir: &Path, bus: &EventBus, cancel: &dyn Fn() -> bool) -> Result<(), String> {
    let dir = super::geo_dir(db_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 geo 目录失败: {e}"))?;
    let mut done: u32 = 0;
    let total = 2 + DATAV_ESTIMATED_FILES;

    for pkg in [&PACKAGE_WORLD_ADM0, &PACKAGE_WORLD_ADM1] {
        if cancel() {
            return Err("已取消".into());
        }
        let path = dir.join(pkg.file);
        if !path.is_file() {
            download_to(&path, pkg.url)?;
            bus.publish(AppEvent::MapGeoProgress {
                stage: "downloading".into(),
                done: (done + 1) as u64,
                total: total as u64,
                message: Some(pkg.id.into()),
            });
        }
        done += 1;
    }

    // 中国包递归（省级根 → 市 → 县文件全拉齐；进度按文件数推进）
    let prov_path = dir.join(format!("datav-{DATAV_CHINA_CODE}.json"));
    if !prov_path.is_file() {
        download_to(
            &prov_path,
            &format!("{DATAV_BASE}/{DATAV_CHINA_CODE}_full.json"),
        )?;
    }
    let raw = std::fs::read(&prov_path).map_err(|e| format!("读省级文件失败: {e}"))?;
    let full: DatavFull =
        serde_json::from_slice(&raw).map_err(|e| format!("DataV 省级 JSON 无效: {e}"))?;
    let files_done = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    for feature in &full.features {
        if cancel() {
            return Err("已取消".into());
        }
        if let Some(code) = feature
            .get("properties")
            .and_then(|p| super::prop_code(p, "adcode"))
        {
            // 九段线等界线要素（100000_JD）不是行政区：跳过
            if !code.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            // 进度按文件数推进：递归深处市县文件经原子计数器回调汇报
            let counter = files_done.clone();
            let bus = bus.clone();
            let on_file = move || {
                let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
                bus.publish(AppEvent::MapGeoProgress {
                    stage: "downloading".into(),
                    done: (done + n) as u64,
                    total: total as u64,
                    message: Some("datav".into()),
                });
            };
            // 逐省递归（省市县三层）；单省失败重试一次
            let result = download_datav_recursive(&dir, &code, 1, cancel, &on_file);
            if result.is_err() {
                std::thread::sleep(Duration::from_millis(500));
                download_datav_recursive(&dir, &code, 1, cancel, &on_file)?;
            }
        }
        done += 1;
    }

    if !packages_installed(&dir) {
        return Err("下载完成但包不完整".into());
    }
    bus.publish(AppEvent::MapGeoProgress {
        stage: "downloading".into(),
        done: total as u64,
        total: total as u64,
        message: None,
    });
    Ok(())
}
