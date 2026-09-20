//! 语义嵌入推理层（M4）：SigLIP2 双塔 ONNX（vision/text 分离导出，int8
//! 量化）+ HuggingFace WordPiece 分词器。
//!
//! ## 运行时选型（用户定案 2026-09-19 落地说明）
//! - `ort` 2.0.0-rc.13，默认 feature = 构建期下载 ONNX Runtime 预编译库 +
//!   **CPU EP**。DirectML：启用 `directml` feature 后在 [`ensure_sessions`]
//!   的 `apply_gpu_ep` 分支注册 DML EP（`use_gpu=true` 时先试 DML，注册/
//!   初始化失败自动落 CPU 并发 AppError 提示）——v1 该 feature 未开，
//!   分支保留为 M4 后期开关。
//! - 会话全局唯一、互斥串行推理（CPU EP 单会话已吃满 permit 级并行，
//!   串行化避免争抢；GPU 通道接入后按 kind 分流）。
//! - 预处理（preprocessor_config.json 实测 pin）：squash resize 256×256、
//!   RGB、`(px/127.5 - 1)`（mean/std = 0.5）、NCHW f32。图源复用 thumbs
//!   256 档缩略图（命中缓存零解码原图），再 squash 到正方形。
//! - 分词：tokenizer.json（WordPiece，vocab 256000）截断 64 token，
//!   input_ids/attention_mask int64。
//! - 输出 L2 归一化后 768 维（text_embeds / image_embeds），cos 相似度。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;

use super::ModelManager;

/// SigLIP2 输出维度（base-patch16-256 双塔共享投影维度）。
pub const EMBED_DIM: usize = 768;
/// 文本最大 token 数（SigLIP2 max_position_embeddings = 64）。
const MAX_TEXT_TOKENS: usize = 64;

/// 嵌入器抽象：语义索引/搜索的数据面。真实现 = [`ModelManager`]（ONNX），
/// 测试用确定性随机桩（tests/ai_embed_test.rs）。
pub trait SemanticEmbedder: Send + Sync {
    /// 图像嵌入：`src` 原图 + `db_dir`（256 档缩略图缓存根，命中免解码原图）。
    fn embed_image(&self, src: &Path, db_dir: &Path) -> Result<Vec<f32>, String>;
    /// 文本嵌入。
    fn embed_text(&self, text: &str) -> Result<Vec<f32>, String>;
}

/// 全局推理槽：三件套惰性加载（模型未下载时命令报明确错误，下载后自动可用）。
#[derive(Default)]
struct InferSlots {
    visual: Option<Session>,
    text: Option<Session>,
    tokenizer: Option<tokenizers::Tokenizer>,
}

fn slots() -> &'static Mutex<InferSlots> {
    static SLOTS: std::sync::OnceLock<Mutex<InferSlots>> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| Mutex::new(InferSlots::default()))
}

impl ModelManager {
    /// 模型文件落位名（ModelManager 下载管线的最终文件）。
    pub fn model_path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.onnx"))
    }

    /// 语义检索三件套是否齐备（visual + text + tokenizer 均已安装）。
    pub fn semantic_models_ready(&self) -> bool {
        ["siglip2-visual", "siglip2-text", "siglip2-tokenizer"]
            .iter()
            .all(|id| self.model_path(id).is_file())
    }

    /// 语义检索是否可用（模型齐备；enable_clip 由调用方按需叠加）。
    pub fn semantic_ready(&self) -> bool {
        self.semantic_models_ready()
    }
}

impl ModelManager {
    /// ort 会话 intra-op 线程数：≈核心数/4（AI worker 封顶 4，见
    /// semantic::worker_count_for_ai；多 worker × 全核会话会平方级超订阅，
    /// 表现为索引"在跑但极慢"）。搜索单查询用同会话，核心数/4 对 256px
    /// 小模型延迟足够。
    fn embed_intra_threads() -> usize {
        (std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            / 4)
        .clamp(1, 8)
    }

