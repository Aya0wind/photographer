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
pub mod face;
pub mod selection;
pub mod semantic;

use crate::events::{AppEvent, EventBus, Throttle};
use crate::tasks::TaskSupervisor;

/// 进度事件节流（spec：1s）。
const PROGRESS_INTERVAL: Duration = Duration::from_secs(1);
/// 下载读块大小。
const CHUNK: usize = 256 * 1024;

/// 内置模型清单（JSON 常量 → 强类型；sha256/bytes 为 HF API 实测 pin 值）。
///
/// feature="selection"（闭眼检测，0021 选型 2026-09-27）：**暂无条目**。
/// 评估结论——HuggingFace 许可证干净（Apache-2.0）的 open/closed eye
/// 分类器（dima806/closed_eyes_image_detection、MrKrauzer/
/// closed-eyes-image-detection）均为 ViT-base（~330MB）且无 ONNX 权重；
/// MIT+ONNX 的 notgoodkeeper/cnn-based-drowsiness-detection 是整图驾驶
/// 困倦分类，语义/标签不适用于双眼裁剪。按定案「不塞来源不明的权重」
/// 停止收录：eyes 通道代码就绪（ai::selection，trait 注入可测），模型
/// 收录（含 ONNX 会话实现与输入规格）待找到合规小模型后落地。
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
    /// "semantic" | "face"（"selection" 预留：闭眼模型选型未过，暂无条目）
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
// EP 选择与会话构建（embed/face 共用，2026-09-21 DML 接入）
// ---------------------------------------------------------------------------

/// DML 运行时故障位（进程级一次）：DML EP **注册**成功但运行时节点报错
/// （真机 2026-09-21：SigLIP2 int8 的 LayerNormFusion 在 RTX 5070 Ti 的
/// DML 上 E_INVALIDARG——ort 的逐算子回落只覆盖"不支持"，不覆盖"执行即
/// 炸"）。置位后本进程所有新会话回落纯 CPU；推理层捕获错误后丢弃 DML
/// 会话重建（见 run_with_dml_fallback）。
fn poison_flag() -> &'static std::sync::atomic::AtomicBool {
    static POISON: std::sync::OnceLock<std::sync::atomic::AtomicBool> = std::sync::OnceLock::new();
    POISON.get_or_init(|| std::sync::atomic::AtomicBool::new(false))
}

fn dml_poisoned() -> bool {
    poison_flag().load(std::sync::atomic::Ordering::Relaxed)
}

fn poison_dml() {
    poison_flag().store(true, std::sync::atomic::Ordering::Relaxed);
}

/// 当前意图是否 DML 优先（env 覆盖 > use_gpu；已毒化则否）。
fn dml_intended(use_gpu: bool) -> bool {
    match std::env::var("SMARTPHOTO_AI_EP").as_deref() {
        Ok("dml") => !dml_poisoned(),
        Ok("cpu") => false,
        _ => use_gpu && !dml_poisoned(),
    }
}

/// EP 序列：`use_gpu=true` → `[DirectML, CPU]`——DML **注册**失败时 ort 记
/// 警告并回落（CPU EP 恒在队尾兜底，逐算子不支持的也自动回落 CPU）；
/// `use_gpu=false` 或 DML 已毒化（运行时故障）→ 纯 CPU。env
/// `SMARTPHOTO_AI_EP=dml|cpu` 强制覆盖（"dml" 越过 use_gpu 开关，"cpu"
/// 压制之）——基准 A/B 与现场诊断用，优先级最高。
pub(crate) fn execution_providers(use_gpu: bool) -> Vec<ort::ep::ExecutionProviderDispatch> {
    if dml_intended(use_gpu) {
        vec![
            ort::ep::DirectML::default().build(),
            ort::ep::CPU::default().build(),
        ]
    } else {
        vec![ort::ep::CPU::default().build()]
    }
}

/// DML 运行时故障的统一处置：`run` 失败且当前会话确为 DML 优先时——置
/// 毒化位 + `reset` 丢弃该 DML 会话 + 重跑一次（重建会话经
/// execution_providers 自动回落纯 CPU）。非 DML 会话 / 二次失败原样上抛。
pub(crate) fn run_with_dml_fallback<T>(
    use_gpu: bool,
    mut run: impl FnMut() -> Result<T, String>,
    reset: impl FnOnce(),
) -> Result<T, String> {
    match run() {
        Ok(v) => Ok(v),
        Err(err) => {
            if dml_intended(use_gpu) {
                poison_dml();
                eprintln!("DML 推理运行时故障，本进程回落纯 CPU 并重建会话: {err}");
                reset();
                run()
            } else {
                Err(err)
            }
        }
    }
}

