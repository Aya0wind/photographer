//! Vendor protocol profiles: the single seam deciding which vendor protocol a
//! camera may run. OS transports (WPD today, ImageCaptureCore later) only
//! orchestrate bytes; every brand decision goes through a [`VendorProfile`]
//! implementation, because Nikon and Sony reuse the same numeric vendor ranges
//! (e.g. 0x9201-0x9207) with different meanings. Adding a vendor = implement
//! the trait + register it in [`profile`] — call sites never branch on brand
//! literals and never send another vendor's codes.

use super::backend::CameraSetting;
use super::ptp_control::{self as ptp, Brand, PtpTransport};

/// One vendor's protocol behaviour. Capability predicates must derive from the
/// advertised `OperationsSupported`, never from model names or hardcoded
/// trues; behaviour methods must refuse (Err) instead of partially guessing.
pub trait VendorProfile: Send + Sync {
    // 能力与行为按 brand 注册（profile(Brand) 即注册表本身，无自述方法）。

    // ---- 能力判定（按实测操作码集；默认全关） -------------------------------

    /// Live view support. Sony DeviceInfo may hide the vendor extension codes
    /// until the remote handshake runs — judge on the table returned by
    /// [`VendorProfile::expand_operations`], not on raw GetDeviceInfo.
    fn live_view_supported(&self, _operations: &[u16]) -> bool {
        false
    }

    /// Standalone autofocus drive support.
    fn autofocus_supported(&self, _operations: &[u16]) -> bool {
        false
    }

    /// Vendor D1xx/D2xx property codes participate in descriptor probing.
    fn vendor_properties_supported(&self) -> bool {
        false
    }

    /// Property read/write is handled inside the profile (vendor layout or
    /// remote session); false = standard GetDevicePropDesc/Set path.
    fn handles_properties(&self) -> bool {
        false
    }

    /// Capture runs the vendor-specific sequence; false = standard MTP path.
    fn vendor_capture(&self, _operations: &[u16]) -> bool {
        false
    }

    // ---- 行为（默认=明确不支持；能力与行为同源） -----------------------------
    // 厂商远端会话握手不设独立方法：握手幂等且内嵌于 expand_operations/
    // settings/set_setting（Sony 每次调用现场协商协议版本，无跨调用状态）。

    /// Full operation-code table once the vendor session is established.
    /// Some vendors (Sony) hide most extension codes behind the remote
    /// handshake, so capability gating must run on this merged table, not on
    /// raw GetDeviceInfo. Default: the advertised list unchanged; handshake
    /// errors propagate and the caller decides whether to keep the raw list.
    fn expand_operations(
        &self,
        _transport: &mut dyn PtpTransport,
        operations: &[u16],
    ) -> Result<Vec<u16>, String> {
        Ok(operations.to_vec())
    }

    /// Live view session factory: None = live view unsupported (caller must
    /// not call frame/stop). Implementations must agree with
    /// [`VendorProfile::live_view_supported`].
    fn live_view_session(
        &self,
        _operations: &[u16],
    ) -> Option<Box<dyn LiveViewSession + 'static>> {
        None
    }

    /// Read the in-scope camera settings through the vendor protocol.
    fn settings(&self, _transport: &mut dyn PtpTransport) -> Result<Vec<CameraSetting>, String> {
        Err("该相机参数走标准 PTP 描述".into())
    }

    /// Write one setting (raw token form, same contract as the backend trait).
    fn set_setting(
        &self,
        _transport: &mut dyn PtpTransport,
        _id: &str,
        _value: &str,
    ) -> Result<(), String> {
        Err("该相机参数走标准 PTP 写入".into())
    }

    /// Vendor capture sequence (still must be ingested via the receipt FIFO).
    fn capture(&self, _transport: &mut dyn PtpTransport) -> Result<(), String> {
        Err("该相机拍摄走标准序列".into())
    }

    /// Drive autofocus once.
    fn autofocus(
        &self,
        _transport: &mut dyn PtpTransport,
        _operations: &[u16],
    ) -> Result<(), String> {
        Err("该相机不支持对焦驱动".into())
    }

    /// Click-to-focus at normalized live-view coordinates (0..1, origin
    /// top-left). `live` is the most recent frame header when the connection
    /// owns a live view session — vendors that map clicks onto frame pixels
    /// need it for the conversion. Default: a plain autofocus drive.
    fn focus_at(
        &self,
        transport: &mut dyn PtpTransport,
        operations: &[u16],
        _x: f64,
        _y: f64,
        _live: Option<&ptp::LiveViewHeader>,
    ) -> Result<(), String> {
        self.autofocus(transport, operations)
    }
}

