//! 单文件单遍读取流水线（引擎工作线程侧）：读流 → xxh64 哈希 → `.part`
//! 暂存（主/第二目的地双写）→ head 截存 → 类型识别/EXIF → 长度校验。
//! 只做源读取/哈希/写盘，不碰 SQLite；收集端（mod.rs）串行结算。

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use xxhash_rust::xxh64::Xxh64;

use crate::devices::{classify, DeviceError, DeviceSource, FileEntry};
use crate::events::AssetKind;
use crate::import::templates::{
    render_dir, render_name, resolve_captured, RenderCtx, TemplateError,
};
use crate::metadata::exif_lite::{self, MetaLite};
use crate::settings::DuplicatePolicy;

use super::PART_DIR;

/// 单遍读取的块大小（spec §5.2：8MB 缓冲）。
const CHUNK: usize = 8 * 1024 * 1024;
/// 头部截存上限：EXIF-lite + 魔数识别只需文件头。
const HEAD_MAX: usize = 1024 * 1024;
/// {相机}/{镜头} 上下文缺失时的降级默认段。
const FALLBACK_CAMERA: &str = "未知相机";
const FALLBACK_LENS: &str = "未知镜头";

/// 工作线程产出。
pub(super) enum FileOutcome {
    Copied(Box<CopiedFile>),
    /// 免下载预跳：Skip/Ask 策略下目标路径已存在（与收集端 ③ 同判据提前），
    /// 只读了头段（EXIF 渲染目标路径必需），无 .part/无哈希。
    Skipped {
        entry: FileEntry,
    },
    Failed {
        entry: FileEntry,
        error: String,
    },
    /// 设备失联（DeviceError::Disconnected / 传输中断）：触发自动暂停。
    Disconnected {
        entry: FileEntry,
        error: String,
    },
}

/// 已复制完成（写满 + 长度校验通过）的文件产物；落位/查重/入库由收集端完成。
pub(super) struct CopiedFile {
    /// 同卷 rename 快道产物：源已被 rename 成 .part（收集端**跳过删源**；
    /// xxh 为 0 哨兵 = 待后台哈希通道补算，精确查重层随之跳过）。
    pub(super) fast_moved: bool,
    pub(super) entry: FileEntry,
    pub(super) kind: AssetKind,
    pub(super) meta: MetaLite,
    pub(super) xxh: u64,
    /// 暂存 .part 路径（已写满、长度已校验）。
    pub(super) part: PathBuf,
    /// 渲染出的最终路径（收集端做冲突处理后 rename）。
    pub(super) dst: PathBuf,
    /// F2 第二目的地：暂存 .part 与最终路径（无第二目的地为 None）。
    pub(super) part2: Option<PathBuf>,
    pub(super) dst2: Option<PathBuf>,
}

impl CopiedFile {
    /// 丢弃暂存 .part（跳过/失败路径）：主 + 第二目的地双侧清理。
    pub(super) fn discard_parts(&self) {
        let _ = fs::remove_file(&self.part);
        if let Some(part2) = &self.part2 {
            let _ = fs::remove_file(part2);
        }
    }
}

/// 单遍双写暂存：主/第二目的地的 `.part` 文件对，同一读流同遍写双侧；
/// 任一侧写/校验失败即双侧清盘（不留半套拷贝）。
struct PartSink {
    out: File,
    part: PathBuf,
    out2: Option<File>,
    part2: Option<PathBuf>,
}

impl PartSink {
    /// 在两个暂存目录各建 `{seq}.part`（第二目录 None 则只建主路）。
    fn create(part_dir: &Path, second_dir: Option<&Path>, seq: u64) -> Result<Self, String> {
        let part = part_dir.join(format!("{seq}.part"));
        let out = File::create(&part).map_err(|e| format!("创建临时文件失败: {e}"))?;
        let (out2, part2) = match second_dir {
            Some(dir) => {
                let part2 = dir.join(format!("{seq}.part"));
                let out2 =
                    File::create(&part2).map_err(|e| format!("创建第二目的地临时文件失败: {e}"))?;
                (Some(out2), Some(part2))
            }
            None => (None, None),
        };
        Ok(Self {
            out,
            part,
            out2,
            part2,
        })
    }

    /// 写双侧 + xxh64 单遍更新；失败清双侧半成品。
    fn write(&mut self, buf: &[u8], xxh: &mut Xxh64) -> Result<(), String> {
        if let Err(e) = self.out.write_all(buf) {
            self.discard();
            return Err(format!("写盘失败: {e}"));
        }
        if let Some(out2) = &mut self.out2 {
            if let Err(e) = out2.write_all(buf) {
                self.discard();
                return Err(format!("第二目的地写盘失败: {e}"));
            }
        }
        xxh.update(buf);
        Ok(())
    }

