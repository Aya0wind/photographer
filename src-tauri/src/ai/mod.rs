//! AI 模型在线下载管理器（M4 前置，用户定案 2026-09-19：模型按需下载，
//! 安装包不含模型）。推理（ort/usearch）后续接入；本模块只管模型生命周期。
//!
//! - [`catalog`]：内置清单（HuggingFace 主源 + hf-mirror.com 镜像，sha256/
//!   bytesTotal 已用 HF API pin 死）。语义检索模型 = SigLIP2
//!   base-patch16-256（分离导出：vision_model_quantized 94MB /
//!   text_model_quantized 283MB / tokenizer.json 34MB——双塔合体文件
//!   model_quantized.onnx 378MB 的替代，内存减半语义相同；int8 量化在
//!   语义检索场景质量损失可接受，输出维度仍 768）。
//! - [`ModelManager`]：`.part` 暂存 + Content-Range 断点续传（网络中断保留
//!   `.part`，换源/重连从断点续传）；下载完成 SHA256 校验，不匹配删
//!   `.part` 重来一次，再失败置 failed；主 URL 失败自动切镜像，两处都败
//!   才 failed；同模型并发下载去重；cancel 清 `.part`；delete 删文件翻
//!   installed。进度/结果经 `aiModelDownloadProgress`（1s 节流）/
//!   `aiModelDownloadFinished` 事件回报，任务经 TaskSupervisor 派发。
//!
//! 模型落 `models_root/<id>.onnx`（app 配置目录下，全局共享不随库走）。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub mod embed;
pub mod semantic;

use crate::events::{AppEvent, EventBus, Throttle};
use crate::tasks::TaskSupervisor;

/// 进度事件节流（spec：1s）。
const PROGRESS_INTERVAL: Duration = Duration::from_secs(1);
/// 下载读块大小。
const CHUNK: usize = 256 * 1024;

/// 内置模型清单（JSON 常量 → 强类型；sha256/bytes 为 HF API 实测 pin 值）。
///
/// 语义模型选型（2026-09-19 调研定案）：目标为 SigLIP 2（多语言中文直搜、
/// 检索优于 CLIP），但可用 ONNX 量化转换版的 URL/SHA 需先核实——以下暂以
/// Xenova/clip-vit-base-patch32 量化版占位，M4 推理落地前替换为
/// siglip2-base-patch16-256 的 onnx 社区转换源（维度 512→768，HNSW/DB 同步改）。
const CATALOG_JSON: &str = r#"[
  {
    "id": "siglip2-visual",
    "url": "https://huggingface.co/onnx-community/siglip2-base-patch16-256-ONNX/resolve/main/onnx/vision_model_quantized.onnx",
    "mirrorUrl": "https://hf-mirror.com/onnx-community/siglip2-base-patch16-256-ONNX/resolve/main/onnx/vision_model_quantized.onnx",
    "sha256": "f2eb8ccfa3dc0b3761d9ea9a39554fe0f2be71b247ad7f68a80720ec88895650",
    "bytesTotal": 94737653,
    "version": "siglip2-base-patch16-256-v1",
    "feature": "semantic"
  },
  {
    "id": "siglip2-text",
    "url": "https://huggingface.co/onnx-community/siglip2-base-patch16-256-ONNX/resolve/main/onnx/text_model_quantized.onnx",
    "mirrorUrl": "https://hf-mirror.com/onnx-community/siglip2-base-patch16-256-ONNX/resolve/main/onnx/text_model_quantized.onnx",
    "sha256": "6f59b39d880c413042314b79302b74d0dd93b273caf8fbfdb1eb2df61a7fefd4",
    "bytesTotal": 283438275,
    "version": "siglip2-base-patch16-256-v1",
    "feature": "semantic"
  },
  {
    "id": "siglip2-tokenizer",
    "url": "https://huggingface.co/onnx-community/siglip2-base-patch16-256-ONNX/resolve/main/tokenizer.json",
    "mirrorUrl": "https://hf-mirror.com/onnx-community/siglip2-base-patch16-256-ONNX/resolve/main/tokenizer.json",
    "sha256": "cb9140fae3ac5122c972d37adf83e1248471a38147ad76f8215c8872c6fd8322",
    "bytesTotal": 34363039,
    "version": "siglip2-base-patch16-256-v1",
    "feature": "semantic"
  },
  {
    "id": "scrfd",
    "url": "https://huggingface.co/immich-app/scrfd_34g_gnkps/resolve/main/detection/model.onnx",
    "mirrorUrl": "https://hf-mirror.com/immich-app/scrfd_34g_gnkps/resolve/main/detection/model.onnx",
    "sha256": "aa19f0e7f4d120d4cf990086639ab74a0136adceaebd232e0dc4745e0cfd4257",
    "bytesTotal": 39424525,
    "version": "v1",
    "feature": "face"
  },
  {
    "id": "arcface",
    "url": "https://huggingface.co/garavv/arcface-onnx/resolve/main/arc.onnx",
    "mirrorUrl": "https://hf-mirror.com/garavv/arcface-onnx/resolve/main/arc.onnx",
    "sha256": "ffe014a45c9488506719d37fd578ece6661bb385535b36e8039975fa5d4683db",
    "bytesTotal": 136619444,
    "version": "v1",
    "feature": "face"
  }
]"#;