/// 索引推理是否走批量（批大 >1 只在 DML 激活时划算：真机 2026-09-21
/// RTX 5070 Ti，SigLIP2 vision int8——DML 批16 = 24.6 img/s vs 单图 20.0
/// （+23%）；CPU 批16 = 12.5 vs 单图 14.1（**-12%**，单图 Run 的 intra-op
/// 线程已吃满核，批化反而劣化）。CPU/毒化/显式 cpu 一律批 1。
pub(crate) fn prefer_batch_inference() -> bool {
    dml_intended(true)
}

/// ort 会话构建（embed/face 共用）：图优化 + 可选 intra 线程 + EP 序列。
/// intra=None 用 ORT 默认（全物理核）；embed 传核心数/4（多 worker 不
/// 超订，见 embed.rs），face 单 worker 用默认。
/// 图优化档位：**DML 会话自动 Level1**——Level3 的 LayerNormFusion 在
/// DML 上执行即炸（E_INVALIDARG，真机 2026-09-21），Level1 绕开融合后
/// SigLIP2 vision 可全跑 DML；CPU 会话保持 Level3（SCRFD 实测 Level1
/// 比 Level3 慢 ~36%）。env `SMARTPHOTO_AI_OPT=level1|disable` 可强制
/// 覆盖（诊断用）。
pub(crate) fn build_session(
    path: &std::path::Path,
    intra_threads: Option<usize>,
    use_gpu: bool,
    model_label: &str,
) -> Result<ort::session::Session, String> {
    use ort::session::builder::GraphOptimizationLevel;
    let opt_level = match std::env::var("SMARTPHOTO_AI_OPT").as_deref() {
        Ok("level1") => GraphOptimizationLevel::Level1,
        Ok("disable") => GraphOptimizationLevel::Disable,
        _ if dml_intended(use_gpu) => GraphOptimizationLevel::Level1,
        _ => GraphOptimizationLevel::Level3,
    };
    let mut builder = ort::session::Session::builder().map_err(|e| e.to_string())?;
    if let Some(n) = intra_threads {
        builder = builder.with_intra_threads(n).map_err(|e| e.to_string())?;
    }
    builder
        .with_optimization_level(opt_level)
        .map_err(|e| e.to_string())?
        .with_execution_providers(execution_providers(use_gpu))
        .map_err(|e| e.to_string())?
        .commit_from_file(path)
        .map_err(|e| format!("加载 {model_label} 失败: {e}"))
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

/// AI 索引推理参数（settings.ai 的运行时投影；settings_set / 启动时刷新）。
/// 放 ModelManager（Clone 共享）而非逐层传参：embed/face 推理层都从
/// manager 取，避免 SemanticEmbedder trait 与聚类缓存签名随配置膨胀。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AiIndexParams {
    /// 语义嵌入输入档位（px；thumb 档 + squash + 张量边长共用）。
    pub embed_input_size: u16,
    /// SCRFD 检测框置信门槛。
    pub face_detect_threshold: f32,
    /// 在线聚类归簇 cos 阈值。
    pub face_cluster_threshold: f32,
    /// 允许 GPU（settings.ai.use_gpu 投影）：true 时会话 EP 序列
    /// [DirectML, CPU]（DML 失败自动落 CPU，见 execution_providers）。
    pub use_gpu: bool,
}

impl Default for AiIndexParams {
    fn default() -> Self {
        Self {
            embed_input_size: 256,
            face_detect_threshold: 0.5,
            face_cluster_threshold: 0.4,
            use_gpu: true,
        }
    }
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
    /// 索引推理参数（settings 快照；启动 / settings_set 时刷新）。
    params: Arc<Mutex<AiIndexParams>>,
}

impl ModelManager {
    pub fn new(root: PathBuf, bus: EventBus, supervisor: Arc<TaskSupervisor>) -> Self {
        Self {
            root,
            bus,
            supervisor,
            active: Arc::new(Mutex::new(HashMap::new())),
            failed: Arc::new(Mutex::new(HashSet::new())),
            params: Arc::new(Mutex::new(AiIndexParams::default())),
        }
    }

    /// 当前索引推理参数（推理层读；缺省 256/0.5/0.4）。
    pub fn ai_params(&self) -> AiIndexParams {
        *self.params.lock().expect("ai params mutex poisoned")
    }

    /// 参数快照刷新（启动加载 settings / settings_set 落库后调用）。
    pub fn set_ai_params(&self, params: AiIndexParams) {
        *self.params.lock().expect("ai params mutex poisoned") = params;
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
