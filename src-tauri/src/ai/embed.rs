//! 语义嵌入推理层（M4）：SigLIP2 双塔 ONNX（vision/text 分离导出，int8
//! 量化）+ HuggingFace WordPiece 分词器。
//!
//! ## 运行时选型（2026-09-21 DirectML 接入修订）
//! - `ort` 2.0.0-rc.13 + `directml` feature（构建期下载含 DML 的 ms 预编译
//!   库）。EP 序列见 [`super::build_session`]：`use_gpu=true`（默认）
//!   → `[DirectML, CPU]`，DML 注册/初始化失败 ort 自动回落 CPU EP；
//!   `use_gpu=false` → 纯 CPU；env `SMARTPHOTO_AI_EP=dml|cpu` 强制覆盖
//!   （基准 A/B / 现场诊断）。
//! - 会话全局唯一、互斥串行推理——ort rc.13 的 `Session::run` 收 `&mut
//!   self`（Rust 侧独占借用），共享会话的并发 Run 必须串行化。索引吞吐
//!   靠**批量化**（[`ModelManager::embed_images`]：B 图一次 Run）吃满
//!   intra-op 线程池 / GPU dispatch，而不是并发多 Run；预处理（解码+
//!   squash）在互斥锁**外**做，多 worker 可重叠。
//! - 预处理（preprocessor_config.json 实测 pin）：squash resize 256×256、
//!   RGB、`(px/127.5 - 1)`（mean/std = 0.5）、NCHW f32。图源复用 thumbs
//!   256 档缩略图（命中缓存零解码原图），再 squash 到正方形。
//! - 分词：tokenizer.json（WordPiece，vocab 256000）截断 64 token，
//!   input_ids/attention_mask int64。
//! - 输出 L2 归一化后 768 维（text_embeds / image_embeds），cos 相似度。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ort::session::Session;
use ort::value::Tensor;

use super::ModelManager;

/// SigLIP2 输出维度（base-patch16-256 双塔共享投影维度）。
pub const EMBED_DIM: usize = 768;
/// 文本最大 token 数（SigLIP2 max_position_embeddings = 64）。
const MAX_TEXT_TOKENS: usize = 64;

/// 嵌入器抽象：语义索引/搜索的数据面。真实现 = [`ModelManager`]（ONNX），
/// 测试用确定性随机桩（tests/ai_embed_test.rs）。
pub trait SemanticEmbedder: Send + Sync {
    /// Whether this embedder benefits from batching; CPU/stub implementations default to single images.
    fn prefer_batch(&self, _model: &str) -> bool {
        false
    }

    /// 图像嵌入：`src` 原图 + `db_dir`（256 档缩略图缓存根，命中免解码原图）。
    fn embed_image(&self, src: &Path, db_dir: &Path) -> Result<Vec<f32>, String>;
    /// 批量图像嵌入（索引回填主路径，2026-09-21 并发优化）：多图 squash
    /// 预处理在推理锁外做，组 (B,3,H,W) 张量单次 Run——CPU intra-op 线程
    /// 与 GPU dispatch 都按批吃满（实测 2-4x）。默认逐图实现（桩/兼容），
    /// 真实现见 ModelManager。
    fn embed_images(&self, srcs: &[PathBuf], db_dir: &Path) -> Result<Vec<Vec<f32>>, String> {
        srcs.iter()
            .map(|src| self.embed_image(src, db_dir))
            .collect()
    }
    /// 文本嵌入。
    fn embed_text(&self, text: &str) -> Result<Vec<f32>, String>;
}

/// 全局推理槽：三件套惰性加载（模型未下载时命令报明确错误，下载后自动可用）。
/// visual/text 会话记录对应模型 id：三档画质切档（normal↔accurate 换
/// int8↔fp16 件）后不匹配即弃缓存会话重载（tokenizer 各档共享不换）。
#[derive(Default)]
struct InferSlots {
    visual: Option<Session>,
    visual_model: Option<String>,
    text: Option<Session>,
    text_model: Option<String>,
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