/// Live view state machine owned by a connection. Implementations keep
/// whatever per-session state they need (e.g. "we started it, we stop it").
pub trait LiveViewSession: Send {
    fn frame(&mut self, transport: &mut dyn PtpTransport) -> Result<Vec<u8>, String>;
    /// Stop only what this session started; must be callable on an already
    /// stopped session without error.
    fn stop(&mut self, transport: &mut dyn PtpTransport) -> Result<(), String>;
    /// 最近一帧的头部（画面尺寸/AF 框；点击对焦坐标换算依据）。无头部
    /// 概念的厂商（Sony 无状态取景）默认 None。
    fn last_header(&self) -> Option<ptp::LiveViewHeader> {
        None
    }
}

/// Nikon: full LiveView triple required, AF drive gated on 0x90C1, vendor
/// property codes participate; capture stays on the standard/MTP 0x9207 path
/// (`mtp::capture_command_for_operations`) — no vendor capture override.
struct NikonProfile;

impl VendorProfile for NikonProfile {
    fn live_view_supported(&self, operations: &[u16]) -> bool {
        [
            ptp::NIKON_LIVE_START,
            ptp::NIKON_LIVE_END,
            ptp::NIKON_LIVE_IMAGE,
        ]
        .iter()
        .all(|code| operations.contains(code))
    }
    fn autofocus_supported(&self, operations: &[u16]) -> bool {
        operations.contains(&ptp::NIKON_AUTOFOCUS)
    }
    fn vendor_properties_supported(&self) -> bool {
        true
    }
    fn autofocus(
        &self,
        transport: &mut dyn PtpTransport,
        operations: &[u16],
    ) -> Result<(), String> {
        if !self.autofocus_supported(operations) {
            return Err("该相机不支持对焦驱动".into());
        }
        ptp::nikon_autofocus(transport)
    }
    fn live_view_session(
        &self,
        operations: &[u16],
    ) -> Option<Box<dyn LiveViewSession + 'static>> {
        self.live_view_supported(operations).then(|| {
            Box::new(NikonLiveViewSession {
                inner: ptp::NikonLiveView::default(),
            }) as Box<dyn LiveViewSession>
        })
    }
    /// 点击对焦（真机行为证据链：libgphoto2 0x9205=两 u32 参数无数据相；
    /// digiCamControl 把显示点击按头部 4/6 全图尺寸换算成绝对像素后发送，
    /// 选区本身不驱动对焦——发送后补一发 0x90C1 AF 驱动才合焦）。
    /// 0x9205 未声明或还没有取景帧头部时退化为整帧 AF 驱动（与旧行为一致）。
    fn focus_at(
        &self,
        transport: &mut dyn PtpTransport,
        operations: &[u16],
        x: f64,
        y: f64,
        live: Option<&ptp::LiveViewHeader>,
    ) -> Result<(), String> {
        if !self.autofocus_supported(operations) {
            return Err("该相机不支持对焦驱动".into());
        }
        if let Some(header) = live {
            if operations.contains(&ptp::NIKON_FOCUS_AREA) {
                let (px, py) = ptp::click_focus_offset(x, y, header)?;
                ptp::nikon_change_af_area(transport, px, py)?;
            }
        }
        ptp::nikon_autofocus(transport)
    }
}

