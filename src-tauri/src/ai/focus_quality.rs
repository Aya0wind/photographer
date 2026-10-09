//! Lightweight regional focus suggestions. These are transparent edge/texture
//! heuristics, not a calibrated defocus-model probability or a rejection rule.
use image::GrayImage;

pub struct Metric {
    pub score: Option<f64>,
    pub state: &'static str,
    pub reason: Option<&'static str>,
}

pub fn measure(gray: &GrayImage, threshold: f64) -> Metric {
    let (w, h) = (gray.width() as usize, gray.height() as usize);
    if w < 8 || h < 8 {
        return Metric {
            score: None,
            state: "unknown",
            reason: Some("region_pixels"),
        };
    }
    let p = gray.as_raw();
    let mut hist = [0usize; 256];
    for value in p {
        hist[*value as usize] += 1;
    }
    let percentile = |fraction: f64| {
        let mut sum = 0usize;
        for (v, count) in hist.iter().enumerate() {
            sum += count;
            if sum as f64 >= p.len() as f64 * fraction {
                return v;
            }
        }
        255
    };
    if percentile(0.95).saturating_sub(percentile(0.05)) < 16 {
        return Metric {
            score: None,
            state: "unknown",
            reason: Some("low_texture"),
        };
    }
    let mut energy = 0u64;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            let gx = -i64::from(p[i - w - 1]) - 2 * i64::from(p[i - 1]) - i64::from(p[i + w - 1])
                + i64::from(p[i - w + 1])
                + 2 * i64::from(p[i + 1])
                + i64::from(p[i + w + 1]);
            let gy = -i64::from(p[i - w - 1]) - 2 * i64::from(p[i - w]) - i64::from(p[i - w + 1])
                + i64::from(p[i + w - 1])
                + 2 * i64::from(p[i + w])
                + i64::from(p[i + w + 1]);
            energy += (gx * gx + gy * gy) as u64;
        }
    }
    let gradient = energy as f64 / ((w - 2) * (h - 2)) as f64 / 16.0;
    if gradient < 9.0 {
        return Metric {
            score: None,
            state: "unknown",
            reason: Some("low_texture"),
        };
    }
    let lap = super::selection::laplacian_variance(gray).unwrap_or(0.0);
    let ratio = lap / (gradient + 1.0);
    let score = super::selection::normalize_blur_score(lap);
    let state = if score >= threshold && ratio >= 0.08 {
        "sharp"
    } else if score < threshold && ratio < 0.08 {
        "soft"
    } else {
        "maybe"
    };
    Metric {
        score: Some(score),
        state,
        reason: Some("focus_heuristic"),
    }
}

/// Representative textured regions; ignore the softest backgrounds. A sharp
/// detail somewhere is described as detail clarity, never guaranteed subject focus.
pub fn detail_regions(gray: &GrayImage, threshold: f64) -> Vec<([f64; 4], Metric)> {
    let mut regions = Vec::new();
    for row in 0..3 {
        for col in 0..3 {
            let x = gray.width() * col / 3;
            let y = gray.height() * row / 3;
            let w = gray.width() * (col + 1) / 3 - x;
            let h = gray.height() * (row + 1) / 3 - y;
            let crop = image::imageops::crop_imm(gray, x, y, w, h).to_image();
            let metric = measure(&crop, threshold);
            if metric.score.is_some() {
                regions.push((
                    [
                        x as f64 / gray.width() as f64,
                        y as f64 / gray.height() as f64,
                        w as f64 / gray.width() as f64,
                        h as f64 / gray.height() as f64,
                    ],
                    metric,
                ));
            }
        }
    }
    regions.sort_by(|a, b| {
        b.1.score
            .unwrap_or(0.0)
            .total_cmp(&a.1.score.unwrap_or(0.0))
    });
    regions.truncate(3);
    regions
}