/// 清单条目（IPC 载荷形态，camelCase）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    pub id: String,
    pub url: String,
    pub mirror_url: String,
    pub sha256: String,
    pub bytes_total: u64,
    pub version: String,
    /// "semantic" | "face"
    pub feature: String,
}

/// 模型状态 DTO（ai_models_status 载荷，camelCase）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatusDto {
    pub id: String,
    pub installed: bool,
    pub bytes_total: u64,
    pub downloaded_bytes: u64,
    pub version: String,
    pub feature: String,
    /// "idle" | "downloading" | "verifying" | "done" | "failed"
    pub state: String,
}

/// 内置模型清单（OnceLock 单次解析）。
pub fn catalog() -> &'static [ModelEntry] {
    static CATALOG: std::sync::OnceLock<Vec<ModelEntry>> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| serde_json::from_str(CATALOG_JSON).expect("内置清单必须合法"))
}

// ---------------------------------------------------------------------------
// HTTP agent（全局一份；连接 15s / 读 60s 超时）
// ---------------------------------------------------------------------------

fn agent() -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(60))
            .build()
    })
}

// ---------------------------------------------------------------------------
// 管理器
// ---------------------------------------------------------------------------

/// 进行中下载的共享态（任务线程写 / 命令线程读）。
struct ActiveDownload {
    cancelled: AtomicBool,
    /// "downloading" | "verifying"
    state: Mutex<String>,
    done: AtomicU64,
}

/// 模型下载管理器（AppState 持有；Clone 共享）。
#[derive(Clone)]
pub struct ModelManager {
    root: PathBuf,
    bus: EventBus,
    supervisor: Arc<TaskSupervisor>,
    active: Arc<Mutex<HashMap<String, Arc<ActiveDownload>>>>,
    /// 最近一次失败的模型 id（重下载时清除）。
    failed: Arc<Mutex<HashSet<String>>>,
}

impl ModelManager {
    pub fn new(root: PathBuf, bus: EventBus, supervisor: Arc<TaskSupervisor>) -> Self {
        Self {
            root,
            bus,
            supervisor,
            active: Arc::new(Mutex::new(HashMap::new())),
            failed: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// 事件总线（测试订阅进度/完成事件用）。
    #[doc(hidden)]
    #[allow(dead_code)] // 集成测试引用（lib 目标内无调用点）
    pub fn bus(&self) -> &EventBus {
        &self.bus
    }

    fn final_path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.onnx"))
    }

    fn part_path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.onnx.part"))
    }

    /// 单模型状态（IO 读文件长度 + 内存态）。
    pub fn status(&self, entry: &ModelEntry) -> Result<ModelStatusDto, String> {
        let id = &entry.id;
        let final_len = fs::metadata(self.final_path(id)).map(|m| m.len()).ok();
        let installed = final_len == Some(entry.bytes_total);
        let part_len = fs::metadata(self.part_path(id))
            .map(|m| m.len())
            .unwrap_or(0);
        let active = self
            .active
            .lock()
            .expect("ai active mutex poisoned")
            .get(id)
            .cloned();
        let state = if let Some(active) = &active {
            active
                .state
                .lock()
                .expect("ai state mutex poisoned")
                .clone()
        } else if installed {
            "done".to_string()
        } else if self
            .failed
            .lock()
            .expect("ai failed mutex poisoned")
            .contains(id)
        {
            "failed".to_string()
        } else {
            "idle".to_string()
        };
        let downloaded_bytes = active
            .map(|a| a.done.load(Ordering::SeqCst))
            .unwrap_or(final_len.unwrap_or(part_len));
        Ok(ModelStatusDto {
            id: id.clone(),
            installed,
            bytes_total: entry.bytes_total,
            downloaded_bytes,
            version: entry.version.clone(),
            feature: entry.feature.clone(),
            state,
        })
    }

