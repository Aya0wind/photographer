//! CameraBackend trait + 能力位 + 注册表（阶段 E-1）。
//!
//! 注册表模式与 V3 既定 trait 体系一致（OnceLock 全局单例 + 按 id 取用）；
//! v1 唯一实现 [`crate::tethering::wpd_backend::WpdMtpBackend`]（L0 文件
//! 通道 + L1 MTP 透传合并），为 L3 Sony SDK 插件预留槽位（运行时探测
//! DLL 存在性，不存在整层隐藏——本阶段不实现）。
//!
//! COM 边界用 trait 做 seam 隔离：IPC 核心编排函数接收 `&dyn
//! CameraBackend`，集成测试注入假后端验证注册表/超时/错误语义（不接真机）。

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
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
    /// 物理身份（2026-10-11 §E）：机身序列号（探测读取；取不到 None）。
    pub serial: Option<String>,
    /// USB 厂商 id（从设备路径解析；非 USB 端点/取不到 None）。
    pub vid: Option<String>,
    /// USB 产品 id（同上）。
    pub pid: Option<String>,
}

impl CameraInfo {
    /// 物理相机稳定键（§E）：serial 优先；缺 serial 回退
    /// vid+pid+model+设备实例路径——同一物理机跨端点（wpd 与 gphoto 同报
    /// 一台）得到同一键，不同物理机同名不合并。
    pub fn physical_key(&self) -> String {
        if let Some(serial) = self.serial.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            return format!("serial:{serial}");
        }
        format!(
            "usb:{vid}:{pid}:{model}:{instance}",
            vid = self.vid.as_deref().unwrap_or(""),
            pid = self.pid.as_deref().unwrap_or(""),
            model = self.name,
            instance = crate::devices::normalize_device_id(&self.pnp_id),
        )
    }
}

/// 从 Windows PnP 设备路径解析 USB VID/PID（`usb#vid_04b0&pid_0456#...`
/// 形态；大小写不敏感）。跨后端共用：端点路径里确有的身份才算数，
/// gphoto 端口路径（`usb:001,010`）解析不出 → None。
pub fn usb_vid_pid_from_pnp(pnp: &str) -> (Option<String>, Option<String>) {
    let lower = pnp.to_ascii_lowercase();
    let parse = |token: &str| -> Option<String> {
        let start = lower.find(token)? + token.len();
        let hex: String = lower[start..]
            .chars()
            .take_while(|c| c.is_ascii_hexdigit())
            .take(4)
            .collect();
        (hex.len() == 4).then(|| hex.to_ascii_uppercase())
    };
    (parse("vid_"), parse("pid_"))
}

/// 取景帧：JPEG 字节 + 相机回报的归一化 AF 区域（同一帧数据，单次往返；
/// None = 本帧无 AF 数据或不适用）。
#[derive(Debug, Clone)]
pub struct LiveViewFrame {
    pub jpeg: Vec<u8>,
    /// 归一化 (x, y, w, h)，原点左上，相对取景画面。
    pub af_region: Option<(f64, f64, f64, f64)>,
}

impl LiveViewFrame {
    /// 无 AF 数据的普通帧（gphoto/Sony 桥等不解析帧头的路径）。
    pub fn plain(jpeg: Vec<u8>) -> Self {
        Self {
            jpeg,
            af_region: None,
        }
    }
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
    fn probe(&self, pnp_id: &str) -> Result<CameraInfo, TetherError> {
        let info = self.connect(pnp_id)?;
        self.disconnect(pnp_id);
        Ok(info)
    }

    fn disconnect(&self, pnp_id: &str);