    /// flush + 关闭句柄（Windows rename 前必须）+ 双侧长度校验。
    fn finish(mut self, expected: u64) -> Result<(PathBuf, Option<PathBuf>), String> {
        if let Err(e) = self.out.flush() {
            self.discard();
            return Err(format!("刷盘失败: {e}"));
        }
        if let Some(out2) = &mut self.out2 {
            if let Err(e) = out2.flush() {
                self.discard();
                return Err(format!("第二目的地刷盘失败: {e}"));
            }
        }
        let verify = |part: &Path, label: &str| -> Result<(), String> {
            let len = match fs::metadata(part) {
                Ok(meta) => meta.len(),
                Err(e) => {
                    return Err(format!("{label}临时文件丢失: {e}"));
                }
            };
            if len != expected {
                return Err(format!(
                    "{label}长度校验失败: 期望 {expected} 字节，实得 {len}"
                ));
            }
            Ok(())
        };
        if let Err(e) = verify(&self.part, "") {
            self.discard();
            return Err(e);
        }
        if let Some(part2) = &self.part2 {
            if let Err(e) = verify(part2, "第二目的地") {
                self.discard();
                return Err(e);
            }
        }
        // 双侧写句柄随 self 在返回时 drop（Windows：rename 前必须关闭）
        Ok((self.part, self.part2))
    }

    /// 清双侧半成品（best-effort）。
    fn discard(&self) {
        let _ = fs::remove_file(&self.part);
        if let Some(part2) = &self.part2 {
            let _ = fs::remove_file(part2);
        }
    }
}

