//! AI 模型在线下载管理器（M4 前置，用户定案 2026-09-19：模型按需下载，
//! 安装包不含模型）。推理（ort/usearch）后续接入；本模块只管模型生命周期。
//!
//! - [`catalog`]：内置清单（HuggingFace 主源 + hf-mirror.com 镜像，sha256/
//!   bytesTotal 已用 HF API pin 死）。语义检索模型 = SigLIP2
//!   base-patch16-256（分离导出：vision_model_quantized 94MB /
//!   text_model_quantized 283MB / tokenizer.json 34MB——双塔合体文件
//!   model_quantized.onnx 378MB 的替代，内存减半语义相同；int8 量化在
//!   语义检索场景质量损失可接受，输出维度仍 768）。
//! - 三档画质（2026-09-28 定案，[`QualityTier`]）：fast 换 SCRFD 10G 小
//!   检测模型、accurate 换 fp16 语义双塔（检测源三档统一缓存优先，normal 维持
//!   既有件。档位→模型/源策略的解析集中在 [`face_detect_model_id`] /
//!   [`semantic_model_ids`]，切档经参数指纹自动重建（ipc::indexing）。
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
pub mod focus_quality;
pub mod idle;
pub mod selection;
pub mod selection_defocus;
pub mod selection_regions;
pub mod semantic;

use crate::events::{AppEvent, EventBus, Throttle};
use crate::tasks::TaskSupervisor;

/// 进度事件节流（spec：1s）。
const PROGRESS_INTERVAL: Duration = Duration::from_secs(1);
/// 下载读块大小。
const CHUNK: usize = 256 * 1024;

