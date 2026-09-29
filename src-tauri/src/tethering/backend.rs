//! CameraBackend trait + 能力位 + 注册表（阶段 E-1）。
//!
//! 注册表模式与 V3 既定 trait 体系一致（OnceLock 全局单例 + 按 id 取用）；
//! v1 唯一实现 [`crate::tethering::wpd_backend::WpdMtpBackend`]（L0 文件
//! 通道 + L1 MTP 透传合并），为 L3 Sony SDK 插件预留槽位（运行时探测
//! DLL 存在性，不存在整层隐藏——本阶段不实现）。
//!
//! COM 边界用 trait 做 seam 隔离：IPC 核心编排函数接收 `&dyn
//! CameraBackend`，集成测试注入假后端验证注册表/超时/错误语义（不接真机）。

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Duration;

use super::wpd_backend::WpdMtpBackend;

// ---------------------------------------------------------------------------
// 能力位（位标志；DTO 布尔字段与之逐位对应）
// ---------------------------------------------------------------------------

/// 相机能力位（路线图 §7「每能力报告 supported/unsupported」）。
/// 设备级能力由探测（GetDeviceInfo / WPD 能力查询交叉）产出；
/// 后端级能力（trait [`CameraBackend::capabilities`]）是类型上限。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities(u32);

impl Capabilities {
    /// 文件传输（L0：既有 WPD 导入栈，恒真于 WpdMtpBackend）。
    pub const FILE_TRANSFER: Self = Self(1 << 0);
    /// 标准 PTP InitiateCapture（0x100E）拍摄。
    pub const STANDARD_CAPTURE: Self = Self(1 << 1);
    /// Nikon 厂商扩展拍摄码 0x90C0。
    pub const VENDOR_CAPTURE_NIKON: Self = Self(1 << 2);
    /// OBJECT_ADDED 事件可用（拍后自动收片依据）。
    pub const OBJECT_ADDED_EVENTS: Self = Self(1 << 3);
    /// LiveView 预览流（v1 恒不支持）。
    pub const LIVE_VIEW: Self = Self(1 << 4);

    /// 空集。
    #[allow(dead_code)] // 测试桩构造用（lib 内暂无触发点）
    pub const NONE: Self = Self(0);

    /// 并集（插入位）。
    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// 是否包含 other 的全部位。
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// 单位读取（DTO 装配用）。
    pub fn file_transfer(self) -> bool {
        self.contains(Self::FILE_TRANSFER)
    }
    pub fn standard_capture(self) -> bool {
        self.contains(Self::STANDARD_CAPTURE)
    }
    pub fn vendor_capture_nikon(self) -> bool {
        self.contains(Self::VENDOR_CAPTURE_NIKON)
    }
    pub fn object_added_events(self) -> bool {
        self.contains(Self::OBJECT_ADDED_EVENTS)
    }
    pub fn live_view(self) -> bool {
        self.contains(Self::LIVE_VIEW)
    }
}

// ---------------------------------------------------------------------------
// 领域类型
// ---------------------------------------------------------------------------

/// 一台可联拍相机的会话外信息（枚举与探测共用形态）。
#[derive(Debug, Clone, PartialEq)]
pub struct CameraInfo {
    /// PnP 设备路径（与 WPD 导入栈同一标识；注册表键用规范化小写形态）。
    pub pnp_id: String,
    /// 展示名（WPD 友好名）。
    pub name: String,
    /// 设备级能力（探测产出；未探测时为后端默认位 = FILE_TRANSFER）。
    pub capabilities: Capabilities,
}

/// 拍摄产出的新对象（OBJECT_ADDED 事件的富化结果）。
#[derive(Debug, Clone, PartialEq)]
pub struct CapturedObject {
    /// WPD 会话对象 ID（瞬态；不落库，仅诊断）。
    pub object_id: String,
    /// 对象文件名（优先 ORIGINAL_FILE_NAME，回退 WPD_OBJECT_NAME）。
    pub object_name: String,
    /// 对象大小（字节）。
    pub object_size: u64,
}

/// 联拍错误语义（与 DeviceError 对齐的可提示分类；UI 按类渲染）。
#[derive(Debug, thiserror::Error)]
pub enum TetherError {
    /// 等待拍摄结果超时（契约文案：相机未响应拍摄命令）。
    #[error("相机未响应拍摄命令")]
    Timeout,
    /// 相机/后端不支持拍摄（无 0x100E 亦无厂商拍摄码）。
    #[error("该相机不支持联机拍摄（未声明 InitiateCapture 或厂商拍摄码）")]
    CaptureNotSupported,
    /// 相机未切 PC 连接模式 / 会话被其他软件占用。
    #[error("相机拒绝访问（未切换连接模式或会话被占用）")]
    AccessDenied,
    /// 拔线/会话丢失。
    #[error("相机已断开")]
    Disconnected,
    #[error("{0}")]
    Other(String),
}