/// 单文件单遍复制：流式读 → 哈希/写盘/head 截存 → 长度校验。
/// dst = target_root + 渲染结果；second=Some(根) 时同遍双写第二目的地
/// （**同布局公式、同文件名模板**，仅根不同——2026-09-28 定案，第二份
/// 落 `{secondRoot}/{创建YYYY}/{创建MM}/{dir_name}/[{子组}/]`，子组段随
/// plan 传入的 dir_template 已含，0022）。
#[allow(clippy::too_many_arguments)]
pub(super) fn copy_one(
    source: &dyn DeviceSource,
    part_dir: &Path,
    seq: u64,
    move_mode: bool,
    duplicate_policy: DuplicatePolicy,
    entry: &FileEntry,
    target_root: &Path,
    dir_template: &str,
    name_template: &str,
    second: Option<&Path>,
) -> FileOutcome {
    let fail = |error: String| FileOutcome::Failed {
        entry: entry.clone(),
        error,
    };
    let disconnect = |error: String| FileOutcome::Disconnected {
        entry: entry.clone(),
        error,
    };
    let io_err = |e: std::io::Error| {
        if e.kind() == std::io::ErrorKind::ConnectionAborted {
            disconnect(format!("传输中断: {e}"))
        } else {
            fail(format!("读取失败: {e}"))
        }
    };

    let mut reader = match source.stream(&entry.id) {
        Ok(reader) => reader,
        Err(DeviceError::Disconnected) => return disconnect("设备连接中断".into()),
        Err(err) => return fail(err.to_string()),
    };

    // 首段：截存 head（≤1MB）——EXIF + 魔数识别；同时进哈希与 .part
    let mut head: Vec<u8> = Vec::with_capacity(entry.size.min(HEAD_MAX as u64) as usize);
    let mut first = vec![0u8; HEAD_MAX];
    while head.len() < HEAD_MAX {
        match reader.read(&mut first[..HEAD_MAX - head.len()]) {
            Ok(0) => break,
            Ok(n) => head.extend_from_slice(&first[..n]),
            Err(e) => return io_err(e),
        }
    }

    let kind = classify(&entry.rel_path, &head);
    if kind == AssetKind::Other {
        return fail("类型识别失败（扩展名与内容不符，疑似伪装文件）".into());
    }
    let meta = exif_lite::parse(&head);

    // 渲染目标相对路径（{相机}/{镜头} 缺失降级默认段；主/第二目的地同
    // 布局公式同文件名模板——dir_template 为相册固定公式（字面量，可含
    // 0022 子组段），逐照片令牌仅剩文件名段）
    let (stem, ext) = split_stem_ext(&entry.rel_path);
    let ctx = RenderCtx {
        captured_at: resolve_captured(meta.captured_at, entry.mtime),
        camera: meta.camera.clone(),
        lens: None,
        original_stem: stem,
        ext,
    };
    let rel = match render_dst(&ctx, dir_template, name_template) {
        Ok(rel) => rel,
        Err(error) => return fail(error),
    };
    let dst = target_root.join(&rel);
    // 免下载预跳（2026-09-29 实测重建库全量重导：重复照片被完整拉回
    // ~600ms/张再丢弃）：Skip/Ask 策略下目标已存在与收集端 ③ 同判据
    // 提前到这里——只花流打开+头读（EXIF 渲染目标路径必需），不写
    // .part、不算哈希、不删临时文件；MTP 单 worker 串行下整体吞吐
    // 量级提升。Rename 策略仍需下载落位，不预跳。
    if matches!(duplicate_policy, DuplicatePolicy::Skip | DuplicatePolicy::Ask) && dst.exists() {
        return FileOutcome::Skipped {
            entry: entry.clone(),
        };
    }
    // 第二目的地：同公式、仅根不同
    let (dst2, second_part_dir) = match second {
        Some(root) => (Some(root.join(&rel)), Some(root.join(PART_DIR))),
        None => (None, None),
    };

    // 同卷 rename 快道（M8-①）：move 模式 + 单目的地，直接尝试 rename →
    // 源直改 .part（大 RAW 毫秒级），跳过流式复制与内联哈希（xxh=0 哨兵，
    // hash 通道后台补算——见 index::process_hash_task）。目标名已占用时
    // 不走快道：Skip 策略要保留源数据，流式 .part 才可安全丢弃；rename
    // 失败（EXDEV/权限/被锁）静默回退流式路径，绝不失败导入。
    if move_mode && second.is_none() && !dst.exists() {
        if let Some(src) = source.local_path(&entry.id) {
            let part_path = part_dir.join(format!("{seq}.part"));
            if fs::rename(&src, &part_path).is_ok() {
                return FileOutcome::Copied(Box::new(CopiedFile {
                    entry: entry.clone(),
                    kind,
                    meta,
                    xxh: 0,
                    fast_moved: true,
                    part: part_path,
                    dst,
                    part2: None,
                    dst2: None,
                }));
            }
            // 回退：源未动，走流式
        }
    }

    // 写 .part（集中暂存目录，避免同名目标并发冲突；双目的地各建一份）
    let mut sink = match PartSink::create(part_dir, second_part_dir.as_deref(), seq) {
        Ok(sink) => sink,
        Err(error) => return fail(error),
    };
    let mut xxh = Xxh64::new(0);
    if let Err(error) = sink.write(&head, &mut xxh) {
        return fail(error);
    }

    // 剩余流：8MB 块单遍读（双写）
    let mut chunk = vec![0u8; CHUNK];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if let Err(error) = sink.write(&chunk[..n], &mut xxh) {
                    return fail(error);
                }
            }
            Err(e) => return io_err(e),
        }
    }

    // 刷盘 + 长度校验（spec §7：任何时刻不留半文件）
    let (part, part2) = match sink.finish(entry.size) {
        Ok(parts) => parts,
        Err(error) => return fail(error),
    };

    FileOutcome::Copied(Box::new(CopiedFile {
        entry: entry.clone(),
        kind,
        meta,
        xxh: xxh.digest(),
        fast_moved: false,
        part,
        dst,
        part2,
        dst2,
    }))
}

/// 渲染目录+文件名 → 完整目标路径；`{相机}`/`{镜头}` 缺失时用默认段重试。
fn render_dst(ctx: &RenderCtx, dir_template: &str, name_template: &str) -> Result<PathBuf, String> {
    let render = |ctx: &RenderCtx| -> Result<String, TemplateError> {
        Ok(format!(
            "{}/{}",
            render_dir(dir_template, ctx)?,
            render_name(name_template, ctx)?
        ))
    };
    match render(ctx) {
        Ok(path) => Ok(PathBuf::from(path)),
        Err(TemplateError::MissingContext(_)) => {
            // 无 EXIF 的文件（截图/转码/损坏头）：默认段降级重试
            let fallback = RenderCtx {
                captured_at: ctx.captured_at,
                camera: Some(FALLBACK_CAMERA.to_string()),
                lens: Some(FALLBACK_LENS.to_string()),
                original_stem: ctx.original_stem.clone(),
                ext: ctx.ext.clone(),
            };
            render(&fallback)
                .map(PathBuf::from)
                .map_err(|e| e.to_string())
        }
        Err(TemplateError::UnknownToken(token)) => {
            Err(format!("命名模板含未知令牌「{token}」，请检查导入设置"))
        }
    }
}

fn split_stem_ext(rel_path: &str) -> (String, String) {
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), ext.to_string()),
        _ => (name.to_string(), String::new()),
    }
}