/// 内置模型清单（JSON 常量 → 强类型；sha256/bytes 为 HF API 实测 pin 值，
/// LFS oid 即内容 sha256；scrfd-10g 另做了全量下载校验，2026-09-28）。
///
/// **三档画质新件的许可记档（2026-09-28 核实）**：
/// - `scrfd-10g`：原计划的独立仓 immich-app/scrfd_10g_bnkps 已不可达
///   （HF API/resolve 均 401，2026-09-28 实测）——同字节权重（sha256 一致，
///   且与 antelopev2 仓 detection/model.onnx 同 oid）由 insightface 官方
///   buffalo_l 包仓 immich-app/buffalo_l 提供，改从该仓收录。**注意许可**：
///   该仓 license = "other"（insightface，见仓 README license_link 指向
///   deepinsight/insightface python-package 许可）而非定案时以为的
///   Apache-2.0；既有 arcface（garavv/arcface-onnx）同为 insightface 系
///   权重，风险口径一致（个人摄影工作流自托管使用场景）。
/// - `siglip2-*-fp16`：与既有 int8 件同仓（onnx-community/
///   siglip2-base-patch16-256-ONNX）同许可（仓未单列许可，随基模型
///   google/siglip2-base-patch16-256 = Apache-2.0，与现件一致，无新增
///   许可负担）。
///
/// feature="selection"（闭眼检测，0021 选型 2026-09-27 → 实装 2026-09-28）：
/// - 前一稿停摆结论（无合规小模型）作废原因：改为收录 **MediaPipe Face
///   Landmarker 478 点 ONNX**（yakhyo/mediapipe-face-mesh-onnx 导出），
///   走 EAR 眼部长宽比几何判据而非端到端「闭眼分类器」——不需要大分类
///   模型，Apache-2.0 干净件即可满足。license 证据：导出仓 LICENSE =
///   Apache-2.0（2026-09-28 核实 https://github.com/yakhyo/
///   mediapipe-face-mesh-onnx）；权重按其 README 系从 Google MediaPipe
///   `face_landmarker.task`（face_landmarks_detector.tflite）反量化导出，
///   MediaPipe 本体 Apache-2.0，权重可再分发。
/// - 文件：GitHub Releases `releases/download/weights/
///   face_landmarker_Nx3x256x256.onnx`，4.86MB，sha256 实测 pin 死。
///   hf-mirror 只镜像 HuggingFace 不镜像 GitHub Releases → mirrorUrl
///   填同 GitHub 主源（重试语义，无第二源）。
/// - 实装校准注记（2026-09-28 Python/ONNXRuntime 实测，详见
///   ai::selection eyes 段注释）：468 点眼睑网格与官方 face_landmarker
///   对齐良好；**虹膜点（468-477）系统性漂移不可用**；score 头输出
///   与导出仓 docstring（"confident faces 20-40"）不符（正脸裁剪
///   logit −8~−31 且与背景无判别力）→ 两者在实现中均不作判据。
///
/// 语义模型选型（2026-09-19 调研定案）：目标为 SigLIP 2（多语言中文直搜、
/// 检索优于 CLIP），但可用 ONNX 量化转换版的 URL/SHA 需先核实——以下暂以
/// Xenova/clip-vit-base-patch32 量化版占位，M4 推理落地前替换为
/// siglip2-base-patch16-256 的 onnx 社区转换源（维度 512→768，HNSW/DB 同步改）。
// open-closed-eye: official OMZ original ONNX, Apache-2.0 training extensions.
// BGR mean/scale are applied by selection_regions; infrared training is not
// treated as photography calibration. SHA256 and size verified from the download.
const CATALOG_JSON: &str = r#"[
  {
    "id": "open-closed-eye",
    "url": "https://storage.openvinotoolkit.org/repositories/open_model_zoo/public/2022.1/open-closed-eye-0001/open_closed_eye.onnx",
    "mirrorUrl": "https://download.01.org/opencv/openvino_training_extensions/models/open_closed_eye/open_closed_eye.onnx",
    "sha256": "4daa100034482525a26c9afb9297c16580a531189e66e3d2b2ac7d32becfd593",
    "bytesTotal": 46164,
    "version": "omz-2022.1-candidate-v1",
    "feature": "selection",
    "tier": null
  },
  {
    "id": "siglip2-visual",
    "url": "https://huggingface.co/onnx-community/siglip2-base-patch16-256-ONNX/resolve/main/onnx/vision_model_quantized.onnx",
    "mirrorUrl": "https://hf-mirror.com/onnx-community/siglip2-base-patch16-256-ONNX/resolve/main/onnx/vision_model_quantized.onnx",
    "sha256": "f2eb8ccfa3dc0b3761d9ea9a39554fe0f2be71b247ad7f68a80720ec88895650",
    "bytesTotal": 94737653,
    "version": "siglip2-base-patch16-256-v1",
    "feature": "semantic",
    "tier": "normal"
  },
  {
    "id": "siglip2-text",
    "url": "https://huggingface.co/onnx-community/siglip2-base-patch16-256-ONNX/resolve/main/onnx/text_model_quantized.onnx",
    "mirrorUrl": "https://hf-mirror.com/onnx-community/siglip2-base-patch16-256-ONNX/resolve/main/onnx/text_model_quantized.onnx",
    "sha256": "6f59b39d880c413042314b79302b74d0dd93b273caf8fbfdb1eb2df61a7fefd4",
    "bytesTotal": 283438275,
    "version": "siglip2-base-patch16-256-v1",
    "feature": "semantic",
    "tier": "normal"
  },
  {
    "id": "siglip2-tokenizer",
    "url": "https://huggingface.co/onnx-community/siglip2-base-patch16-256-ONNX/resolve/main/tokenizer.json",
    "mirrorUrl": "https://hf-mirror.com/onnx-community/siglip2-base-patch16-256-ONNX/resolve/main/tokenizer.json",
    "sha256": "cb9140fae3ac5122c972d37adf83e1248471a38147ad76f8215c8872c6fd8322",
    "bytesTotal": 34363039,
    "version": "siglip2-base-patch16-256-v1",
    "feature": "semantic",
    "tier": null
  },
  {
    "id": "siglip2-visual-fp16",
    "url": "https://huggingface.co/onnx-community/siglip2-base-patch16-256-ONNX/resolve/main/onnx/vision_model_fp16.onnx",
    "mirrorUrl": "https://hf-mirror.com/onnx-community/siglip2-base-patch16-256-ONNX/resolve/main/onnx/vision_model_fp16.onnx",
    "sha256": "fe9ad8020a6d3d98d394c9be8f07064066135fc2f87ec11692de0b677c0ac4db",
    "bytesTotal": 186131676,
    "version": "siglip2-base-patch16-256-fp16-v1",
    "feature": "semantic",
    "tier": "accurate"
  },
  {
    "id": "siglip2-text-fp16",
    "url": "https://huggingface.co/onnx-community/siglip2-base-patch16-256-ONNX/resolve/main/onnx/text_model_fp16.onnx",
    "mirrorUrl": "https://hf-mirror.com/onnx-community/siglip2-base-patch16-256-ONNX/resolve/main/onnx/text_model_fp16.onnx",
    "sha256": "80954edffdc689599e5d5bc6a1738380bc9e8139a18e5c8892485f248b6b4890",
    "bytesTotal": 564862230,
    "version": "siglip2-base-patch16-256-fp16-v1",
    "feature": "semantic",
    "tier": "accurate"
  },
  {
    "id": "scrfd",
    "url": "https://huggingface.co/immich-app/scrfd_34g_gnkps/resolve/main/detection/model.onnx",
    "mirrorUrl": "https://hf-mirror.com/immich-app/scrfd_34g_gnkps/resolve/main/detection/model.onnx",
    "sha256": "aa19f0e7f4d120d4cf990086639ab74a0136adceaebd232e0dc4745e0cfd4257",
    "bytesTotal": 39424525,
    "version": "v1",
    "feature": "face",
    "tier": "normal"
  },
  {
    "id": "scrfd-10g",
    "url": "https://huggingface.co/immich-app/buffalo_l/resolve/main/detection/model.onnx",
    "mirrorUrl": "https://hf-mirror.com/immich-app/buffalo_l/resolve/main/detection/model.onnx",
    "sha256": "5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91",
    "bytesTotal": 16923827,
    "version": "v1",
    "feature": "face",
    "tier": "fast"
  },
  {
    "id": "arcface",
    "url": "https://huggingface.co/garavv/arcface-onnx/resolve/main/arc.onnx",
    "mirrorUrl": "https://hf-mirror.com/garavv/arcface-onnx/resolve/main/arc.onnx",
    "sha256": "ffe014a45c9488506719d37fd578ece6661bb385535b36e8039975fa5d4683db",
    "bytesTotal": 136619444,
    "version": "v1",
    "feature": "face",
    "tier": null
  },
  {
    "id": "facemesh",
    "url": "https://github.com/yakhyo/mediapipe-face-mesh-onnx/releases/download/weights/face_landmarker_Nx3x256x256.onnx",
    "mirrorUrl": "https://github.com/yakhyo/mediapipe-face-mesh-onnx/releases/download/weights/face_landmarker_Nx3x256x256.onnx",
    "sha256": "111795f8703cdeb6d0c68a9f3cc966a0f23f8786bb00f4577a11f461fc4276ac",
    "bytesTotal": 4864717,
    "version": "mediapipe-face-landmarker-478-v1",
    "feature": "selection",
    "tier": null
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
    /// "semantic" | "face" | "selection"（selection = 闭眼检测 facemesh）
    pub feature: String,
    /// 画质档位归属（2026-09-28 三档画质）：Some("fast"|"normal"|"accurate")
    /// = 该档独占件；None = 各档共用件（arcface / tokenizer）。
    #[serde(default)]
    pub tier: Option<String>,
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
    /// 档位归属（镜像清单条目；None = 各档共用件）。
    #[serde(default)]
    pub tier: Option<String>,
}