    fn settings(&self, _pnp_id: &str) -> Result<Vec<CameraSetting>, TetherError> {
        Ok(Vec::new())
    }
    fn set_setting(&self, _pnp_id: &str, _id: &str, _value: &str) -> Result<(), TetherError> {
        Err(TetherError::Other("相机不支持参数控制".into()))
    }
    fn live_view_frame(&self, _pnp_id: &str) -> Result<LiveViewFrame, TetherError> {
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
    /// 按对象 id 主动收取（崩溃/重连恢复用）：校验对象仍在设备上并重新
    /// 入收片队列，返回可恢复的 id。默认空 = 该后端不支持按需收取，调用
    /// 方退回事件驱动等待。
    fn request_objects(&self, _pnp_id: &str, _ids: &[String]) -> Result<Vec<String>, TetherError> {
        Ok(Vec::new())
    }
    /// 收片队列中待接收对象数（快照真值：会话「N 待收」显示用）。无队列
    /// 概念的后端返回 0。
    fn pending_receipts(&self, _pnp_id: &str) -> usize {
        0
    }
    /// 参数变化通知的序号；不支持通知的后端使用低频快照兜底。
    fn settings_revision(&self, _pnp_id: &str) -> u64 {
        0
    }
    /// Library ingestion has committed successfully. Backends may now release
    /// their local download copy; never call this on an ingestion failure.
    fn acknowledge_captured(
        &self,
        _pnp_id: &str,
        _object: &CapturedObject,
    ) -> Result<(), TetherError> {
        Ok(())
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
// 自动后端选择（§E-2）与跨后端物理去重（§E-3）
// ---------------------------------------------------------------------------

/// 判断探测能力是否达到联拍门槛（实际拍摄能力——按 GetDeviceInfo 的
/// operations supported 探测产出，不按后端名硬编码）。
fn supports_tethering(caps: Capabilities) -> bool {
    caps.standard_capture() || caps.vendor_capture_nikon()
}

/// 会话建立时的后端选择（§E-2）：按入参顺序（注册表序 = 原生 → 已安装
/// SDK → libgphoto2）逐一 connect 探测，选第一个**探测到实际拍摄能力**
/// 的端点；全部无拍摄能力时保留第一个成功连接的端点（准确显示限制）。
///
/// 铁律：
/// - 探测复用既有 connect 语义（GetDeviceInfo/能力交叉），不触发快门、
///   不改设置；
/// - 存在未 Acknowledge 收据的相机禁止切换后端路径——账本里未确认收据
///   记录的产生后端（`pending.backend`）之外的后端一律跳过；该后端自身
///   连不上则返回 Err 说明；
/// - 未选中的已连接端点必须 disconnect（不能并留两份设备句柄）。
pub fn establish_backend(
    backends: &[std::sync::Arc<dyn CameraBackend>],
    camera_id: &str,
    receipts_dir: Option<&Path>,
) -> Result<(std::sync::Arc<dyn CameraBackend>, CameraInfo), String> {
    let mut fallback: Option<(std::sync::Arc<dyn CameraBackend>, CameraInfo)> = None;
    let mut last_error = String::from("没有可用的相机后端");
    let mut receipts_pin: Option<Vec<String>> = None;
    for backend in backends {
        let info = match backend.connect(camera_id) {
            Ok(info) => info,
            Err(error) => {
                last_error = error.to_string();
                continue;
            }
        };
        // §E：未确认收据把相机钉在产生收据的后端上。
        if let Some(dir) = receipts_dir {
            let unacked =
                super::receipts_store::ReceiptsStore::open(dir, &info.physical_key()).unacked();
            let pinned: HashSet<String> = unacked
                .iter()
                .map(|receipt| receipt.backend.clone())
                .collect();
            if !pinned.is_empty() {
                receipts_pin = Some(pinned.iter().cloned().collect());
                if !pinned.contains(backend.id()) {
                    backend.disconnect(camera_id);
                    continue;
                }
            }
        }
        if supports_tethering(info.capabilities) {
            if let Some((previous, _)) = fallback.take() {
                previous.disconnect(camera_id);
            }
            return Ok((backend.clone(), info));
        }
        match &fallback {
            None => fallback = Some((backend.clone(), info)),
            Some(_) => backend.disconnect(camera_id), // 无拍摄能力的后来者不占设备
        }
    }
    if let Some((backend, info)) = fallback {
        return Ok((backend, info));
    }
    if let Some(pinned) = receipts_pin {
        return Err(format!(
            "相机存在未确认的接收收据（产生于 {pinned:?} 后端），该后端当前不可用；\
             为避免照片丢失已禁止切换后端路径，请重试或重连原后端"
        ));
    }
    Err(format!("无法连接相机：{last_error}"))
}

/// 枚举结果跨后端按物理身份去重（§E-3）：同一物理相机被多个端点（wpd
/// 与 gphoto 同报一台）重复报告时只保留先到的优先端点入口（入参顺序即
/// 注册表优先序：原生 → SDK → gphoto）；不同物理机同名不合并。
pub fn dedupe_by_physical_identity(cameras: Vec<CameraInfo>) -> Vec<CameraInfo> {
    let mut seen = HashSet::new();
    cameras
        .into_iter()
        .filter(|info| seen.insert(info.physical_key()))
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

    /// Legacy default backend for callers without a camera id.
    #[allow(dead_code)]
    pub fn primary(&self) -> &std::sync::Arc<dyn CameraBackend> {
        &self.backends[0]
    }

    /// Select the backend from the stable camera id prefix.
    pub fn for_camera_id(&self, camera_id: &str) -> Option<&std::sync::Arc<dyn CameraBackend>> {
        let id = if camera_id.starts_with("gphoto:") {
            "gphoto"
        } else if camera_id.starts_with("sony-sdk:") {
            "sony-sdk"
        } else {
            "wpd-mtp"
        };
        self.get(id)
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
        assert_eq!(
            registry.for_camera_id("gphoto:usb=001,002").unwrap().id(),
            "gphoto"
        );
        assert_eq!(
            registry.for_camera_id("sony-sdk:camera").unwrap().id(),
            "sony-sdk"
        );
        assert_eq!(
            registry.for_camera_id("wpd-camera").unwrap().id(),
            "wpd-mtp"
        );
    }

    // -----------------------------------------------------------------------
    // 物理身份（§E-1/§E-3）
    // -----------------------------------------------------------------------

    fn info(pnp: &str, name: &str, serial: Option<&str>) -> CameraInfo {
        let (vid, pid) = usb_vid_pid_from_pnp(pnp);
        CameraInfo {
            pnp_id: pnp.into(),
            name: name.into(),
            capabilities: Capabilities::FILE_TRANSFER,
            serial: serial.map(str::to_string),
            vid,
            pid,
        }
    }

    #[test]
    fn usb_vid_pid_parses_from_windows_pnp_paths_only() {
        let (vid, pid) = usb_vid_pid_from_pnp(r"\\?\usb#vid_04b0&pid_0456#000123#{g}");
        assert_eq!(vid.as_deref(), Some("04B0"));
        assert_eq!(pid.as_deref(), Some("0456"));
        let (vid, pid) = usb_vid_pid_from_pnp("USB\\VID_04B0&PID_0456\\X");
        assert_eq!(vid.as_deref(), Some("04B0"));
        assert_eq!(pid.as_deref(), Some("0456"));
        assert_eq!(usb_vid_pid_from_pnp("gphoto:usb:001,010"), (None, None));
        assert_eq!(usb_vid_pid_from_pnp("sony-sdk:camera"), (None, None));
    }

    #[test]
    fn physical_key_prefers_serial_and_falls_back_to_usb_triple() {
        let a = info(r"\\?\usb#vid_04b0&pid_0456#s1#{g}", "Z5II", Some("SN-1"));
        let b = info("gphoto:usb:001,010", "Z5II via gphoto", Some("sn-1 "));
        assert_eq!(a.physical_key(), "serial:SN-1");
        // 同序列号跨端点同键（大小写/空白归一）；不同序列号不同键。
        assert_eq!(b.physical_key(), "serial:sn-1");
        assert_ne!(a.physical_key(), b.physical_key());
        // 无 serial：vid+pid+model+实例路径——同名不同实例不合并。
        let c = info(r"\\?\usb#vid_04b0&pid_0456#s1#{g}", "D750", None);
        let d = info(r"\\?\usb#vid_04b0&pid_0456#s2#{g}", "D750", None);
        assert_ne!(c.physical_key(), d.physical_key());
        assert!(c.physical_key().starts_with("usb:04B0:0456:D750:"));
        // 空/纯空白 serial 视为缺失。
        let blank = info(r"\\?\usb#vid_04b0&pid_0456#s3#{g}", "D750", Some("  "));
        assert!(blank.physical_key().starts_with("usb:"));
    }

    #[test]
    fn enumerate_dedupes_same_physical_camera_across_backends() {
        let list = vec![
            info(r"\\?\usb#vid_04b0&pid_0456#s1#{g}", "Z5II", Some("SN-1")), // 原生优先入口
            info("gphoto:usb:001,010", "Nikon Z5II", Some("SN-1")), // 同机 gphoto 端点
            info(r"\\?\usb#vid_04b0&pid_0456#s2#{g}", "Z5II", Some("SN-2")), // 同型不同机
        ];
        let deduped = dedupe_by_physical_identity(list);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].pnp_id, r"\\?\usb#vid_04b0&pid_0456#s1#{g}");
        assert_eq!(deduped[1].serial.as_deref(), Some("SN-2"));
    }

    // -----------------------------------------------------------------------
    // 自动后端选择（§E-2）
    // -----------------------------------------------------------------------

    struct SelectBackend {
        id: &'static str,
        caps: Capabilities,
        ok: bool,
        /// connect 返回的序列号（物理身份/账本键对齐用）。
        serial: &'static str,
        disconnects: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    impl CameraBackend for SelectBackend {
        fn id(&self) -> &'static str {
            self.id
        }
        fn name(&self) -> &'static str {
            self.id
        }
        fn capabilities(&self) -> Capabilities {
            self.caps
        }
        fn enumerate(&self) -> Result<Vec<CameraInfo>, TetherError> {
            Ok(Vec::new())
        }
        fn connect(&self, _: &str) -> Result<CameraInfo, TetherError> {
            if !self.ok {
                return Err(TetherError::Disconnected);
            }
            Ok(CameraInfo {
                pnp_id: "cam".into(),
                name: format!("{} cam", self.id),
                capabilities: self.caps,
                serial: Some(self.serial.into()),
                vid: None,
                pid: None,
            })
        }
        fn disconnect(&self, _: &str) {
            self.disconnects.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        fn capture_still(&self, _: &str, _: Duration) -> Result<CapturedObject, TetherError> {
            Err(TetherError::CaptureNotSupported)
        }
    }

    fn selecting_backend(
        id: &'static str,
        caps: Capabilities,
        ok: bool,
        serial: &'static str,
    ) -> std::sync::Arc<SelectBackend> {
        std::sync::Arc::new(SelectBackend {
            id,
            caps,
            ok,
            serial,
            disconnects: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        })
    }

    #[test]
    fn selection_prefers_first_capable_backend_in_registry_order() {
        // 原生连通但无拍摄能力 → 选下一个探测到拍摄能力的端点。
        let native = selecting_backend("native", Capabilities::FILE_TRANSFER, true, "s-native");
        let sdk = selecting_backend(
            "sdk",
            Capabilities::FILE_TRANSFER.union(Capabilities::STANDARD_CAPTURE),
            true,
            "s-sdk",
        );
        let gphoto = selecting_backend(
            "gphoto",
            Capabilities::FILE_TRANSFER.union(Capabilities::VENDOR_CAPTURE_NIKON),
            true,
            "s-gphoto",
        );
        let backends: Vec<std::sync::Arc<dyn CameraBackend>> =
            vec![native.clone(), sdk.clone(), gphoto.clone()];
        let (selected, info) = establish_backend(&backends, "cam", None).unwrap();
        assert_eq!(selected.id(), "sdk", "按探测能力选路，不按后端名");
        assert!(info.capabilities.standard_capture());
        // 无拍摄能力的原生端点曾连接作回退——一旦选中能力更强的端点，
        // 回退连接必须释放（同一相机不得并留两份设备句柄）。
        assert_eq!(
            native.disconnects.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            sdk.disconnects.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "选中者保持连接"
        );
    }

    #[test]
    fn selection_falls_back_to_first_connected_without_capture_and_skips_failed() {
        let broken = selecting_backend("broken", Capabilities::NONE, false, "s-broken");
        let plain = selecting_backend("plain", Capabilities::FILE_TRANSFER, true, "s-plain");
        let backends: Vec<std::sync::Arc<dyn CameraBackend>> = vec![broken.clone(), plain.clone()];
        let (selected, _) = establish_backend(&backends, "cam", None).unwrap();
        assert_eq!(selected.id(), "plain");
        let none: Vec<std::sync::Arc<dyn CameraBackend>> = vec![broken.clone()];
        let err = establish_backend(&none, "cam", None)
            .map(|_| ())
            .unwrap_err();
        assert!(err.contains("无法连接相机"), "{err}");
    }

    #[test]
    fn unacked_receipts_pin_the_backend_path() {
        let dir = tempfile::tempdir().unwrap();
        // 收据产生于 "pinned" 后端、相机 serial:s-pinned（与桩对齐）。
        let ledger =
            super::super::receipts_store::ReceiptsStore::open(dir.path(), "serial:s-pinned");
        ledger.record_pending("pinned", "o1", 1, "t");
        let capable = selecting_backend(
            "capable",
            Capabilities::FILE_TRANSFER.union(Capabilities::STANDARD_CAPTURE),
            true,
            "s-pinned",
        );
        let pinned_dead = selecting_backend(
            "pinned",
            Capabilities::FILE_TRANSFER.union(Capabilities::STANDARD_CAPTURE),
            false, // 产生收据的后端连不上
            "s-pinned",
        );
        let backends: Vec<std::sync::Arc<dyn CameraBackend>> =
            vec![capable.clone(), pinned_dead.clone()];
        let err = establish_backend(&backends, "cam", Some(dir.path()))
            .map(|_| ())
            .unwrap_err();
        assert!(err.contains("未确认"), "{err}");
        assert!(err.contains("pinned"), "{err}");
        // 有能力但非收据后端的端点必须被断开（不并留设备句柄）。
        assert_eq!(
            capable.disconnects.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        // 收据后端可用时：即使只有文件传输能力也直接选它（恢复优先）。
        let pinned_alive = selecting_backend("pinned", Capabilities::FILE_TRANSFER, true, "s-pinned");
        let backends: Vec<std::sync::Arc<dyn CameraBackend>> =
            vec![capable.clone(), pinned_alive.clone()];
        let (selected, _) = establish_backend(&backends, "cam", Some(dir.path())).unwrap();
        assert_eq!(selected.id(), "pinned");
    }
}