    /// 惰性加载 vision 会话（模型缺失 → 明确错误）。
    fn ensure_visual(&self, slots: &mut InferSlots) -> Result<(), String> {
        if slots.visual.is_some() {
            return Ok(());
        }
        let path = self.model_path("siglip2-visual");
        if !path.is_file() {
            return Err("模型 siglip2-visual 未下载（设置页下载后再试）".into());
        }
        let session = Session::builder()
            .map_err(|e| e.to_string())?
            .with_intra_threads(Self::embed_intra_threads())
            .map_err(|e| e.to_string())?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| e.to_string())?
            .commit_from_file(&path)
            .map_err(|e| format!("加载 siglip2-visual 失败: {e}"))?;
        slots.visual = Some(session);
        Ok(())
    }

    /// 惰性加载 text 会话 + 分词器。
    fn ensure_text(&self, slots: &mut InferSlots) -> Result<(), String> {
        if slots.text.is_none() {
            let path = self.model_path("siglip2-text");
            if !path.is_file() {
                return Err("模型 siglip2-text 未下载（设置页下载后再试）".into());
            }
            let session = Session::builder()
                .map_err(|e| e.to_string())?
                .with_intra_threads(Self::embed_intra_threads())
                .map_err(|e| e.to_string())?
                .with_optimization_level(GraphOptimizationLevel::Level3)
                .map_err(|e| e.to_string())?
                .commit_from_file(&path)
                .map_err(|e| format!("加载 siglip2-text 失败: {e}"))?;
            slots.text = Some(session);
        }
        if slots.tokenizer.is_none() {
            let path = self.model_path("siglip2-tokenizer");
            if !path.is_file() {
                return Err("分词器 siglip2-tokenizer 未下载（设置页下载后再试）".into());
            }
            let mut tokenizer = tokenizers::Tokenizer::from_file(&path)
                .map_err(|e| format!("加载分词器失败: {e}"))?;
            let params = tokenizers::TruncationParams {
                max_length: MAX_TEXT_TOKENS,
                ..Default::default()
            };
            tokenizer
                .with_truncation(Some(params))
                .map_err(|e| format!("设置截断失败: {e}"))?;
            slots.tokenizer = Some(tokenizer);
        }
        Ok(())
    }

    /// L2 归一化（双塔嵌入统一后处理，cos 相似度=点积）。
    fn normalize(mut v: Vec<f32>) -> Vec<f32> {
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > f32::EPSILON {
            for x in &mut v {
                *x /= norm;
            }
        }
        v
    }
}

impl SemanticEmbedder for ModelManager {
    fn embed_image(&self, src: &Path, db_dir: &Path) -> Result<Vec<f32>, String> {
        // 图源 = embed_input_size 档缩略图（settings.ai 可配，默认 256；
        // 命中缓存零解码原图；缺失则顺手生成——与缩略图索引任务协同）
        let input_size = self.ai_params().embed_input_size;
        let thumb_path = crate::thumbs::thumb_file(db_dir, src, input_size).ok_or_else(|| {
            format!(
                "缩略图生成失败（RAW 预览提取失败或已损坏）: {}",
                src.display()
            )
        })?;
        let mut slots = slots().lock().expect("infer slots mutex poisoned");
        self.ensure_visual(&mut slots)?;
        let session = slots.visual.as_mut().expect("ensure_visual 已保证");

        // squash resize 到 input_size²（SigLIP2 预处理：非保比，直接缩放；
        // onnx-community 导出为动态 H/W，改档位即改嵌入——需重建语义索引）
        let img = image::ImageReader::open(&thumb_path)
            .ok()
            .and_then(|r| r.decode().ok())
            .ok_or_else(|| format!("缩略图解码失败: {thumb_path}"))?
            .to_rgb8();
        let dim = u32::from(input_size);
        let img = image::imageops::resize(&img, dim, dim, image::imageops::FilterType::Triangle);
        // NCHW f32，(px/127.5 - 1)
        let mut data = Vec::with_capacity(3 * input_size as usize * input_size as usize);
        for ch in 0..3 {
            for px in img.pixels() {
                data.push((px.0[ch] as f32 / 127.5) - 1.0);
            }
        }
        let tensor = Tensor::from_array((vec![1i64, 3, dim as i64, dim as i64], data))
            .map_err(|e| format!("构造图像张量失败: {e}"))?;
        let out_name = pick_embed_name(session, &["image_embeds", "pooler", "embeds", "hidden"])?;
        let outputs = session
            .run(ort::inputs![input_name_pixel_values(session) => tensor])
            .map_err(|e| format!("vision 推理失败: {e}"))?;
        Ok(Self::normalize(pick_embed(
            &outputs,
            out_name.as_str(),
            None,
        )?))
    }