/// 内置模型清单（OnceLock 单次解析）。
///
/// debug 构建支持 `SMARTPHOTO_DL_TEST_BASE`（如 `http://127.0.0.1:8787`）：
/// 全部 url/mirrorUrl 重写到本地测试服务器（保留文件名），配合
/// `scripts/test_dl_server.py` 确定性复现 下载失败/断流续传/SHA 不匹配。
/// release 构建无此行为；正常开发不带该变量也不受影响。
pub fn catalog() -> &'static [ModelEntry] {
    static CATALOG: std::sync::OnceLock<Vec<ModelEntry>> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| {
        let entries: Vec<ModelEntry> =
            serde_json::from_str(CATALOG_JSON).expect("内置清单必须合法");
        #[cfg(debug_assertions)]
        {
            if let Ok(base) = std::env::var("SMARTPHOTO_DL_TEST_BASE") {
                let base = base.trim_end_matches('/');
                // 按 id 重写（URL 原名可能撞名：scrfd 与 scrfd-10g 的原 URL
                // 文件名同为 model.onnx）；测试服务器按 <id> / <id>.onnx /
                // <id>.json 顺序解析到本地文件。
                entries
                    .into_iter()
                    .map(|mut e| {
                        e.url = format!("{base}/{}", e.id);
                        e.mirror_url = format!("{base}/{}", e.id);
                        e
                    })
                    .collect()
            } else {
                entries
            }
        }
        #[cfg(not(debug_assertions))]
        entries
    })
}