/// Nikon live view session adapter over the ptp_control state machine.
struct NikonLiveViewSession {
    inner: ptp::NikonLiveView,
}

impl LiveViewSession for NikonLiveViewSession {
    fn frame(&mut self, transport: &mut dyn PtpTransport) -> Result<Vec<u8>, String> {
        self.inner.frame(transport)
    }
    fn stop(&mut self, transport: &mut dyn PtpTransport) -> Result<(), String> {
        self.inner.stop(transport)
    }
    fn last_header(&self) -> Option<ptp::LiveViewHeader> {
        self.inner.last_header().cloned()
    }
}

/// Sony: SDIO remote handshake before property traffic, vendor property
/// layout and capture sequence; live view reads the magic preview object.
struct SonyProfile;

impl VendorProfile for SonyProfile {
    fn live_view_supported(&self, operations: &[u16]) -> bool {
        // 扩展码多藏在握手之后（OperationSet 经 0x9202 展开）——能力判定
        // 必须用 expand_operations 展开后的表，不能直接信 GetDeviceInfo。
        super::sony_ptp::capabilities_supported(operations).live_view
    }
    fn vendor_capture(&self, operations: &[u16]) -> bool {
        super::sony_ptp::capabilities_supported(operations).capture
    }
    fn handles_properties(&self) -> bool {
        true
    }
    fn expand_operations(
        &self,
        transport: &mut dyn PtpTransport,
        operations: &[u16],
    ) -> Result<Vec<u16>, String> {
        let session = super::sony_ptp::init_remote_detailed(transport)?;
        let fresh: Vec<u16> = session
            .ext_codes
            .iter()
            .copied()
            .filter(|code| !operations.contains(code))
            .collect();
        let mut merged = operations.to_vec();
        merged.extend(fresh);
        Ok(merged)
    }
    fn settings(&self, transport: &mut dyn PtpTransport) -> Result<Vec<CameraSetting>, String> {
        // 握手幂等（上游每会话重发全套）；协议版本现场协商，无跨调用状态。
        let session = super::sony_ptp::init_remote_detailed(transport)?;
        super::sony_ptp::settings(transport, session.protocol)
    }
    fn set_setting(
        &self,
        transport: &mut dyn PtpTransport,
        id: &str,
        value: &str,
    ) -> Result<(), String> {
        let session = super::sony_ptp::init_remote_detailed(transport)?;
        super::sony_ptp::set_setting(transport, session.protocol, id, value)
    }
    fn capture(&self, transport: &mut dyn PtpTransport) -> Result<(), String> {
        super::sony_ptp::capture(transport)
    }
    fn live_view_session(
        &self,
        operations: &[u16],
    ) -> Option<Box<dyn LiveViewSession + 'static>> {
        self.live_view_supported(operations)
            .then(|| Box::new(SonyLiveViewSession) as Box<dyn LiveViewSession>)
    }
}

/// Sony live view has no start/stop commands (upstream `camera_preview`):
/// every frame is a stateless magic-object read.
struct SonyLiveViewSession;

impl LiveViewSession for SonyLiveViewSession {
    fn frame(&mut self, transport: &mut dyn PtpTransport) -> Result<Vec<u8>, String> {
        super::sony_ptp::live_view_frame(transport)
    }
    fn stop(&mut self, _transport: &mut dyn PtpTransport) -> Result<(), String> {
        Ok(())
    }
}

/// Standard PTP: everything defaulted (no vendor codes, no session).
struct StandardProfile;

impl VendorProfile for StandardProfile {
}

static NIKON: NikonProfile = NikonProfile;
static SONY: SonyProfile = SonyProfile;
static STANDARD: StandardProfile = StandardProfile;