impl From<crate::devices::DeviceError> for TetherError {
    /// DeviceError → 联拍错误语义（复用既有映射，不新增异常路径）。
    fn from(err: crate::devices::DeviceError) -> Self {
        match err {
            crate::devices::DeviceError::AccessDenied => Self::AccessDenied,
            crate::devices::DeviceError::Disconnected => Self::Disconnected,
            crate::devices::DeviceError::NotSupported(what) => Self::from_not_supported(&what),
            other => Self::Other(other.to_string()),
        }
    }
}

impl TetherError {
    /// NotSupported 细分：拍摄相关操作缺失归 CaptureNotSupported，其余 Other。
    fn from_not_supported(what: &str) -> Self {
        if what.contains("capture") || what.contains("拍摄") {
            Self::CaptureNotSupported
        } else {
            Self::Other(format!("operation not supported: {what}"))
        }
    }
}

/// 参数控件形态（前端按 kind 选控件：下拉/开关/滑条/按钮）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingKind {
    /// 多选一下拉（RADIO/MENU）。
    Choice,
    /// 开关（TOGGLE，On/Off）。
    Toggle,
    /// 数值滑条（RANGE，min/max/step）。
    Range,
    /// 立即执行的动作按钮（如自动对焦触发）。
    Action,
    /// 只读文本（诊断信息，不参与设置）。
    Text,
}