// ---------------------------------------------------------------------------
// 三档画质（快速/普通/精准，用户定案 2026-09-28）
// ---------------------------------------------------------------------------

/// AI 索引画质档位。ArcFace 不换；选片闭眼通道统一使用 512px 缓存缩略图。
///
/// | 档 | 人脸检测 | 检测源策略 | 语义 |
/// |---|---|---|---|
/// | fast | scrfd-10g（buffalo_l 包，17MB） | 缓存优先 [512, 2048] | base int8 |
/// | normal（默认） | scrfd（34g） | 缓存优先 [512, 2048] | base int8 |
/// | accurate | scrfd（34g，同件） | 缓存优先 [512, 2048]（三档统一；2048 优先已退役） | base fp16 |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QualityTier {
    Fast,
    #[default]
    Normal,
    Accurate,
}

impl QualityTier {
    /// settings.ai.quality_tier 字符串解析（非法值 → None，settings_set 拒绝）。
    pub fn from_setting(value: &str) -> Option<Self> {
        match value {
            "fast" => Some(Self::Fast),
            "normal" => Some(Self::Normal),
            "accurate" => Some(Self::Accurate),
            _ => None,
        }
    }

    /// 档位字符串形态（日志/测试断言用）。
    #[doc(hidden)]
    #[allow(dead_code)] // 集成测试引用（lib 目标内无调用点）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Normal => "normal",
            Self::Accurate => "accurate",
        }
    }
}

/// 人脸检测模型 id（fast = 10G 小模型换速，normal/accurate = 34G 现件）。
pub fn face_detect_model_id(tier: QualityTier) -> &'static str {
    match tier {
        QualityTier::Fast => "scrfd-10g",
        _ => "scrfd",
    }
}

/// 语义双塔模型 id 组 [vision, text]（fast/normal = int8 现件共享 → fast↔normal
/// 不动语义索引；accurate = fp16 新件 → 切档经指纹自动重建）。
pub fn semantic_model_ids(tier: QualityTier) -> [&'static str; 2] {
    match tier {
        QualityTier::Accurate => ["siglip2-visual-fp16", "siglip2-text-fp16"],
        _ => ["siglip2-visual", "siglip2-text"],
    }
}

// ---------------------------------------------------------------------------
// HTTP agent（全局一份；连接 15s / 读 15s 超时）
// ---------------------------------------------------------------------------

fn agent() -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            // 读超时 15s（2026-09-28 由 60s 收紧）：模型源（HF/GH）是稳定
            // 流式，15s 无数据即视为僵死连接；同时保证取消/换源最坏 15s 内
            // 可见——软取消标志只有在阻塞读返回时才能被观测到，60s 会让
            // 僵死连接下的取消等 30-45s（真机实证）。
            .timeout_read(Duration::from_secs(15))
            .build()
    })
}