    fn embed_text(&self, text: &str) -> Result<Vec<f32>, String> {
        let mut slots = slots().lock().expect("infer slots mutex poisoned");
        self.ensure_text(&mut slots)?;
        let tokenizer = slots.tokenizer.as_ref().expect("ensure_text 已保证");
        let encoding = tokenizer
            .encode(text, true)
            .map_err(|e| format!("分词失败: {e}"))?;
        let ids: Vec<i64> = encoding.get_ids().iter().map(|v| i64::from(*v)).collect();
        let mask: Vec<i64> = encoding
            .get_attention_mask()
            .iter()
            .map(|v| i64::from(*v))
            .collect();
        let seq = ids.len() as i64;
        let session = slots.text.as_mut().expect("ensure_text 已保证");

        let ids_t = Tensor::from_array((vec![1i64, seq], ids))
            .map_err(|e| format!("构造 ids 张量失败: {e}"))?;
        let mask_t = Tensor::from_array((vec![1i64, seq], mask))
            .map_err(|e| format!("构造 mask 张量失败: {e}"))?;
        let has_mask = session
            .inputs()
            .iter()
            .any(|i| i.name().to_ascii_lowercase().contains("attention"));
        let out_name = pick_embed_name(session, &["text_embeds", "pooler", "embeds", "hidden"])?;
        let outputs = if has_mask {
            session.run(ort::inputs![
                input_name_input_ids(session) => ids_t,
                input_name_attention_mask(session) => mask_t,
            ])
        } else {
            session.run(ort::inputs![input_name_input_ids(session) => ids_t])
        }
        .map_err(|e| format!("text 推理失败: {e}"))?;
        let mask = encoding.get_attention_mask().to_vec();
        Ok(Self::normalize(pick_embed(
            &outputs,
            out_name.as_str(),
            Some(&mask),
        )?))
    }
}

/// 输入名匹配（ONNX 导出命名基本固定，宽松 contains 兜底不同导出版本）。
fn find_input(session: &Session, contains: &str) -> String {
    session
        .inputs()
        .iter()
        .map(|i| i.name().to_string())
        .find(|n| n.to_ascii_lowercase().contains(contains))
        .unwrap_or_else(|| contains.to_string())
}

fn input_name_pixel_values(session: &Session) -> String {
    find_input(session, "pixel")
}

fn input_name_input_ids(session: &Session) -> String {
    find_input(session, "input_ids")
}

fn input_name_attention_mask(session: &Session) -> String {
    find_input(session, "attention")
}

/// 按名称优先级解析嵌入输出名（run 前调用，避免与 run 的可变借用冲突）。
fn pick_embed_name(session: &Session, prefer_contains: &[&str]) -> Result<String, String> {
    let names: Vec<String> = session
        .outputs()
        .iter()
        .map(|o| o.name().to_string())
        .collect();
    for cand in prefer_contains {
        if let Some(name) = names.iter().find(|n| n.to_ascii_lowercase().contains(cand)) {
            return Ok(name.clone());
        }
    }
    Err(format!(
        "模型缺少可用的嵌入输出 {prefer_contains:?}（现有: {names:?}）"
    ))
}

/// 从输出里提取嵌入向量：rank-2 直接取；rank-3（last_hidden_state
/// [1, seq, 768]）做 mean-pool（提供 mask 时按 mask 加权——文本最后
/// 一层隐藏态的兜底路径）。
fn pick_embed(
    outputs: &ort::session::SessionOutputs<'_>,
    name: &str,
    attention_mask: Option<&[u32]>,
) -> Result<Vec<f32>, String> {
    let value = outputs
        .get(name)
        .ok_or_else(|| format!("输出 {name} 不存在"))?;
    let (shape, data) = value
        .try_extract_tensor::<f32>()
        .map_err(|e| format!("输出 {name} 解析失败: {e}"))?;
    if shape.len() == 2 {
        return Ok(data.to_vec());
    }
    if shape.len() == 3 {
        // [1, seq, dim]：mean-pool（mask 提供时加权）
        let seq = shape[1] as usize;
        let dim = shape[2] as usize;
        if data.len() != seq * dim {
            return Err(format!("输出 {name} 形状异常 {shape:?}"));
        }
        let mut out = vec![0f32; dim];
        let mut weight_sum = 0f32;
        for t in 0..seq {
            let w = match attention_mask {
                Some(mask) if t < mask.len() => mask[t] as f32,
                _ => 1.0,
            };
            weight_sum += w;
            for d in 0..dim {
                out[d] += data[t * dim + d] * w;
            }
        }
        if weight_sum > 0.0 {
            for x in &mut out {
                *x /= weight_sum;
            }
        }
        return Ok(out);
    }
    Err(format!("输出 {name} 形状不支持: {shape:?}"))
}
