//! pHash 感知哈希（M6 连拍分组的场景链因子）：256 档缩略图 → 灰度 →
//! 32×32 → DCT-II → 左上 8×8（去 DC）→ 64-bit。
//!
//! ## 算法（经典 pHash，中值阈值法）
//! - 灰度缩到 32×32 → 二维 DCT-II（可分离：先行后列，cos 表预计算）；
//! - 取左上 8×8 低频块，**去掉 DC [0][0]** 剩 63 个系数；
//! - 阈值取 63 系数的**中值**（比均值法对亮度/对比度整体偏移更稳）；
//! - bit_i = (coef_i > median)，63 位 + 1 位空闲（高位补 0）→ u64。
//! - 相似度 = 汉明距离 `popcount(a ^ b)`：同场景平移/重压缩典型 ≤10，
//!   不同场景典型 >20（阈值经 settings.ai.burst_hamming_max 可调）。
//!
//! DCT 手写不引 rustdct：32×32 可分离变换 ~65k 次乘加，微秒级；省一个
//! 依赖（phash 精度对 DCT 归一化系数不敏感——中值阈值下归一化只做
//! 正数缩放，不改变相对序）。
//!
//! 输入图源 = 已缓存 256 档缩略图（`thumb_file(db_dir, src, 256)`，缺失
//! 顺手生成——与缩略图索引任务协同），计算毫秒级、全核可并行。

use std::path::Path;

/// DCT 边长。
const N: usize = 32;
/// 取的低频块边长。
const BLOCK: usize = 8;

/// cos 表（懒初始化 OnceLock；[u][x] = cos((2x+1)·u·π / (2N))）。
fn cos_table() -> &'static [[f32; N]; N] {
    static TABLE: std::sync::OnceLock<[[f32; N]; N]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [[0f32; N]; N];
        for (u, row) in table.iter_mut().enumerate() {
            for (x, slot) in row.iter_mut().enumerate() {
                let angle =
                    (2.0 * x as f32 + 1.0) * u as f32 * std::f32::consts::PI / (2.0 * N as f32);
                *slot = angle.cos();
            }
        }
        table
    })
}

/// 灰度图 → pHash（64-bit）。图像尺寸任意（内部先缩 32×32）。
pub fn phash_of_gray(img: &image::GrayImage) -> u64 {
    // 缩放到 N×N（非保比 squash——pHash 约定，宽高比信息舍弃）
    let small = image::imageops::resize(
        img,
        N as u32,
        N as u32,
        image::imageops::FilterType::Triangle,
    );
    let mut f = [[0f32; N]; N];
    for (y, row) in f.iter_mut().enumerate() {
        for (x, slot) in row.iter_mut().enumerate() {
            *slot = f32::from(small.get_pixel(x as u32, y as u32).0[0]);
        }
    }
    // 可分离 DCT-II：先行（x→u）后列（y→v）
    let table = cos_table();
    let mut rows = [[0f32; N]; N];
    for y in 0..N {
        for u in 0..N {
            let mut sum = 0f32;
            for (x, fx) in f[y].iter().enumerate() {
                sum += fx * table[u][x];
            }
            rows[y][u] = sum;
        }
    }
    let mut dcts = [[0f32; N]; N];
    for v in 0..N {
        for u in 0..N {
            let mut sum = 0f32;
            for (y, row) in rows.iter().enumerate() {
                sum += row[u] * table[v][y];
            }
            dcts[v][u] = sum;
        }
    }
    // 左上 8×8 去 DC → 63 系数 → 中值阈值
    let mut coefs: Vec<f32> = Vec::with_capacity(BLOCK * BLOCK - 1);
    for (v, row) in dcts.iter().enumerate().take(BLOCK) {
        for (u, coef) in row.iter().enumerate().take(BLOCK) {
            if u == 0 && v == 0 {
                continue; // DC：携带整体亮度，无判别力
            }
            coefs.push(*coef);
        }
    }
    let mut sorted = coefs.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let median = sorted[sorted.len() / 2];
    let mut hash: u64 = 0;
    for (bit, coef) in coefs.into_iter().enumerate() {
        if coef > median {
            hash |= 1 << bit;
        }
    }
    hash
}

/// 从资产文件算 pHash（图源 = 256 档缩略图缓存，缺失先生成）。
/// 返回 None = 解码失败（worker 按 attempts 策略重试）。
pub fn compute_phash(db_dir: &Path, src: &Path) -> Option<u64> {
    let thumb = crate::thumbs::thumb_file(db_dir, src, 256)?;
    let img = image::ImageReader::open(&thumb)
        .ok()?
        .decode()
        .ok()?
        .to_luma8();
    Some(phash_of_gray(&img))
}

/// 汉明距离（popcount；std 内建）。
pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}