// ---------------------------------------------------------------------------
// EP 选择与会话构建（embed/face 共用，2026-09-21 DML 接入）
// ---------------------------------------------------------------------------

/// DML 运行时故障位（**按模型隔离**，2026-09-28）：DML EP **注册**成功但
/// 运行时节点报错（真机 2026-09-21：SigLIP2 int8 的 LayerNormFusion 在
/// RTX 5070 Ti 的 DML 上 E_INVALIDARG——ort 的逐算子回落只覆盖"不支持"，
/// 不覆盖"执行即炸"）。某模型（"scrfd"/"scrfd-10g"/"arcface"/"siglip2-visual"
/// /"siglip2-text"/"siglip2-visual-fp16"/"siglip2-text-fp16"/"facemesh"）
/// 运行时故障只毒化该模型——重建纯 CPU 会话时**其他模型保住 DML**（此前全局一位，单模型炸
/// 会连坐全部通道）。推理层捕获错误后丢弃该模型的 DML 会话重建（见
/// run_with_acceleration_fallback）。
type PoisonedBackends = HashMap<String, HashSet<crate::platform::InferenceBackend>>;
fn poison_flags() -> &'static Mutex<PoisonedBackends> {
    static POISON: std::sync::OnceLock<Mutex<PoisonedBackends>> = std::sync::OnceLock::new();
    POISON.get_or_init(|| Mutex::new(HashMap::new()))
}

fn backend_poisoned(model: &str, backend: crate::platform::InferenceBackend) -> bool {
    poison_flags()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(model)
        .is_some_and(|backends| backends.contains(&backend))
}

fn poison_backend(model: &str, backend: crate::platform::InferenceBackend) {
    poison_flags()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(model.to_owned())
        .or_default()
        .insert(backend);
}

fn inference_plan(use_gpu: bool, model: &str) -> crate::platform::InferencePlan {
    use crate::platform::{AccelerationPreference, InferenceBackend, InferencePlan};
    let preference = match std::env::var("SMARTPHOTO_AI_EP").as_deref() {
        Ok("cpu") => AccelerationPreference::Cpu,
        Ok("dml") => AccelerationPreference::DirectMl,
        Ok("coreml") => AccelerationPreference::CoreMl,
        _ if use_gpu => AccelerationPreference::Auto,
        _ => AccelerationPreference::Cpu,
    };
    let plan = crate::platform::inference_plan(preference);
    if plan.backend != InferenceBackend::Cpu && backend_poisoned(model, plan.backend) {
        InferencePlan::cpu()
    } else {
        plan
    }
}

