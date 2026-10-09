//! 基础编辑与导出（阶段 D，roadmap §8 / 2026-09-27 跟进计划 §2.D）。
//!
//! 非破坏编辑：裁剪/旋转/文字/画笔标注/输出尺寸保存为**编辑配方**
//! （`edit_recipe` 表，version 化 JSON）；导出才解码源文件实时合成新 JPEG。
//! 源文件与其拍摄元数据永不被修改。
//!
//! 模块拆分：
//! - [`recipe`]：配方契约（version 1）——结构反序列化 + 服务端校验
//!   （数值夹取 0..1、颜色/版本非法报错），存库前归一化。
//! - [`text`]：系统字体加载（运行时读 `C:\Windows\Fonts`，**不捆绑字体文件**
//!   ——msyh.ttc → simhei.ttf → 静态回退）+ 文字/笔迹栅格化。
//! - [`render`]：源解码（JPEG=image/turbojpeg 路径，RAW=rawler 显影→内嵌
//!   预览兜底）→ 方向转正（与缩略图管线共用 [`crate::thumbs::apply_orientation`]）
//!   → 旋转 → 裁剪 → 文字/笔迹 → 只缩不放的高质量缩放。
//! - [`meta`]：导出 JPEG 元数据——源 EXIF 尽量保留（拍摄时间/相机/镜头/
//!   参数），removeGps 剥 GPS IFD；版权/作者/关键词写 EXIF Artist/Copyright
//!   + XMP dc:rights/dc:creator/dc:subject（img-parts 段级改写，图像数据零重编码）。
//! - [`export`]：导出执行器——folder 模式（.part + 原子改名，目标存在报错）
//!   与 album 模式（复用导入引擎的相册落位/登记路径）；任务落 `export_job`
//!   表（持久化任务铁律），进度/收尾走 EventBus 既有任务事件通道。
//! - [`ipc`]：Tauri 命令壳（放本模块而非 ipc/ 的原因见该文件头注释）。
//!
//! **本体写回原子替换铁律**（§八-4，2026-10-09 审计结论）：库内照片本体
//! 一切写回必须「临时文件 + rename」——rename 只换名字指向，旧 inode 留给
//! 硬链接导出物（快照成真），且崩溃安全。本模块为**非破坏编辑**：本体与
//! 拍摄元数据永不被改写；img-parts 段级改写只作用于**导出产物的内存缓冲**
//! （[`meta::apply_metadata`] 入参即编码后的 JPEG 字节），落盘统一走
//! [`export`] 的 `.part` + rename（同款手法的还有：导入引擎 pipeline 的
//! `.part` 暂存、XMP 边车 `xmp::sync_sidecar` 的 `.xmp.tmp` + rename、
//! move 模式边车跟随的临时文件复制）。若未来新增任何对库内本体的直接
//! 写回（如 JPEG 段原地 patch），必须改为「临时文件 + rename」——禁止
//! 原地覆写。
//!
//! 线程模型：全部核函数为同步阻塞（解码/渲染/编码/IO 重），IPC 层经
//! `run_blocking` 或 TaskSupervisor 后台线程调用，绝不上 UI/主线程。
//!
//! 坐标契约（version 1）：旋转先于裁剪；crop 相对**旋转后**图像（x,w 归一
//! 化到宽、y,h 归一化到高）；文字/笔迹相对**裁剪后画布**且 x,y,w 全部
//! 归一化到**画布宽**（sizeRel=字高/画布宽，widthRel=笔宽/画布宽——宽一
//! 归一避免笔迹随画布纵横比变形）；文字锚点=文本框左上角，左对齐。

pub mod export;
mod gpu;
pub mod native_canvas;
#[cfg(windows)]
mod native_presenter;
pub mod ipc;
pub mod meta;
pub mod metadata;
pub mod photocraft;
pub mod preview;
mod preview_renderer;
pub mod project;
pub mod recipe;
pub mod render;
mod source;
pub mod text;