    /// 语义检索三件套是否齐备（**按当前画质档位**：normal/fast = int8 双塔
    /// + tokenizer；accurate = fp16 双塔 + tokenizer。缺当前档位件时
    /// ready=false——切回已装档位即恢复，不强制下载全量件）。
    pub fn semantic_models_ready(&self) -> bool {
        let [visual, text] = super::semantic_model_ids(self.ai_params().quality_tier);
        [visual, text, "siglip2-tokenizer"]
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

    /// 惰性加载 vision 会话（模型按当前档位解析 int8/fp16；切档后
    /// visual_model 不匹配即弃缓存重载。模型缺失 → 明确错误；EP 序列按
    /// use_gpu/env，毒化位按模型 id 隔离）。
    fn ensure_visual(&self, slots: &mut InferSlots) -> Result<(), String> {
        let model_id = super::semantic_model_ids(self.ai_params().quality_tier)[0];
        if slots.visual.is_some() && slots.visual_model.as_deref() == Some(model_id) {
            return Ok(());
        }
        let path = self.model_path(model_id);
        if !path.is_file() {
            return Err(format!("模型 {model_id} 未下载（设置页下载后再试）"));
        }
        let session = super::build_session(
            &path,
            Some(Self::embed_intra_threads()),
            self.ai_params().use_gpu,
            model_id,
        )?;
        slots.visual = Some(session);
        slots.visual_model = Some(model_id.to_string());
        Ok(())
    }

    /// 惰性加载 text 会话 + 分词器（text 按档位解析；tokenizer 共享）。
    fn ensure_text(&self, slots: &mut InferSlots) -> Result<(), String> {
        let model_id = super::semantic_model_ids(self.ai_params().quality_tier)[1];
        if slots.text.is_none() || slots.text_model.as_deref() != Some(model_id) {
            let path = self.model_path(model_id);
            if !path.is_file() {
                return Err(format!("模型 {model_id} 未下载（设置页下载后再试）"));
            }
            let session = super::build_session(
                &path,
                Some(Self::embed_intra_threads()),
                self.ai_params().use_gpu,
                model_id,
            )?;
            slots.text = Some(session);
            slots.text_model = Some(model_id.to_string());
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

impl ModelManager {
    /// 缩略图 → squash 预处理张量行（NCHW f32，(px/127.5 - 1)）。
    /// 在推理互斥锁**外**调用——多 worker 预处理与他人的 Run 重叠。
    /// squash resize 到 input_size²（SigLIP2 预处理：非保比，直接缩放；
    /// onnx-community 导出为动态 batch/H/W，改档位即改嵌入——需重建语义索引）。
    fn preprocess_thumb(&self, thumb_path: &Path) -> Result<Vec<f32>, String> {
        let input_size = self.ai_params().embed_input_size;
        let dim = u32::from(input_size);
        let img = image::ImageReader::open(thumb_path)
            .ok()
            .and_then(|r| r.decode().ok())
            .ok_or_else(|| format!("缩略图解码失败: {}", thumb_path.display()))?
            .to_rgb8();
        let img = image::imageops::resize(&img, dim, dim, image::imageops::FilterType::Triangle);
        let mut data = Vec::with_capacity(3 * input_size as usize * input_size as usize);
        for ch in 0..3 {
            for px in img.pixels() {
                data.push((px.0[ch] as f32 / 127.5) - 1.0);
            }
        }
        Ok(data)
    }

    /// 批量张量单次 Run → 逐行提取 L2 归一化嵌入（输出 rank-2 [B,768]
    /// 直接行切；rank-3 为 last_hidden 兜底——按批逐行 mean-pool）。
    /// DML 运行时故障（如 int8 LayerNormFusion E_INVALIDARG，真机
    /// 2026-09-21）经 run_with_acceleration_fallback 一次性回落纯 CPU 重跑。
    fn embed_batched(&self, batch: usize, data: Vec<f32>) -> Result<Vec<Vec<f32>>, String> {
        let input_size = u32::from(self.ai_params().embed_input_size);
        let dim = i64::from(input_size);
        let input_shape = vec![batch as i64, 3, dim, dim];
        let use_gpu = self.ai_params().use_gpu;
        // 锁内完成 run + 张量提取（SessionOutputs 借用会话，不可出锁）
        let extract = || -> Result<(Vec<i64>, Vec<f32>), String> {
            let tensor = Tensor::from_array((input_shape.clone(), data.clone()))
                .map_err(|e| format!("构造图像张量失败: {e}"))?;
            let mut slots = slots().lock().expect("infer slots mutex poisoned");
            self.ensure_visual(&mut slots)?;
            let session = slots.visual.as_mut().expect("ensure_visual 已保证");
            let out_name =
                pick_embed_name(session, &["image_embeds", "pooler", "embeds", "hidden"])?;
            let outputs = session
                .run(ort::inputs![input_name_pixel_values(session) => tensor])
                .map_err(|e| format!("vision 推理失败: {e}"))?;
            let value = outputs
                .get(out_name.as_str())
                .ok_or_else(|| format!("输出 {out_name} 不存在"))?;
            let (shape, flat) = value
                .try_extract_tensor::<f32>()
                .map_err(|e| format!("输出 {out_name} 解析失败: {e}"))?;
            Ok((shape.to_vec(), flat.to_vec()))
        };
        let reset = || {
            let mut slots = slots().lock().expect("infer slots mutex poisoned");
            slots.visual = None; // 丢弃 DML 会话：重建走 execution_providers 的 CPU 分支
        };
        let vision_model = super::semantic_model_ids(self.ai_params().quality_tier)[0];
        let (shape, flat) =
            super::run_with_acceleration_fallback(use_gpu, vision_model, extract, reset)?;
        let (rows, dim_out) = match shape.len() {
            // [B, 768]：整块按行切
            2 => (shape[0] as usize, shape[1] as usize),
            // [B, seq, 768]（last_hidden 兜底）：无 mask 语义，等权 mean-pool
            3 => (shape[0] as usize, shape[1] as usize * shape[2] as usize),
            n => return Err(format!("vision 输出形状不支持: rank {n} {shape:?}")),
        };
        if flat.len() != rows * dim_out || rows != batch {
            return Err(format!(
                "vision 输出行数与批不符（期望 {batch}，得 {rows}；shape {shape:?}）"
            ));
        }
        if shape.len() == 3 {
            // rank-3：批内逐样本 mean-pool 后归一化
            let seq = shape[1] as usize;
            let d = shape[2] as usize;
            let mut out = Vec::with_capacity(rows);
            for r in 0..rows {
                let mut pooled = vec![0f32; d];
                for t in 0..seq {
                    for (k, x) in pooled.iter_mut().enumerate() {
                        *x += flat[r * seq * d + t * d + k];
                    }
                }
                for x in &mut pooled {
                    *x /= seq as f32;
                }
                out.push(Self::normalize(pooled));
            }
            return Ok(out);
        }
        Ok((0..rows)
            .map(|r| Self::normalize(flat[r * dim_out..(r + 1) * dim_out].to_vec()))
            .collect())
    }
}

impl SemanticEmbedder for ModelManager {
    fn prefer_batch(&self, model: &str) -> bool {
        super::prefer_batch_inference(self.ai_params().use_gpu, model)
    }

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
        let data = self.preprocess_thumb(Path::new(&thumb_path))?;
        Ok(self.embed_batched(1, data)?.pop().expect("批 1 必有一行"))
    }

    fn embed_images(&self, srcs: &[PathBuf], db_dir: &Path) -> Result<Vec<Vec<f32>>, String> {
        if srcs.is_empty() {
            return Ok(Vec::new());
        }
        let input_size = self.ai_params().embed_input_size;
        // 缩略图备齐（命中缓存毫秒级；缺失顺手生成）+ 预处理（锁外）
        let unit = 3 * usize::from(input_size) * usize::from(input_size);
        let mut data = Vec::with_capacity(unit * srcs.len());
        for src in srcs {
            let thumb_path =
                crate::thumbs::thumb_file(db_dir, src, input_size).ok_or_else(|| {
                    format!(
                        "缩略图生成失败（RAW 预览提取失败或已损坏）: {}",
                        src.display()
                    )
                })?;
            data.extend_from_slice(&self.preprocess_thumb(Path::new(&thumb_path))?);
        }
        self.embed_batched(srcs.len(), data)
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
