//! 联机拍摄底座（阶段 E-1，「WPD 零驱动联拍」，依据
//! docs/plans/2026-09-28-tethering-evaluation.md 路线 L0+L1）。
//!
//! 分层：
//! - [`backend`]：`CameraBackend` trait + `Capabilities` 能力位 + 注册表
//!   （V3 trait 注册表模式；v1 唯一实现为 [`wpd_backend::WpdMtpBackend`]，
//!   为 L3 Sony SDK 插件预留槽位）。
//! - [`mtp`]：跨平台纯函数——PTP/MTP 操作码常量、GetDeviceInfo 数据集
//!   解析、能力位推导、拍摄操作码选择（单测覆盖，无 COM）。
//! - [`wpd_backend`]：WPD MTP 扩展透传（`IPortableDevice::SendCommand` +
//!   WPD_COMMAND_MTP_EXT_*，fmtid `4D545058-1A2E-4106-A357-771E0819FC56`，
//!   常量取自 windows crate 收录的 SDK 头文件数值）与
//!   `WPD_EVENT_OBJECT_ADDED` 事件管线（Advise/Unadvise）。
//!
//! 事件流向：capture 成功收片 → `AppEvent::TetheringObjectAdded` →
//! 既有事件总线 → 前端 `app://event`（纯增量）。联拍会话态不入库
//! （瞬态）；导入落库走既有导入管线，本层不重复。
//!
//! # 真机验证清单（离线不可验，接机身后逐项确认——本模块不实现）
//!
//! | # | 项 | 说明 |
//! |---|---|---|
//! | N2/N3 | Nikon D750 0x90C0 实测 | 透传 GetDeviceInfo 记录 OperationsSupported 全表；0x90C0 触发拍摄 + OBJECT_ADDED 收片闭环（参数 0x1 的取值以实测为准） |
//! | S2 | A7R V PC Remote 模式下 WPD 可见性 | 切 PC Remote 后 WPD 是否仍枚举/可列文件（决定 Sony 上 L0/L1 边界） |
//! | - | InitiateCapture 0x100E 机型面 | 哪些机身对标准码响应（历史实证：RICOH/Panasonic 有、Nikon 报 Parameter Not Supported） |
//! | - | WPD→PTP ObjectAdded 事件映射 | MTP 类驱动将 PTP ObjectAdded(0x4002) 映射为 WPD_EVENT_OBJECT_ADDED 的机制（评估文档推断项） |

pub mod backend;
pub mod mtp;
pub mod wpd_backend;

pub mod sony_backend;
pub mod gphoto_backend;
pub mod session;