/// Registry lookup — the one and only brand match in the vendor layer.
pub fn profile(brand: Brand) -> &'static dyn VendorProfile {
    match brand {
        Brand::Nikon => &NIKON,
        Brand::Sony => &SONY,
        Brand::Standard => &STANDARD,
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const NIKON_LIVEVIEW_OPS: &[u16] = &[0x9201, 0x9202, 0x9203, 0x90c1, 0x9205];

    #[test]
    fn registry_returns_per_brand_profiles() {
        // 注册表身份经行为验证（无自述 brand 方法）：取景门槛 Nikon 专属、
        // 参数布局 Sony 自管、Standard 全默认。
        assert!(profile(Brand::Nikon).live_view_supported(NIKON_LIVEVIEW_OPS));
        assert!(!profile(Brand::Sony).live_view_supported(NIKON_LIVEVIEW_OPS));
        assert!(profile(Brand::Sony).handles_properties());
        assert!(!profile(Brand::Nikon).handles_properties());
        assert!(!profile(Brand::Standard).handles_properties());
    }

    #[test]
    fn live_view_requires_nikon_brand_and_all_three_operations() {
        assert!(profile(Brand::Nikon).live_view_supported(NIKON_LIVEVIEW_OPS));
        // Same numeric codes on a Sony endpoint must not enable Nikon live view.
        assert!(!profile(Brand::Sony).live_view_supported(NIKON_LIVEVIEW_OPS));
        assert!(!profile(Brand::Standard).live_view_supported(NIKON_LIVEVIEW_OPS));
        // Incomplete operation set → disabled, no partial vendor commands.
        for missing in [0x9201u16, 0x9202, 0x9203] {
            let partial: Vec<u16> = NIKON_LIVEVIEW_OPS
                .iter()
                .copied()
                .filter(|code| *code != missing)
                .collect();
            assert!(!profile(Brand::Nikon).live_view_supported(&partial));
        }
    }

    #[test]
    fn autofocus_is_nikon_only_and_operation_gated() {
        assert!(profile(Brand::Nikon).autofocus_supported(NIKON_LIVEVIEW_OPS));
        assert!(!profile(Brand::Sony).autofocus_supported(NIKON_LIVEVIEW_OPS));
        assert!(!profile(Brand::Nikon).autofocus_supported(&[]));
    }

    #[test]
    fn vendor_property_codes_are_nikon_only() {
        assert!(profile(Brand::Nikon).vendor_properties_supported());
        assert!(!profile(Brand::Sony).vendor_properties_supported());
        assert!(!profile(Brand::Standard).vendor_properties_supported());
    }

    struct NoopTransport;
    impl PtpTransport for NoopTransport {
        fn read(&mut self, _: u16, _: &[u32], _: usize) -> Result<(u32, Vec<u8>), String> {
            Ok((0x2001, Vec::new()))
        }
        fn write(&mut self, _: u16, _: &[u32], _: &[u8]) -> Result<u32, String> {
            Ok(0x2001)
        }
        fn command(&mut self, _: u16, _: &[u32]) -> Result<u32, String> {
            Ok(0x2001)
        }
    }

    #[test]
    fn autofocus_dispatch_refuses_unsupported_combinations() {
        let mut transport = NoopTransport;
        assert!(profile(Brand::Nikon)
            .autofocus(&mut transport, &[])
            .is_err());
        assert!(profile(Brand::Sony)
            .autofocus(&mut transport, NIKON_LIVEVIEW_OPS)
            .is_err());
        assert!(profile(Brand::Standard)
            .autofocus(&mut transport, NIKON_LIVEVIEW_OPS)
            .is_err());
    }

    #[test]
    fn standard_profile_refuses_every_vendor_behaviour() {
        let mut transport = NoopTransport;
        let p = profile(Brand::Standard);
        assert!(p.settings(&mut transport).is_err());
        assert!(p.set_setting(&mut transport, "iso", "100").is_err());
        assert!(p.capture(&mut transport).is_err());
        assert!(p.autofocus(&mut transport, &[]).is_err());
        // 无会话概念的厂商操作码表原样透传（不握手）。
        assert_eq!(
            p.expand_operations(&mut transport, &[0x1001]).unwrap(),
            vec![0x1001]
        );
    }

    #[test]
    fn live_view_session_factory_matches_capability() {
        assert!(profile(Brand::Nikon)
            .live_view_session(NIKON_LIVEVIEW_OPS)
            .is_some());
        assert!(profile(Brand::Nikon).live_view_session(&[]).is_none());
        assert!(profile(Brand::Standard)
            .live_view_session(NIKON_LIVEVIEW_OPS)
            .is_none());
    }

    #[test]
    fn nikon_focus_at_selects_the_area_then_drives_af() {
        struct LogTransport {
            commands: Vec<(u16, Vec<u32>)>,
        }
        impl PtpTransport for LogTransport {
            fn read(&mut self, _: u16, _: &[u32], _: usize) -> Result<(u32, Vec<u8>), String> {
                unreachable!()
            }
            fn write(&mut self, _: u16, _: &[u32], _: &[u8]) -> Result<u32, String> {
                unreachable!()
            }
            fn command(&mut self, operation: u16, params: &[u32]) -> Result<u32, String> {
                self.commands.push((operation, params.to_vec()));
                Ok(0x2001)
            }
        }
        let header = ptp::LiveViewHeader {
            af_regions: Vec::new(),
            size: Some((640, 480)),
            image_size: Some((4032, 3024)),
        };
        let operations = [ptp::NIKON_AUTOFOCUS, ptp::NIKON_FOCUS_AREA];

        // 有帧头 + 声明 0x9205：先选区（全图像素坐标）再驱动 AF。
        let mut transport = LogTransport { commands: Vec::new() };
        profile(Brand::Nikon)
            .focus_at(&mut transport, &operations, 0.25, 0.75, Some(&header))
            .unwrap();
        assert_eq!(
            transport.commands,
            vec![
                (ptp::NIKON_FOCUS_AREA, vec![1008, 2268]),
                (ptp::NIKON_AUTOFOCUS, vec![1]),
            ]
        );

        // 未声明 0x9205（老机身）→ 只有 AF 驱动，不发区域指令。
        let mut transport = LogTransport { commands: Vec::new() };
        profile(Brand::Nikon)
            .focus_at(&mut transport, &[ptp::NIKON_AUTOFOCUS], 0.25, 0.75, Some(&header))
            .unwrap();
        assert_eq!(transport.commands, vec![(ptp::NIKON_AUTOFOCUS, vec![1])]);

        // 没有帧头（还没取过帧）→ 退化为 AF 驱动，不猜测坐标。
        let mut transport = LogTransport { commands: Vec::new() };
        profile(Brand::Nikon)
            .focus_at(&mut transport, &operations, 0.25, 0.75, None)
            .unwrap();
        assert_eq!(transport.commands, vec![(ptp::NIKON_AUTOFOCUS, vec![1])]);

        // 无 AF 驱动操作码 → 一律拒绝（选区后无法合焦）。
        let mut transport = LogTransport { commands: Vec::new() };
        assert!(profile(Brand::Nikon)
            .focus_at(&mut transport, &[ptp::NIKON_FOCUS_AREA], 0.5, 0.5, Some(&header))
            .is_err());
        assert!(transport.commands.is_empty());
    }

    /// Sony 扩展码经握手展开：隐藏操作码并入后能力判定才翻正。
    #[test]
    fn sony_expand_operations_merges_hidden_ext_codes() {
        // NoopTransport 对所有读取回 OK+空数据：0x9202 空数据=无扩展码，
        // 握手应成功并原样返回操作码表（幂等路径）。
        let mut transport = NoopTransport;
        let base = [0x1001u16, 0x9201];
        let expanded = profile(Brand::Sony)
            .expand_operations(&mut transport, &base)
            .unwrap();
        assert_eq!(expanded, base.to_vec());
        // Nikon/Standard 默认透传（不握手）。
        let mut transport = NoopTransport;
        assert_eq!(
            profile(Brand::Nikon)
                .expand_operations(&mut transport, &base)
                .unwrap(),
            base.to_vec()
        );
    }
}