/// 加速后端运行时故障：按模型/后端隔离故障，重建 CPU 会话并重试一次。
/// CPU 会话失败或第二次失败原样返回，其他模型保持原后端。
pub(crate) fn run_with_acceleration_fallback<T>(
    use_gpu: bool,
    model: &str,
    mut run: impl FnMut() -> Result<T, String>,
    reset: impl FnOnce(),
) -> Result<T, String> {
    match run() {
        Ok(v) => Ok(v),
        Err(err) => {
            let backend = inference_plan(use_gpu, model).backend;
            if backend != crate::platform::InferenceBackend::Cpu {
                poison_backend(model, backend);
                eprintln!(
                    "{backend:?} 推理运行时故障（{model}），该模型本进程回落纯 CPU 并重建会话: {err}"
                );
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
/// 这是**语义通道**的批量化决策 → 按批推理实际用的 siglip vision 标签查。
pub(crate) fn prefer_batch_inference(use_gpu: bool, model: &str) -> bool {
    inference_plan(use_gpu, model).prefer_batch
}

// ---------------------------------------------------------------------------
// 毒化隔离测试缝（集成测试用；生产代码勿调）
// ---------------------------------------------------------------------------

/// 置某模型的 DML 毒化位（tests 毒化隔离真值表用）。
#[doc(hidden)]
#[allow(dead_code)] // 集成测试引用（lib 目标内无调用点）
pub fn poison_dml_for_test(model: &str) {
    poison_backend(model, crate::platform::InferenceBackend::DirectMl);
}

/// 清空全部毒化位（测试隔离：每个用例从干净态起步）。
#[doc(hidden)]
#[allow(dead_code)] // 集成测试引用（lib 目标内无调用点）
pub fn reset_dml_poison_for_test() {
    poison_flags()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
}

/// 某模型当前 DML 意图（tests 断言「A 毒化后 A=CPU、B 仍 DML」）。
#[doc(hidden)]
#[allow(dead_code)] // 集成测试引用（lib 目标内无调用点）
pub fn dml_intended_for_test(use_gpu: bool, model: &str) -> bool {
    inference_plan(use_gpu, model).backend == crate::platform::InferenceBackend::DirectMl
}

/// 直通 run_with_acceleration_fallback（tests 验证「失败一次 → 毒化该模型 + reset +
/// 重跑成功；其他模型不受连坐」——用假 run/reset 闭包当 EP/会话 seam）。
#[doc(hidden)]
#[allow(dead_code)] // 集成测试引用（lib 目标内无调用点）
pub fn run_with_dml_fallback_for_test<T>(
    use_gpu: bool,
    model: &str,
    run: impl FnMut() -> Result<T, String>,
    reset: impl FnOnce(),
) -> Result<T, String> {
    run_with_acceleration_fallback(use_gpu, model, run, reset)
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
    let plan = inference_plan(use_gpu, model_label);
    build_with_cpu_fallback(model_label, plan, |plan| {
        build_session_with_plan(path, intra_threads, model_label, plan)
    })
}

/// Provider registration and model compilation can fail before the first run.
/// Retry with a fresh CPU builder, preserving both errors if the model itself
/// is invalid. Only the failing model/provider pair is disabled.
fn build_with_cpu_fallback<T>(
    model_label: &str,
    plan: crate::platform::InferencePlan,
    mut build: impl FnMut(crate::platform::InferencePlan) -> Result<T, String>,
) -> Result<T, String> {
    use crate::platform::{InferenceBackend, InferencePlan};
    match build(plan) {
        Ok(session) => Ok(session),
        Err(error) if plan.backend == InferenceBackend::Cpu => Err(error),
        Err(acceleration_error) => {
            poison_backend(model_label, plan.backend);
            eprintln!(
                "{:?} 模型初始化失败（{model_label}），尝试 CPU: {acceleration_error}",
                plan.backend
            );
            build(InferencePlan::cpu())
                .map_err(|cpu_error| format!("{acceleration_error}; CPU 回退失败: {cpu_error}"))
        }
    }
}

fn build_session_with_plan(
    path: &std::path::Path,
    intra_threads: Option<usize>,
    model_label: &str,
    plan: crate::platform::InferencePlan,
) -> Result<ort::session::Session, String> {
    use ort::session::builder::GraphOptimizationLevel;
    let opt_level = match std::env::var("SMARTPHOTO_AI_OPT").as_deref() {
        Ok("level1") => GraphOptimizationLevel::Level1,
        Ok("disable") => GraphOptimizationLevel::Disable,
        _ => plan.optimization,
    };
    let mut builder = ort::session::Session::builder().map_err(|e| e.to_string())?;
    // 关闭 ORT 内存 arena（2026-09-29 内存审计项 B）：arena 只增不还，
    // 实测语义搜索后 commit 971MB vs 物理 342MB——关掉后分配走系统堆，
    // 随 Session drop 全额归还；推理速度影响个位数百分比（可接受）。
    builder = builder
        .with_config_entry("session.disable_mem_arena", "1")
        .map_err(|e| e.to_string())?;
    if let Some(n) = intra_threads {
        builder = builder.with_intra_threads(n).map_err(|e| e.to_string())?;
    }
    builder
        .with_optimization_level(opt_level)
        .map_err(|e| e.to_string())?
        .with_execution_providers(crate::platform::execution_providers(plan.backend))
        .map_err(|e| e.to_string())?
        .commit_from_file(path)
        .map_err(|e| format!("加载 {model_label} 失败: {e}"))
}

#[cfg(test)]
mod initialization_fallback_tests {
    use super::*;
    use crate::platform::{InferenceBackend, InferencePlan};

    fn accelerated() -> InferencePlan {
        InferencePlan {
            backend: InferenceBackend::CoreMl,
            ..InferencePlan::cpu()
        }
    }

    #[test]
    fn initialization_failure_retries_cpu_and_isolates_model_and_provider() {
        let model = "test-coreml-init-isolation";
        let mut backends = Vec::new();
        let result = build_with_cpu_fallback(model, accelerated(), |plan| {
            backends.push(plan.backend);
            if plan.backend == InferenceBackend::CoreMl {
                Err("CoreML model compilation failed".into())
            } else {
                Ok(42)
            }
        });
        assert_eq!(result, Ok(42));
        assert_eq!(backends, [InferenceBackend::CoreMl, InferenceBackend::Cpu]);
        assert!(backend_poisoned(model, InferenceBackend::CoreMl));
        assert!(!backend_poisoned(model, InferenceBackend::DirectMl));
        assert!(!backend_poisoned(
            "test-unaffected-model",
            InferenceBackend::CoreMl
        ));
    }

    #[test]
    fn cpu_initialization_failure_is_not_retried() {
        let mut calls = 0;
        let result: Result<(), String> =
            build_with_cpu_fallback("test-cpu-init", InferencePlan::cpu(), |_| {
                calls += 1;
                Err("bad model".into())
            });
        assert_eq!(result.unwrap_err(), "bad model");
        assert_eq!(calls, 1);
    }

    #[test]
    fn dual_failure_retains_diagnostics_without_retry_loop() {
        let mut calls = 0;
        let result: Result<(), String> =
            build_with_cpu_fallback("test-dual-init-failure", accelerated(), |plan| {
                calls += 1;
                Err(format!("{:?} failure", plan.backend))
            });
        let error = result.unwrap_err();
        assert!(error.contains("CoreMl failure") && error.contains("Cpu failure"));
        assert_eq!(calls, 2);
    }

    #[test]
    fn successful_accelerator_is_not_disabled_or_rebuilt() {
        let model = "test-successful-coreml-init";
        let mut calls = 0;
        assert_eq!(
            build_with_cpu_fallback(model, accelerated(), |_| {
                calls += 1;
                Ok::<_, String>(9)
            }),
            Ok(9)
        );
        assert_eq!(calls, 1);
        assert!(!backend_poisoned(model, InferenceBackend::CoreMl));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn corrupt_model_uses_real_session_builders_and_preserves_both_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("invalid.onnx");
        std::fs::write(&path, b"this is not an ONNX protobuf").unwrap();
        let model = "test-real-coreml-initialization-failure";
        let error = build_with_cpu_fallback(model, accelerated(), |plan| {
            build_session_with_plan(&path, Some(1), model, plan)
        })
        .expect_err("invalid model must fail with both providers");
        assert!(error.contains("CPU 回退失败"), "{error}");
        assert!(backend_poisoned(model, InferenceBackend::CoreMl));
        assert!(!backend_poisoned(model, InferenceBackend::Cpu));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn coreml_runtime_failure_falls_back_once_for_that_model() {
        if std::env::var_os("SMARTPHOTO_AI_EP").is_some() {
            return;
        }
        let model = "test-coreml-runtime-isolation";
        let mut calls = 0;
        let mut resets = 0;
        let result = run_with_acceleration_fallback(
            true,
            model,
            || {
                calls += 1;
                if calls == 1 {
                    Err("runtime failure".into())
                } else {
                    Ok(7)
                }
            },
            || resets += 1,
        );
        assert_eq!(result, Ok(7));
        assert_eq!((calls, resets), (2, 1));
        assert_eq!(inference_plan(true, model).backend, InferenceBackend::Cpu);
        assert_eq!(
            inference_plan(true, "test-other-coreml-model").backend,
            InferenceBackend::CoreMl
        );
    }
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
    /// [DirectML, CPU]（DML 失败自动落 CPU，见 build_session）。
    pub use_gpu: bool,
    /// 画质档位（settings.ai.quality_tier 投影，默认 normal）：推理层
    /// 经 face_detect_model_id / semantic_model_ids 解析当前档位的模型件
    /// 与检测源策略；切档后 settings_set 刷新快照 → 惰性重建会话。
    pub quality_tier: QualityTier,
}

impl Default for AiIndexParams {
    fn default() -> Self {
        Self {
            embed_input_size: 256,
            face_detect_threshold: 0.5,
            face_cluster_threshold: 0.4,
            use_gpu: true,
            quality_tier: QualityTier::Normal,
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
            tier: entry.tier.clone(),
        })
    }

    /// 全清单状态。
    pub fn status_all(&self) -> Vec<ModelStatusDto> {
        catalog()
            .iter()
            .filter_map(|e| self.status(e).ok())
            .collect()
    }

    /// 发起下载：全局已安装或已在队（同模型）直接返回；否则登记 + supervisor
    /// 后台执行。结果经 `aiModelDownloadFinished` 事件回报。
    pub fn download(&self, entry: ModelEntry) -> Result<(), String> {
        {
            let mut active = self.active.lock().expect("ai active mutex poisoned");
            if active.contains_key(&entry.id) {
                return Ok(()); // 去重：已在下载
            }
            // 成功下载经 SHA 校验后才落到最终路径。共用文件独立于库，
            // 重启或切换库后也无需再下载；与任务登记在同一锁内防重复请求。
            if fs::metadata(self.final_path(&entry.id))
                .is_ok_and(|meta| meta.is_file() && meta.len() == entry.bytes_total)
            {
                return Ok(());
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

    /// 任务收尾：除名 + 失败登记 + 事件。**取消不是失败**（2026-09-28
    /// 边界修复）：用户主动取消不进 `failed` set（status 回 idle，UI 不
    /// 显示失败徽标），事件照发 ok:false + error="下载已取消"（对外事件
    /// 名和字段不变）；仅真失败（网络/SHA）登记 failed。
    fn finish(&self, id: &str, outcome: DownloadOutcome) {
        self.active
            .lock()
            .expect("ai active mutex poisoned")
            .remove(id);
        let (ok, error) = match outcome {
            DownloadOutcome::Installed => (true, None),
            DownloadOutcome::Cancelled => {
                // 不登记 failed：前端刷新快照后回 idle
                (false, Some("下载已取消".into()))
            }
            DownloadOutcome::Failed(err) => {
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
    /// 返回三态 [`DownloadOutcome`]（取消与失败分开记账）。
    fn run_download(&self, entry: &ModelEntry) -> DownloadOutcome {
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
                    return DownloadOutcome::Cancelled;
                }
                match fetch_source(source, &part, entry, &active, &self.bus) {
                    FetchOutcome::Installed => return DownloadOutcome::Installed,
                    FetchOutcome::Cancelled => {
                        let _ = fs::remove_file(&part);
                        return DownloadOutcome::Cancelled;
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
                    return DownloadOutcome::Failed("SHA256 校验不匹配（已重试一次）".into());
                }
                sha_retried = true;
                continue; // 主源+镜像整体重来一轮
            }
            // 到这里必然两源网络级失败
            return DownloadOutcome::Failed("主源与镜像均不可用（网络失败）".into());
        }
    }
}

/// 下载终态（取消与失败分离，2026-09-28）：用户取消不进 failed set，
/// 状态机回 idle；失败才登记 failed（status 显示失败徽标）。
enum DownloadOutcome {
    /// 校验通过并落位。
    Installed,
    /// 用户取消（`.part` 已由 run_download 清理）。
    Cancelled,
    /// 真失败（网络/SHA），携带原因。
    Failed(String),
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
