//! 构建脚本：Tauri 常规构建 + ffmpeg 侧车的 dev 可用性。
//!
//! externalBin（binaries/ffmpeg-x86_64-pc-windows-msvc.exe）只在 `tauri
//! build` 打包时由 bundler 落位；`cargo run` / `cargo test` 产物没有这一步
//! ——这里把它拷到 target/<profile>/ffmpeg.exe（裸名，与安装包落位名一致），
//! 运行时解析函数 `videos::ffmpeg_path()` 先查 current_exe().parent() 即可
//! 命中。侧车缺失（新克隆未跑构建脚本）时静默跳过：海报提取优雅降级，
//! 构建不失败（scripts/build-installer.cmd 负责下载补齐）。

use std::env;
use std::fs;
use std::path::Path;

fn main() {
    copy_ffmpeg_sidecar_for_dev();
    tauri_build::build();
}

/// target/<profile>/ 推导：OUT_DIR = target/<profile>/build/<crate>-<hash>。
fn copy_ffmpeg_sidecar_for_dev() {
    let Some(out_dir) = env::var_os("OUT_DIR") else {
        return;
    };
    let Ok(target) = env::var("TARGET") else {
        return;
    };
    let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") else {
        return;
    };
    let Some(profile_dir) = Path::new(&out_dir).ancestors().nth(3) else {
        return;
    };
    let src = Path::new(&manifest_dir)
        .join("binaries")
        .join(format!("ffmpeg-{target}.exe"));
    if !src.is_file() {
        // 未下载：不阻断构建（海报优雅降级；见模块头注释）
        println!("cargo:rerun-if-changed=binaries");
        return;
    }
    let dst = profile_dir.join("ffmpeg.exe");
    let needs_copy = match (fs::metadata(&src), fs::metadata(&dst)) {
        (Ok(s), Ok(d)) => s.len() != d.len() || s.modified().ok() > d.modified().ok(),
        (Ok(_), Err(_)) => true,
        _ => false,
    };
    if needs_copy {
        let _ = fs::copy(&src, &dst);
    }
    println!("cargo:rerun-if-changed=binaries/ffmpeg-{target}.exe");
}