    /// 全清单状态。
    pub fn status_all(&self) -> Vec<ModelStatusDto> {
        catalog()
            .iter()
            .filter_map(|e| self.status(e).ok())
            .collect()
    }

    /// 发起下载：已在队（同模型）直接返回（去重）；否则登记 + supervisor
    /// 后台执行。结果经 `aiModelDownloadFinished` 事件回报。
    pub fn download(&self, entry: ModelEntry) -> Result<(), String> {
        {
            let mut active = self.active.lock().expect("ai active mutex poisoned");
            if active.contains_key(&entry.id) {
                return Ok(()); // 去重：已在下载
            }
            active.insert(
                entry.id.clone(),
                Arc::new(ActiveDownload {
                    cancelled: AtomicBool::new(false),
                    state: Mutex::new("downloading".into()),
                    done: AtomicU64::new(0),
                }),
            );
        }
        self.failed
            .lock()
            .expect("ai failed mutex poisoned")
            .remove(&entry.id);
        let mgr = self.clone();
        let id = entry.id.clone();
        self.supervisor
            .spawn("ai-model", format!("download-{id}"), move |_| {
                let outcome = mgr.run_download(&entry);
                mgr.finish(&id, outcome);
            });
        Ok(())
    }

    /// 取消下载（软标志，任务线程在块边界响应）；`.part` 由任务线程清理。
    pub fn cancel(&self, id: &str) -> Result<(), String> {
        let active = self.active.lock().expect("ai active mutex poisoned");
        match active.get(id) {
            Some(state) => {
                state.cancelled.store(true, Ordering::SeqCst);
                Ok(())
            }
            None => Err(format!("模型 {id} 没有进行中的下载")),
        }
    }

    /// 删除已下载模型文件（释放磁盘；installed 翻 false）。下载中被拒。
    pub fn delete(&self, id: &str) -> Result<(), String> {
        if self
            .active
            .lock()
            .expect("ai active mutex poisoned")
            .contains_key(id)
        {
            return Err(format!("模型 {id} 正在下载，先取消再删除"));
        }
        fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        for path in [self.final_path(id), self.part_path(id)] {
            if path.exists() {
                fs::remove_file(&path).map_err(|e| format!("删除失败 {}: {e}", path.display()))?;
            }
        }
        self.failed
            .lock()
            .expect("ai failed mutex poisoned")
            .remove(id);
        Ok(())
    }

    /// 任务收尾：除名 + 失败登记 + 事件。
    fn finish(&self, id: &str, outcome: Result<(), String>) {
        self.active
            .lock()
            .expect("ai active mutex poisoned")
            .remove(id);
        let (ok, error) = match outcome {
            Ok(()) => (true, None),
            Err(err) => {
                self.failed
                    .lock()
                    .expect("ai failed mutex poisoned")
                    .insert(id.to_string());
                (false, Some(err))
            }
        };
        self.bus.publish(AppEvent::AiModelDownloadFinished {
            id: id.to_string(),
            ok,
            error,
        });
    }

    /// 进行中下载数（测试/观测钩子）。
    #[doc(hidden)]
    #[allow(dead_code)] // 集成测试引用（lib 目标内无调用点）
    pub fn active_count(&self) -> usize {
        self.active.lock().expect("ai active mutex poisoned").len()
    }

