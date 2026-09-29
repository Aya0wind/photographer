//! 地理数据内置安装（2026-09-30 用户定案：9.8MB 数据包随二进制分发，
//! include_bytes 嵌入——无下载、无管理 UI，就当资源文件）：
//! - 数据落应用配置目录 `config_dir/geo/`（与 models 同级，全库共享）；
//! - 首次启动（或地图页发现未就位）解压一次，之后文件常驻磁盘不再动；
//! - 解压完成自动接力 backfill 管线（加载树 → regions 入库 → GPS 回填）。
//! 包内容：DataV 中国省市县 398 文件 + Natural Earth 50m 世界两包
//! （assets/geo-data.zip，DataV areas_v3 2021.5 版快照）。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::events::{AppEvent, EventBus};
use crate::tasks::TaskSupervisor;

use super::{datav_count, geo_dir, packages_installed, GeoPhase};

/// 内置数据包（编译期嵌入；更新数据 = 换 zip 重编，无版本协商）
pub const GEO_DATA_ZIP: &[u8] = include_bytes!("../../assets/geo-data.zip");

/// 完整判定的 DataV 文件下限（快照全量 398；留余量防个别文件剔除）
const DATAV_COMPLETE_MIN: usize = 340;

/// 数据包是否已解压就位（文件常驻后即真，永不再解）
fn extracted(dir: &Path) -> bool {
    packages_installed(dir) && datav_count(dir) >= DATAV_COMPLETE_MIN
}

/// 解压内置包到 `dir`（覆盖同名文件；条目名收扁平文件名，拒目录穿越——
/// 内置包虽可信，守卫零成本）。返回解出的文件数。
pub fn extract_embedded(dir: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建 geo 目录失败: {e}"))?;
    let reader = std::io::Cursor::new(GEO_DATA_ZIP);
    let mut zip = zip::ZipArchive::new(reader).map_err(|e| format!("内置数据包无效: {e}"))?;
    let mut count = 0usize;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("读取包条目失败: {e}"))?;
        let name = entry.name().to_string();
        if name.contains("..")
            || name.contains('\\')
            || name.contains('/')
            || name.is_empty()
            || entry.is_dir()
        {
            continue;
        }
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut buf)
            .map_err(|e| format!("解压 {name} 失败: {e}"))?;
        let final_path = dir.join(&name);
        let tmp = dir.join(format!("{name}.part"));
        std::fs::write(&tmp, &buf).map_err(|e| format!("写盘失败 {name}: {e}"))?;
        std::fs::rename(&tmp, &final_path)
            .map_err(|e| format!("落盘失败 {name}: {e}"))?;
        count += 1;
    }
    Ok(count)
}

/// 幂等安装入口：未解压则后台解压一次 → 接力 backfill 管线（load →
/// regions 入库 → GPS 回填 → Ready；库内已有指纹未变时全为幂等快路径）。
/// 已解压也走管线（应用重启后恢复内存索引/库内 regions 真值）。
/// 管线已在跑（Loading/Backfilling）直接返回，不重入。
pub fn ensure_installed(
    config_dir: PathBuf,
    db_dir: PathBuf,
    bus: Arc<EventBus>,
    supervisor: &Arc<TaskSupervisor>,
) {
    {
        let mut state = super::geo_state().lock().unwrap_or_else(|e| e.into_inner());
        if matches!(state.phase, GeoPhase::Loading | GeoPhase::Backfilling { .. }) {
            return;
        }
        state.phase = GeoPhase::Loading;
    }
    let bus = (*bus).clone();
    supervisor.spawn("geo", "geo-install".into(), move |controls| {
        let cancel = move || controls.is_cancelled();
        let dir = geo_dir(&config_dir);
        if !extracted(&dir) {
            if let Err(err) = extract_embedded(&dir) {
                let mut state = super::geo_state().lock().unwrap_or_else(|e| e.into_inner());
                state.phase = GeoPhase::Failed(err.clone());
                bus.publish(AppEvent::MapGeoProgress {
                    stage: "failed".into(),
                    done: 0,
                    total: 0,
                    message: Some(err),
                });
                return;
            }
            // 旧版按库存放的半截数据（db_dir/geo）：确认新包就位后清理
            let legacy = db_dir.join("geo");
            if legacy.is_dir() {
                let _ = std::fs::remove_dir_all(&legacy);
            }
        }
        super::backfill::run_pipeline(&db_dir, &config_dir, &bus, &cancel);
    });
}