fn default_setting_kind() -> SettingKind {
    SettingKind::Choice
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraSetting {
    pub id: String,
    /// gphoto 面板标签（英文兜底；前端对常用 id 覆盖本地化文案）。
    #[serde(default)]
    pub label: String,
    #[serde(default = "default_setting_kind")]
    pub kind: SettingKind,
    pub current: String,
    pub writable: bool,
    #[serde(default)]
    pub options: Vec<CameraSettingOption>,
    /// Range 专用；Choice/Toggle/Action 无此三项。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CameraSettingOption {
    pub value: String,
    pub label: String,
}

// ---------------------------------------------------------------------------
// CameraBackend trait
// ---------------------------------------------------------------------------

/// 相机联拍后端统一接口（v1 实现见 wpd_backend；L3 Sony SDK 插件预留）。
///
/// 生命周期语义：`connect`（打开会话 + 全量探测 + 刷新能力缓存）→
/// `capture_still`（可反复；无缓存时内部先探测选码）→ `disconnect`
/// （清理会话态与缓存，隔离 worker 旧连接）。
///
/// `id/name/capabilities/disconnect` 为 v1 契约面（L3 Sony 插件注册表
/// 查找 / UI 后端展示 / 会话清理），lib 内调用点待前端接线与 L3 落地。
#[allow(dead_code)]
pub trait CameraBackend: Send + Sync {
    /// 后端稳定标识（注册表查找键，如 "wpd-mtp"）。
    fn id(&self) -> &'static str;

    /// 展示名（UI 后端选择/诊断用）。
    fn name(&self) -> &'static str;

    /// 后端类型的能力上限（设备级以 connect 探测为准；UI 永远按后端
    /// 报告的能力渲染，不做品牌名承诺——评估文档 §3.4）。
    fn capabilities(&self) -> Capabilities;

    /// 枚举本后端可见的相机（不逐台 Open 探测——探测昂贵，connect 单独做；
    /// 已探测设备用缓存能力位填充）。
    fn enumerate(&self) -> Result<Vec<CameraInfo>, TetherError>;

    /// 打开指定相机并全量探测能力（MTP GetDeviceInfo + WPD 能力交叉），
    /// 返回探测后的设备信息并刷新缓存。
    fn connect(&self, pnp_id: &str) -> Result<CameraInfo, TetherError>;

    /// 断开：清理会话态/能力缓存并隔离旧连接（不产生新错误路径）。
    fn disconnect(&self, pnp_id: &str);

    fn settings(&self, _pnp_id: &str) -> Result<Vec<CameraSetting>, TetherError> {
        Ok(Vec::new())
    }
    fn set_setting(&self, _pnp_id: &str, _id: &str, _value: &str) -> Result<(), TetherError> {
        Err(TetherError::Other("相机不支持参数控制".into()))
    }
    fn live_view_frame(&self, _pnp_id: &str) -> Result<Vec<u8>, TetherError> {
        Err(TetherError::Other("相机不支持实时取景".into()))
    }

    /// 点击取景画面对焦：坐标为归一化 live view 坐标（0..1，原点左上）。
    /// 默认不支持（WPD/Sony 桥未实现）。
    fn focus_at(&self, _pnp_id: &str, _x: f64, _y: f64) -> Result<(), TetherError> {
        Err(TetherError::Other("该相机不支持点击对焦".into()))
    }

    fn trigger_capture(&self, pnp_id: &str) -> Result<Vec<CapturedObject>, TetherError> {
        self.capture_still(pnp_id, Duration::from_secs(20))
            .map(|object| vec![object])
    }
    fn poll_objects(&self, _pnp_id: &str) -> Result<Vec<CapturedObject>, TetherError> {
        Ok(Vec::new())
    }
    fn open_captured(
        &self,
        pnp_id: &str,
        object: &CapturedObject,
    ) -> Result<Box<dyn std::io::Read + Send>, TetherError> {
        use crate::devices::DeviceSource;
        crate::devices::wpd::WpdSource::new(pnp_id, pnp_id)
            .stream(&object.object_id)
            .map_err(Into::into)
    }

    /// 触发一次静物拍摄：按能力选 0x90C0（优先，Nikon 对标准码报
    /// Parameter Not Supported）或 0x100E，订阅 OBJECT_ADDED 等待新对象，
    /// 返回对象名/大小。超时返回 [`TetherError::Timeout`]（文案
    /// 「相机未响应拍摄命令」）。
    fn capture_still(&self, pnp_id: &str, timeout: Duration)
        -> Result<CapturedObject, TetherError>;
}

// ---------------------------------------------------------------------------
// 能力缓存合并（纯函数，单测见 tests/tethering_test.rs）
// ---------------------------------------------------------------------------

/// 枚举结果与探测缓存合并：同一（规范化）pnp 且缓存更具体时用缓存位
/// 覆盖枚举默认位。缓存 key 为规范化设备 id（小写 PnP 路径）。
pub fn apply_cached_capabilities(
    devices: Vec<CameraInfo>,
    cache: &BTreeMap<String, Capabilities>,
) -> Vec<CameraInfo> {
    devices
        .into_iter()
        .map(|mut info| {
            if let Some(cached) = cache.get(&crate::devices::normalize_device_id(&info.pnp_id)) {
                info.capabilities = *cached;
            }
            info
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 注册表（OnceLock 全局单例；L3 插件未来经此追加）
// ---------------------------------------------------------------------------

/// 已注册的联拍后端集合（v1 仅 WpdMtpBackend）。
pub struct CameraBackendRegistry {
    backends: Vec<std::sync::Arc<dyn CameraBackend>>,
}

impl CameraBackendRegistry {
    /// v1 构造：WPD MTP 后端（L0 文件通道 + L1 透传合一）+ Sony SDK 桥 +
    /// libgphoto2 进程内后端（DLL 运行时探测，缺失时枚举报「不可用」）。
    fn v1() -> Self {
        Self {
            backends: vec![
                std::sync::Arc::new(WpdMtpBackend::new()),
                std::sync::Arc::new(super::sony_backend::SonyBackend::default()),
                std::sync::Arc::new(super::gphoto_backend::GphotoBackend::default()),
            ],
        }
    }

    /// 按后端 id 查找（L3 插件注册表查找面；测试断言）。
    #[allow(dead_code)]
    pub fn get(&self, id: &str) -> Option<&std::sync::Arc<dyn CameraBackend>> {
        self.backends.iter().find(|b| b.id() == id)
    }

    /// 默认后端（v1 即唯一后端；IPC 命令未指定后端 id 时使用）。
    pub fn primary(&self) -> &std::sync::Arc<dyn CameraBackend> {
        &self.backends[0]
    }

    /// 全部后端（枚举聚合用）。
    pub fn all(&self) -> &[std::sync::Arc<dyn CameraBackend>] {
        &self.backends
    }
}

/// 全局注册表（进程级一次构造；与 ai::catalog 等 V3 静态注册表同款）。
pub fn backend_registry() -> &'static CameraBackendRegistry {
    static REGISTRY: OnceLock<CameraBackendRegistry> = OnceLock::new();
    REGISTRY.get_or_init(CameraBackendRegistry::v1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_bits_are_independent() {
        let caps = Capabilities::FILE_TRANSFER
            .union(Capabilities::STANDARD_CAPTURE)
            .union(Capabilities::VENDOR_CAPTURE_NIKON)
            .union(Capabilities::OBJECT_ADDED_EVENTS);
        assert!(caps.file_transfer());
        assert!(caps.standard_capture());
        assert!(caps.vendor_capture_nikon());
        assert!(caps.object_added_events());
        assert!(!caps.live_view());
        assert!(!Capabilities::NONE.file_transfer());
        assert!(Capabilities::NONE
            .union(Capabilities::LIVE_VIEW)
            .live_view());
    }

    #[test]
    fn registry_has_wpd_backend_as_primary() {
        let registry = backend_registry();
        assert_eq!(registry.primary().id(), "wpd-mtp");
        assert!(registry.get("wpd-mtp").is_some());
        assert!(registry.get("nope").is_none());
        assert_eq!(registry.all().len(), 3);
        assert!(registry.get("gphoto").is_some());
    }
}