    /// 下载主体（supervisor 线程）：[主源, 镜像] 轮询，网络级失败保留
    /// `.part` 换源续传；SHA 不匹配删 `.part` 整体重来一次；取消清理。
    fn run_download(&self, entry: &ModelEntry) -> Result<(), String> {
        let active = self
            .active
            .lock()
            .expect("ai active mutex poisoned")
            .get(&entry.id)
            .cloned()
            .expect("download 已登记");
        let part = self.part_path(&entry.id);
        let sources = [entry.url.as_str(), entry.mirror_url.as_str()];
        let mut sha_retried = false;
        loop {
            let mut sha_mismatch_this_round = false;
            for source in &sources {
                if active.cancelled.load(Ordering::SeqCst) {
                    let _ = fs::remove_file(&part);
                    return Err("下载已取消".into());
                }
                match fetch_source(source, &part, entry, &active, &self.bus) {
                    FetchOutcome::Installed => return Ok(()),
                    FetchOutcome::Cancelled => {
                        let _ = fs::remove_file(&part);
                        return Err("下载已取消".into());
                    }
                    FetchOutcome::ShaMismatch => {
                        let _ = fs::remove_file(&part); // 校验失败不留脏
                        sha_mismatch_this_round = true;
                        break; // 本轮作废（.part 已删），整源重来
                    }
                    FetchOutcome::Failed => {
                        // 网络/短读：保留 .part → 换源/下一轮 Range 续传
                    }
                }
            }
            if sha_mismatch_this_round {
                if sha_retried {
                    return Err("SHA256 校验不匹配（已重试一次）".into());
                }
                sha_retried = true;
                continue; // 主源+镜像整体重来一轮
            }
            // 到这里必然两源网络级失败
            return Err("主源与镜像均不可用（网络失败）".into());
        }
    }
}

/// 单源拉取结果。
enum FetchOutcome {
    /// 校验通过并落位。
    Installed,
    /// 用户取消（.part 由调用方清理）。
    Cancelled,
    /// SHA256 不匹配（.part 已由调用方清理）。
    ShaMismatch,
    /// 网络/短读失败（.part 保留，供续传）。
    Failed,
}

/// 从单源拉取：Range 断点续传（服务器忽略 Range 时 200 从头重写）；
/// 流式写 `.part` + 1s 节流进度；满量后整文件 SHA256 校验，通过则
/// rename 落位。
fn fetch_source(
    url: &str,
    part: &std::path::Path,
    entry: &ModelEntry,
    active: &ActiveDownload,
    bus: &EventBus,
) -> FetchOutcome {
    let resume = fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    let request = agent()
        .get(url)
        .set("Range", &format!("bytes={resume}-"))
        .call();
    let response = match request {
        Ok(resp) if resp.status() == 200 || resp.status() == 206 => resp,
        _ => return FetchOutcome::Failed,
    };
    let append = response.status() == 206 && resume > 0;
    let file = if append {
        fs::OpenOptions::new().append(true).open(part)
    } else {
        fs::create_dir_all(part.parent().unwrap_or(std::path::Path::new("")))
            .and_then(|_| fs::File::create(part))
    };
    let mut file = match file {
        Ok(f) => f,
        Err(_) => return FetchOutcome::Failed,
    };
    let mut reader = response.into_reader();
    let mut buf = vec![0u8; CHUNK];
    let mut written = if append { resume } else { 0 };
    let mut throttle = Throttle::new(PROGRESS_INTERVAL);
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if file.write_all(&buf[..n]).is_err() {
                    return FetchOutcome::Failed;
                }
                written += n as u64;
                active.done.store(written, Ordering::SeqCst);
                if active.cancelled.load(Ordering::SeqCst) {
                    return FetchOutcome::Cancelled;
                }
                if throttle.should_fire() {
                    bus.publish(AppEvent::AiModelDownloadProgress {
                        id: entry.id.clone(),
                        done_bytes: written,
                        total_bytes: entry.bytes_total,
                    });
                }
            }
            Err(_) => return FetchOutcome::Failed, // 中断：保留 .part 续传
        }
    }
    if file.flush().is_err() || written != entry.bytes_total {
        return FetchOutcome::Failed; // 短读（连接被掐）：保留 .part
    }
    // 校验阶段
    *active.state.lock().expect("ai state mutex poisoned") = "verifying".into();
    let actual = sha256_file(part);
    if actual != entry.sha256 {
        return FetchOutcome::ShaMismatch;
    }
    let final_path = part.with_extension(""); // <id>.onnx.part → <id>.onnx
    if fs::rename(part, &final_path).is_err() {
        return FetchOutcome::Failed;
    }
    FetchOutcome::Installed
}

/// 流式 SHA256（hex 小写）。
fn sha256_file(path: &std::path::Path) -> String {
    let Ok(mut file) = fs::File::open(path) else {
        return String::new();
    };
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[..n]),
            Err(_) => return String::new(),
        }
    }
    format!("{:x}", hasher.finalize())
}
