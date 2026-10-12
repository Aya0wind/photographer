# Native tethering implementation

Objective: system-native camera connections, brand-specific camera protocols,
optional vendor SDK adapters, and libgphoto2 as a fallback. Never replace USB
drivers automatically or require SDK development packages from end users.

## Completion requirements

- [ ] Separate portable PTP protocol handling from OS transport.
- [ ] Windows WPD supports shutter/aperture/ISO, focus, white balance and readback confirmation.
      User scope: no other advanced adjustable parameters are required.
- [ ] Nikon native live view, autofocus, media capture, and camera-side capture ingestion.
- [ ] Sony native PTP remote-session initialization, properties, capture and live view.
- [ ] macOS ImageCaptureCore transport implements the same protocol interface.
- [ ] Backend selection uses verified device capabilities, keeps SDK adapters available,
      and falls back to libgphoto2 without changing drivers.
- [ ] A physical camera is owned by only one session/transport at a time.
- [ ] Camera-side bursts and RAW+JPG are queued; committed objects are acknowledged,
      failures remain retryable; no original camera file is deleted.
- [ ] Disconnect/reconnect and shutdown release native subscriptions and live view.
- [ ] Settings changes from either side converge without polling every property per frame.
- [ ] Packaging includes native bridges and redistributable runtime dependencies;
      SDK support is conditional on actual runtime presence and distribution permission.
- [ ] Protocol/transport/lifecycle tests plus real Nikon and Sony checks; macOS compilation
      and hardware checks before claiming cross-platform completion.

## Evidence already obtained

2026-10-11: Windows native WPD successfully discovered Z5II, triggered Nikon media
capture (0x9207, parameters 0xFFFFFFFF/0), and read DSC_8662.JPG (8,086,491 bytes).
This is evidence for native single-photo capture only. It does not prove property
control, live view, bursts, Sony support, or macOS support.

## Architecture

Keep CameraBackend as the application contract. A portable protocol layer handles
PTP values, property descriptors, brand profiles, event records and preview payloads.
OS adapters own USB/COM/ImageCaptureCore lifetimes. All operations for one camera
are serialized; photo receipts and registration use the existing session pipeline.
Capability discovery must not trigger the shutter or silently change user settings.

Native discovery is preferred. SDK and libgphoto2 endpoints remain candidates, not
proof of control support. Fallback occurs at session setup, never halfway through an
unacknowledged capture. Do not merge devices merely because their names match.

## 交接说明（2026-10-11，本目标尚未完成）

### 用户最终要求

1. 系统原生连接 + 按品牌适配协议，厂商 SDK 补充，libgphoto2 后备。
2. 不要求用户手动替换驱动；换电脑、换受支持相机，安装应用后尽量插上即用。
3. 可调参数只做快门、光圈、ISO、对焦、白平衡。其他高级参数暂时不做。
4. 仍需实时取景、应用触发快门、机身按快门自动收片、连续拍摄顺序接收、
   RAW+JPG 全部接收、自动加入选择的相册、不单独创建导入任务。
5. 应用与机身参数双向同步；插拔、意外关窗、切库、结束会话正确释放资源。
6. 用户要求把计划和验证情况留在这个 Markdown 中，以便其他模型继续。

这些要求不意味着所有未知相机都支持遥控。最终应有已验证的机型/固件/系统
能力列表，对缺失功能准确禁用，而不是硬编码所有能力为 true。

### 当前已写入代码

以下均为未提交工作区内容；工作区同时有大量用户与先前任务修改，不能重置。

| 文件 | 作用/当前状态 |
|---|---|
| `src-tauri/src/tethering/ptp_control.rs` | 新增跨平台 PTP 传输 trait、参数描述解析、类型/枚举/范围校验、写入编码、快门/光圈显示、JPEG 取景帧提取。只包含基本参数白名单。 |
| `src-tauri/src/platform/windows/wpd_control.rs` | 新增 WPD transport 实现；读取参数描述、三要素/白平衡/对焦模式列表、SetDevicePropValue 数据写入、尼康 autofocus 动作。已接入后端，真实参数功能尚未验证。 |
| `src-tauri/src/platform/windows/wpd_backend.rs` | 先前修复数据读取开始/结束相位、接收缓冲、HRESULT、结束传输；新增 settings/set_setting 转发，仍用有超时的 WPD worker。 |
| `src-tauri/src/tethering/mtp.rs` | 先前修复尼康拍摄参数；按支持的操作优先用 0x9207 拍到卡，之后标准拍摄，再旧 SDRAM 拍摄。 |
| `src-tauri/src/tethering/mod.rs` | 注册 ptp_control 模块。 |
| `src/features/tethering/TetherStartDialog.tsx` | WPD 设备在进入联机选择时主动探测，避免未探测时只显示传文件能力、按钮永久禁用。 |
| `src/features/tethering/TetheringWindowPage.tsx` | 快捷参数当前值优先显示 option.label，底层仍提交原始 value，避免用户看见快门/光圈编码数字。 |

读取参数目前以 GetDevicePropDesc 自带的 current 为准。写入后利用已有
`Session::set_setting` 回读确认循环（最多 1.5 秒）同步，不能只乐观修改 UI。
对焦动作不需要等待 current 变成 1，已有 Action 类型分支会确认动作。

### 实际验证结果及边界

- 先前 Z5II (`VID_04B0/PID_0456`，序列号 `0000008007062`) 保持微软
  `WUDFWpdMtp` / `wpdmtp.inf`，不用换驱动，探测成功。
- 单次真机快门成功：0x9207 参数 `[0xFFFFFFFF, 0]`，收到 `DSC_8662.JPG`，
  8,086,491 字节完整读取，整个测试约 0.68 秒；原件未删，读取写到 sink。
  这是「快门 + 文件读取」证据，不是「真实相册入库 + 连拍 + RAW/JPG」全链路证据。
- 本轮 Rust `cargo test --lib tethering`：30 通过、3 个真机用例忽略。
- 本轮 TypeScript 检查通过；两个联机窗口前端测试文件共 30 用例通过。
- 最终 `cargo check --offline --manifest-path src-tauri/Cargo.toml --tests` 通过，含集成测试编译。
- `live_wpd_settings` 真机测试失败：`WPD error 0x80070002`；随后只读查询
  Windows 在线设备，尼康与索尼都不在列表中。说明当前设备已离线，不能宣称
  新参数读写/对焦已通过真机验证。重新接机后优先做这一项。
- 标准/尼康对焦模式显示标签已对照上游修正；之后又运行 4 项协议测试全部通过（属于上面 30 项的子集）。
- GET_PROP_VALUE、NIKON_LIVE_*、preview_jpeg 已用于原生取景；目前未用常量为 FOCUS_AREA/EVENTS（这两项未接通）。已有 LNK4098 链接警告并非本轮引入。

### 明确尚未实现/存在缺口

- Windows 尼康实时取景现已接入；只有声明完整 9201/9202/9203 指令的尼康端点才报告 LiveView。仅经过模拟 transport 测试，尚未经过真机帧验证。点击取景对焦仍未接入。
- WPD 已改为持续原生连接/Advise 和 receipt FIFO，机身事件、应用快门均走此队列；RAW+JPG 与重试顺序有纯函数测试，真实连拍及文件入库仍待硬件验收。旧单值 ObjectAddedWait 已移除。
- 队列目前仅保留在进程内，支持关闭/重开会话后续收；应用进程崩溃后的待收恢复/持久化日志仍未实现。不能宣称崩溃情况下已经完整保障。
- 新参数读写需要真机验证。`descriptors()` 暂时跳过读取失败的单个属性，后续
  要区分不支持与连接失败，避免把所有连接错误静默降级为空列表。
- Property Range 虽有写入校验，但 UI 当前只列出 current，需把有限范围展开为
  选项或提供正确范围控件；不能宣称范围型属性已完整可调。
- 白平衡还有品牌扩展枚举/色温标签未完善；不要按索尼/尼康共用数字猜含义。
- Sony 原生遥控初始化与参数批量数据格式未做，现有 Sony SDK bridge 与
  libgphoto2 路径仅保留可用功能。
- macOS `platform/macos/wpd.rs`、`wpd_backend.rs` 仍转发 unsupported 桩。
  ImageCaptureCore transport、桥的打包与真机验证都还没做。
- 后端当前按 camera id 前缀选 (`wpd-mtp`/`sony-sdk`/`gphoto`)；不是按实际
  能力自动选择，物理设备跨后端去重和独占也未完成。
- 厂商 SDK 自动打包未做。`docs/tethering-sdks.md` 的许可证总述与后文存在
  不一致，接 SDK 发布时须查当期协议，不能把其中一句当成事实。

## 建议续做顺序

### A. 先完成 Windows 尼康（最接近可验证）

1. 重新连接 Z5II，菜单保持 MTP/PTP；运行只读 `live_wpd_settings`。
2. 看真实 PropertyDesc 的类型、可写位与 enum。核实快门 D100、光圈 5007、
   ISO 500F、白平衡 5005、对焦模式 D061/500A，不盲目假设机型一致。
3. 在支持的选项间临时修改并读回，再恢复原值；测试应有恢复保护，失败也恢复。
   同时用机身改变这些参数，验证窗口可同步。
4. 将设备 profile 与品牌协议逻辑移出 WPD 文件。现有 `PtpTransport` 是 seam，
   Windows COM 与 macOS helper 实现它，PTP 数据布局/格式解析共用。
5. Nikon LiveView 指令：启动 9201、结束 9202、取帧 9203；AF 动作 90C1，
   改 AF 区域 9205。依据实际 OperationsSupported 决定，错误保留响应码。
6. 改 `execute_with_data_to_read/read_payload` 接受按用途指定的字节上限。
   当前 256KB 上限用于 DeviceInfo，不能直接套到取景帧（可合理限 16MB）。
7. 镜头/对焦区坐标不能只乘缩略图尺寸猜测：核对尼康实时取景头里的坐标系、
   原始画面尺寸、方向、裁剪；先做 autofocus，再做有证据的点击对焦。
8. 记录是否由应用开启 LiveView，结束会话只恢复自己改变的状态，不破坏用户
   本来已经开启的状态；读取/写入/快门互斥，忙/暂时无帧不等于拔线。

### B. 持久事件队列与多张收片

1. 同一相机的持续 WPD device/Advise 要归固定 worker 线程；COM 接口不能
   直接放进跨线程 Arc。可使用该线程的 thread_local native connection。
2. COM 初始化的生命周期必须覆盖 TLS 连接析构；disconnect 先在拥有线程
   Unadvise、结束自己开的 LiveView、Close，再 invalidate worker。
3. 回调只投递 ObjectID 到 FIFO 队列并唤醒；不做文件读取、数据库、缩略图。
   过滤目录/非照片事件时也要保留后续已排队的照片事件。
4. `poll_objects` 交付待接收对象；应用快门与机身快门共享队列、统一去重。
   RAW+JPG 的第二个事件不能因第一个已返回而丢掉。
5. 未确认的 receipt 不应因为一次 poll 或传输失败被移除；成功入库后才
   `acknowledge_captured`。失败保留并限速重试，不循环刷错误、不删除相机原件。
6. 复用 `session.rs::receive/ingest_from_backend/ingest`。照片进入所选相册并
   跟进索引；不创建独立导入任务。窗口 64 张最近缩略图限制不等于接收上限。
7. 设置事件只标记失效/版本；必要时合并读取这几项参数。没有可靠参数事件的
   相机保留低频兜底刷新，不能每一帧读取所有参数。

### C. Sony 原生协议

1. 用未替换驱动的 Windows 机器验证 Sony 遥控模式在 WPD 可见的接口。
   当前开发机以前装过 libusbK，现有 libgphoto2 可用不能证明新机器开箱即用。
2. 用 Sony Camera Remote Command 公开文档与 libgphoto2 Sony 适配代码核对
   SDIO 连接阶段、支持指令查询、属性批量描述、增量状态、快门、取景帧。
3. 只把同一基本参数范围投影到 CameraSetting；机型不同则按 profile 适配。
4. Sony 与 Nikon 可能使用同数值的厂商操作码表示不同功能。必须先确定厂商，
   不能仅凭发现 9201/9202 就发尼康 LiveView 指令。
5. 系统原生访问不可用的机型，试已有 Sony SDK bridge；SDK 不存在则准确降级。
   SDK 是否也需专门驱动必须核实，不能把 SDK 当作无需驱动的保证。

### D. macOS 与打包

1. ImageCaptureCore 提供 `ICCameraDevice.requestSendPTPCommand`，用于原生
   PTP 传输；保持相机 discovery/open/close、取景、事件在正确 run loop。
2. 可做随包 Objective-C/Swift helper，通过结构化 RPC 与 Rust 通信，也可用
   Rust ObjC 绑定；先核对已有 Cargo/CI 工具链后选一种，不把环境步骤交给用户。
3. transport 复用品牌协议；原生文件读取/事件映射保持同样的 receipt/ack 契约。
4. SDK bridge 原文件动态发现，缺席不能阻断原生连接。libgphoto2 最后备用，
   仅使用已有可访问接口，不自动装 WinUSB/libusbK、不改用户驱动。
5. CI 只能证明构建/打包，macOS 真机仍需单独验证。当前环境是 Windows。

### E. 自动选择和完整验收

- 为物理相机建立稳定身份（VID/PID、serial、设备实例）；端点 id 仍可保留
  现有前缀。不能只按名称合并两台同型号相机。
- 原生 → 已安装且可用的 SDK → 可访问的 libgphoto2；按已验证能力选路，
  不能用硬编码的后端能力上限当机型能力。原生缺部分功能时准确显示限制。
- 库/相册业务只持 CameraBackend，不了解具体 driver。后端回退只在连接时
  进行，存在未确认照片时禁止无说明切换路径。
- 跨别名独占同一物理相机；现有 `SessionStore` 只按 camera id 去重，不够。
- 测试 app/相机启动乱序、USB 拔插、关闭窗口/意外关闭、停止后立即重开、
  相机按快门多张、应用连点快门、RAW+JPG、NAS 慢写、参数在两边修改。
- 未通过全部要求前不能标记整个目标完成。本轮只有部分协议/Windows 接入。

## 续做工具与命令

项目 `I:\projects\smart_photo`，PowerShell。没有提交/推送授权，不自行提交。
大量无关工作区修改必须保留。修改 Rust 会触发已有 Tauri dev watcher，编译
可能等待 cargo 锁；观察原进程/会话，不重复启动、不随便杀应用。

```powershell
$env:PATH='C:\Users\12003\AppData\Local\bin\NASM;'+$env:PATH
$env:CARGO_INCREMENTAL='0'
cargo test --offline --manifest-path src-tauri/Cargo.toml --lib tethering -- --nocapture
cargo check --offline --manifest-path src-tauri/Cargo.toml --tests
& C:\Users\12003\.local\nodejs\node.exe node_modules/typescript/bin/tsc --noEmit
& C:\Users\12003\.local\nodejs\node.exe node_modules/vitest/vitest.mjs run src/features/tethering/TetherStartDialog.test.tsx src/features/tethering/TetheringWindowPage.test.tsx
```

真机用例都带 ignore，不能随普通测试误触发快门。重新枚举在线相机后设置
`PHOTOGRAPHER_TEST_CAMERA` 为真实 WPD 路径，再运行 `live_wpd_settings --ignored`。
`live_wpd_capture_and_read --ignored` **会拍一张照片**，仅在需要验证且已告知用户时运行。
必要的系统设备查询/COM 实测可能需工具 require_escalated；此前读取授权不是
更换驱动授权。用户已经明确要求不依赖换驱动，不能再沿 Zadig 方案推进。

## 参考（仅依赖官方文档/上游实现）

- Microsoft WPD MTP begin/read/write/end：
  https://learn.microsoft.com/en-us/windows/win32/wpd_sdk/wpd-command-mtp-ext-execute-command-with-data-to-read
  https://learn.microsoft.com/en-us/windows/win32/wpd_sdk/wpd-command-mtp-ext-execute-command-with-data-to-write
  https://learn.microsoft.com/en-us/windows/win32/wpd_sdk/wpd-command-mtp-ext-write-data
  https://learn.microsoft.com/en-us/windows/win32/wpd_sdk/wpd-command-mtp-ext-end-data-transfer
- Apple 原生 PTP： https://developer.apple.com/documentation/imagecapturecore/iccameradevice
- Sony 协议与 SDK： https://support.d-imaging.sony.co.jp/app/sdk/en/index.html
- Nikon SDK 版本/支持列表： https://sdk.nikonimaging.com/information/en/
- 上游协议布局与机型差异：
  https://github.com/gphoto/libgphoto2/blob/master/camlibs/ptp2/ptp.h
  https://github.com/gphoto/libgphoto2/blob/master/camlibs/ptp2/ptp-pack.c
  https://github.com/gphoto/libgphoto2/blob/master/camlibs/ptp2/config.c
  https://github.com/gphoto/libgphoto2/blob/master/camlibs/ptp2/library.c

下载许可不能凭记忆概括；不从其他摄影软件安装目录提取 SDK。运行时包合法
可再分发时随应用打包，开发头文件不交给最终用户。


## 自动续做进度补记：Windows 持续收片/取景

本阶段新增：
- src-tauri/src/tethering/receipts.rs：FIFO、去重、ack 后移除、失败保留与退避重试；纯函数验证 RAW/JPG 四文件顺序。
- src-tauri/src/platform/windows/wpd_native.rs：固定 worker 上 TLS 原生连接，持续 Advise，COM apartment 覆盖 TLS 连接析构，回调只排对象 ID。
- WPD connect/poll/trigger_capture/acknowledge_captured/live_view_frame/disconnect 接入上述连接；读取属性暂仍经过独立同通道 WPD handle。
- 相机已有会话时 camera_probe 直接返回会话信息；新增 trait probe 区分只读发现和连接，防止探测留下未拥有的 SDK/gphoto 连接。
- NikonLiveView 状态机已放入 ptp_control.rs，可由 macOS transport 复用；模拟 20 帧仅查询一次状态，不关闭机身原来已开的 LiveView。
- 大图读取使用按用途指定的上限，尼康取景上限16MB；读取/写入失败仍结束传输。
- 原生 receipt 对于未就绪文件名/零字节保持待收；目录通知不会吞掉后续照片。

注意：类型/模拟测试通过不能证明真实 ObjectAdded 回调在连拍时都及时到达。
下一步重接 Z5II 后运行 live_native_preview（不会拍照，但会启动/结束应用拥有的 LiveView），
再验证机身连拍、RAW/JPG、NAS 慢写及参数修改。仍需 Sony、macOS、SDK 打包、自动后端选择、
跨端点物理身份独占及进程重启后的待收恢复，目标未完成。

本阶段最终验证：Rust 联机单元测试 33 通过、4 个硬件用例忽略；TypeScript 检查及两个联机前端测试文件 30 项通过。Windows 在线 PnP 查询没有尼康/索尼设备，因此新增原生取景、持续回调及真实连拍未硬件验收。集成测试编译检查通过后也不能替代真实相机验证。

## 自动续做进度补记 2：厂商抽象层 + Sony 原生接线 + 点击对焦（2026-10-09）

### 本阶段新增（全部为未提交工作区代码）

**厂商抽象层（用户要求：不逐机型写死，可扩展）**

- `src-tauri/src/tethering/vendor_dispatch.rs` 重写为 `VendorProfile` trait + 静态注册表
  （`profile(Brand) -> &'static dyn VendorProfile`）。能力判定
  （live_view/autofocus/vendor_properties/handles_properties/vendor_capture）与行为
  （expand_operations/settings/set_setting/capture/autofocus/focus_at/live_view_session）
  全部收敛到 profile；调用点（wpd_control/wpd_native/wpd_backend）零品牌字面量。
  新增厂商 = 实现 trait + 在注册表加一行。
- `LiveViewSession` trait（frame/stop）泛化取景状态机：Nikon 有状态（只停自己开的
  LiveView），Sony 无状态（每帧读魔术对象 0xFFFFC002），连接建立时由
  `profile.live_view_session(ops)` 工厂选择，None=不支持。
- `expand_operations`：Sony 扩展操作码藏在 0x9202 握手之后，能力判定必须用握手
  展开后的码表。probe_device 与原生 connect 都先展开再判能力；握手失败退回原始
  表按保守能力报告，不阻断连接。诊断日志记录 vendor 家族。

**Sony 原生协议接线（占位桩删除）**

- `src-tauri/src/tethering/sony_ptp.rs`（新模块，29 项模拟 transport 测试）：
  SDIO 握手（0x9201 三阶段 + 0x9202 扩展查询 + PriorityMode）、批量属性描述
  （0x9209，失败退 0x9203）、写入（0x9205 退 0x1016）、拍摄（半按→全按→等
  0xD213→释放）、无状态取景。全部对照 libgphoto2 公开协议事实实现，协议层不含
  OS 代码（WPD/macOS 均可承载）。
- WPD 侧接线：参数读写经 `profile.settings/set_setting`（Sony 自管布局；
  Nikon/标准走 GetDevicePropDesc）；拍摄经 `profile.capture`（Sony 厂商序列，
  对象仍统一走 Advise→receipt FIFO，与机身快门同路径）。
- V2 协议（2020 前机型）快门/ISO/光圈步进写入禁用（TODO-verify）。

**点击对焦后端**

- `tethering_focus_at` IPC 命令此前已有前端调用（归一化坐标），本阶段补上 WPD
  后端实现：`com::control::focus_at` → `profile.focus_at`。Nikon 当前退化为整帧
  AF 驱动——0x9205 区域写的坐标系未经真机核实前不发送坐标（TODO-verify，§A7）。

**传输参数放宽**：ptp_control/sony_ptp 的 `&mut impl PtpTransport` 改为
`&mut (impl PtpTransport + ?Sized)`（15 处），协议函数同时接受静态 mock 与
`&mut dyn`（profile 层是 dyn 边界）。

### 本阶段验证

- `cargo test --lib tethering`：110 通过、4 硬件用例忽略（此前 107）。
- `cargo test --test tethering_test`：220 通过、7 忽略（此前 217）。
- `cargo check --all-targets`：仅剩 2 个既有已记录未接通常量警告
  （NIKON_EVENTS/NIKON_FOCUS_AREA，即上文「实际验证结果」所述未接通项）。
- 前端无改动（focus_at 前端在上一 agent 阶段已完成）。

### 仍未完成（诚实清单，接机后逐项验收）

1. **全部真机验收缺口**（本机两台相机均离线，无法验证）：
   - Z5II：参数读回/写入恢复、AF 驱动、取景真帧、连拍顺序接收、RAW+JPG 双事件、
     NAS 慢写、机身改参同步（`live_wpd_settings --ignored` 只读先行）。
   - Sony：真机遥控模式下的 WPD 可见性（§C1，注意开发机装过 libusbK 的干扰）、
     握手/参数/拍摄/取景全链路。
2. **macOS**：`platform/macos/ic_transport.rs`（objc2 ImageCaptureCore transport）
   仅在 macOS CI 编译，本环境无法编译验证；11 项 TODO-verify + 真机未做。
3. **打包与许可**：SDK 条件打包未做；`docs/tethering-sdks.md` 总述与后文不一致，
   接 SDK 发布时必须查当期协议（不可把其中一句当事实）。
4. 硬件连拍/拔插/意外关窗等场景测试（§E 清单）全部待真机。
   （§E 的软件面已具备：物理键 serial 优先、枚举跨后端去重、start() 存储锁下
   原子独占判定+冲突释放连接，均有测试；后端选择 = 注册表顺序 + 未确认收据
   钉住产生收据的后端。）

结论：软件可实现面已完成并全测试绿；真机与跨平台项未验证，目标整体未完成，
不得标记完成。

## 自动续做进度补记 3：点击对焦真坐标（2026-10-09）

### 用户报告

真机点击取景画面对焦时，对焦点位置与点击位置不一致。根因：旧实现
`focus_at` 只发整帧 AF 驱动（0x90C1），从不发送坐标——相机在原有 AF 区域
合焦，自然与点击点偏差。

### 证据链（对照上游公开实现，非猜测）

- libgphoto2 ptp.h:604 / ptp.c：`PTP_OC_NIKON_ChangeAfArea = 0x9205`，
  **两个 u32 参数 (x, y)、无数据相**（`ptp_generic_no_data(..., 2, x, y)`）；
  仅取景模式有效（0xA00B NotLiveView）；config.c 变更后曾计划 afdrive +
  wait busy。
- digiCamControl `NikonBase.Focus(x,y)` + `LiveViewViewModel.SetFocusPos`：
  点击坐标 × **头部 4/6（ImageWidth/ImageHeight，全图尺寸）** = 0x9205 参数；
  头部 20/22 的 FocusX/Y（AF 框中心）同空间（DrawFocusPoint 按
  bitmap/ImageWidth 缩放绘制）；选区本身不驱动对焦，需另发 AF。

### 实现

- `ptp_control.rs`：
  - `LiveViewHeader` 增加 `image_size`（4/6）；AF 框归一化改按全图空间
    （无 4/6 回退 0/2）。
  - `click_focus_offset(x, y, header)`：归一化点击 → 全图像素（4/6 优先、
    0/2 回退、双缺 → Err 不猜中心；输入 clamp，边缘不出界）。
  - `nikon_change_af_area(transport, x, y)`：0x9205 两 u32；0x2019 DeviceBusy
    短退避重试（≤5×100ms）。
  - `NikonLiveView` 每帧解析并缓存最近帧头部（`last_header()`）。
- `vendor_dispatch.rs`：`VendorProfile::focus_at` 增加 `live`（最近帧头）参数；
  `LiveViewSession::last_header`（Sony 无状态取景默认 None）。
  NikonProfile：声明 0x9205 且有帧头 → 选区（像素坐标）→ 0x90C1 AF 驱动；
  无 0x9205 或无帧头 → 退化为整帧 AF；无 AF 操作码 → 拒绝。
- WPD：原生连接缓存 `live_header`（frame() 随帧更新）；`native::focus_at`
  走原生连接（有帧头可换算）；无原生连接回退 `control::focus_at`（独立
  连接整帧 AF）。前端无需改动（点击已按实际渲染 img 矩形归一化，
  object-contain 无 letterbox 偏移）。

### 验证

- `cargo test --lib tethering`：113 通过（新增：坐标映射/0x9205 参数与忙重试/
  帧头缓存/厂商选区→AF 序列）。
- `cargo check --all-targets`：仅剩 NIKON_EVENTS 一个既有未接通常量
  （NIKON_FOCUS_AREA 已接线，警告消失）。

### 待真机核验（接 Z5II 后）

1. 点击点与机身 AF 框是否重合（头部 4/6 vs 0/2 空间选择按 Z5II 实测定案；
   若偏移呈比例，换 0/2 空间是一行改动）。
2. 竖拍（头部 29 旋转字节 1/2）时坐标是否需要旋转变换（digiCamControl 同样
   未处理，Z5II 实测定）。
3. 选区后 0x90C1 是否总是必要（部分机身选区即自动合焦，双发是否引发
   DeviceBusy 重试路径）。

## 自动续做进度补记 4：完整度盘点补缺（2026-10-09）

按完成清单逐项盘点后的软件缺口与补齐（全部未提交工作区代码）：

**1. 「N 待收」改后端 FIFO 真值**
- 旧实现是前端近似（快门 +1、photoAdded −1）：RAW+JPG 一次快门到两张
  事件，计数被双扣 → 第二张在途时提示提前消失。
- `CameraBackend::pending_receipts`（默认 0）+ WPD 进程级 FIFO 计数
  （`Receipts::pending_count`，纯锁读无 COM 往返）；`SessionDto.pendingReceipts`
  随快照下发；前端显示 = max(本地快门计数, 快照真值)，photoAdded 事件后
  400ms 去抖重拉快照对账。

**2. 取景画面叠加相机回报的 AF 框**
- `live_view_frame` 改返 `LiveViewFrame{jpeg, af_region}`（单次往返）：
  WPD 帧头 af_regions 随帧带出；gphoto/Sony 桥 `plain()` 无 AF 数据。
- 前端按 object-contain 内容矩形（容器百分比，帧到达/resize 重算）定位
  绿框——点击对焦后机身实际落点可见，坐标映射是否正确一眼可验。

**3. 主窗口联动**
- GalleryPage 监听 `tetheringPhotoAdded`（2s 去抖重拉首页）：联拍中主窗口
  开着图库时照片实时出现；AppShell 同事件去抖失效会话缓存（画廊快照/
  缩略图/打点）。此前只有相册页联动。

**4. 切库/删库结束联机会话**
- `session::stop_all()`（先置取消再断开——收片循环对象间检查取消位）；
  `database_switch`（激活库实际变化前）与 `database_remove`（删激活库时）
  调用。此前切库后收片会写进新数据库的陈旧相册 id。

验证：Rust lib tethering 114 通过（新增 stop_all/FIFO 计数断言）；
`cargo check --all-targets` 基线；前端全量 126 文件/1219 项通过
（新增待收真值/去抖对账/AF 框定位三组）。

仍未完成（真机/跨平台，见补记 2/3 清单）：Z5II 与 Sony 真机全链路、
macOS 编译与真机、SDK 条件打包与许可核对。软件可实现面至此按清单
全部落地；「Nikon 取景 + 点击对焦 + AF 框回显」待接机验收。

## Sony 参数确认报错修复与真机验证（2026-10-11）

- 日志中的 `Exposure Program / 0x500e is currently read-only` 与前端对
  `expprogram` 的只读例外放行对应。取消例外：仍显示当前模式，禁用修改并
  提示使用机身转盘；后端在每次写入前重新读取 widget 的 readonly 标志。
- 保留已验证的全树写入与真实回读确认。写入阶段单独收集 libgphoto2
  context 错误，防止驱动返回成功却拒绝目标属性，最后仅报泛化超时。
  回读不一致的日志与提示包含参数 ID、请求值、实际值。
- 用户关闭联拍窗口后独占测试 A7R V；分别用开发机库与应用捆绑库验证
  ISO 100→125→100→125，均正确回读，结束恢复 ISO 100。捆绑库各次首次
  回读耗时约 0.51–0.55 秒（不包含前置写入），未拍摄、未更换驱动。
- 新增默认忽略的硬件测试 `sony_iso_write_readback_hardware`，必须相机
  独占且用户允许修改参数后单独执行；测试会恢复原 ISO。只读检测会随
  机身状态变化，硬件测试只在机身实际报告只读模式时断言拒绝。
- 验证：联拍 Rust 单测 114 通过、5 忽略；联拍窗口前端 37 通过；
  TypeScript 检查通过。本次仅确认 Sony ISO 写入链路与只读入口修复，
  不代表其他品牌及全部参数已通过真机验收。

## 放大对焦回读与自动对焦释放（2026-10-11）

- Sony `focusmagnifier` 候选值是倍率（如 `9.3`），回读值为倍率加位置
  （如 `9.3,269,240`）。gphoto Choice DTO 对该参数提取倍率，用于显示、
  选项匹配与会话回读确认；不修改 native widget 中保留的位置。只对格式
  有效的该参数转换，其他参数、错误倍率和畸形回复仍不能通过确认。
- Sony `autofocus=1` 是按住半快门，原调用未发送 `0` 释放。设置面板动作
  与取景点击对焦统一采用按下→短暂等待→释放；按下失败仍尝试释放，释放
  错误不作为成功返回。这是一次短按，尚未增加机身合焦结果确认。
- 新增倍率与坐标格式回归、自动对焦正常/按下失败释放、释放失败传播测试。
  本轮尚未独占相机进行这两项真机操作，不能宣称已真机验收。
- 验证：联拍 Rust 单测 117 通过、5 忽略；变更文件 diff 检查通过。

## 工具条、取景自适应和对焦响应优化（2026-10-11）

- 底部浮条只保留快门速度/光圈/ISO。拍摄模式、白平衡、对焦模式等移到
  右侧，AF 与拍摄按钮也移到右侧。只读拍摄模式继续显示转盘提示并禁用。
- 取景图从仅设置 max-size 改为填满可用区域并等比适配，窗口 resize 与
  图片加载均重算实际内容矩形，点击黑边不会发出对焦，AF 框对齐实际图片。
  默认窗口由 1100×760 调整为 1100×680，最小高度由 600 调整为 520。
- gphoto 对焦复用一次读取的配置树完成位置设置、按下、释放，不再为各步
  重读全参数或等待 180ms；正常调参仍保留原有等待与回读确认。
- 使用应用捆绑库、Sony A7R V 真机验证中心对焦及释放成功，整次操作
  1.083 秒（包含 500ms 对焦等待），没有拍照。首次连接 PTP 超时，用户
  唤醒/重新连接后通过；不能据此宣称首次连接的所有超时问题已解决。
- 验证：前端 39 项、Rust 联拍 117 项通过，硬件中心对焦测试另行通过。
  保留其他品牌及跨平台真机验收待办。

### 工具条范围更正

用户澄清：拍摄模式与曝光三要素均保留在可拖动工具条，其他控制放右侧。
已将拍摄模式恢复到浮条首位，继续禁用只读模式并显示机身转盘提示，
右侧不重复显示模式。白平衡、对焦等仍在右侧。

工具条增加窗口/取景容器与自身尺寸变化的边界修正，拖动后的绝对坐标
始终限制在可见区域内；修正位置同步持久化，重开窗口同样修正旧位置。
窄窗口允许工具条换行，防止整条超出画面。前端联拍测试 40 项通过，
包含缩小后再放大工具条位置仍可见、模式与三要素均在浮条的回归检查。

### 快门、默认位置与模式切换同步更正

- 用户要求拍摄快门也位于浮条：模式、三要素、快门放浮条；AF 及其他参数
  留在右侧。新窗口一律从底部居中打开，旧版绝对位置缓存不再恢复或写入。
  手动拖动仅在本次窗口保留，窗口缩放仍限制位置在取景区域内。
- 模式写入确认后将关联参数快照重新标记失效，下次刷新重读可写性与候选。
  前端写入结束立即安排刷新；事件去抖 50ms，后端参数事件通知合并窗口
  从 1s 改为 100ms。未增加常驻全参数轮询频率，仍以事件为主。
- 前端 40 项测试、TypeScript 检查通过；新增后端模式确认后必须重读描述符
  的回归测试。机身转盘切档后的实际候选更新时间尚未做本轮真机测量。

### 联拍原始文件名与相机选择数字

- libgphoto2 给 Sony 根目录虚拟 RAM 对象加 `capt_` 前缀。下载时仍使用完整
  虚拟名，下载完成落本地收据时仅去掉这一层前缀，例如 `capt_DSC01234.JPG`
  → `DSC01234.JPG`。普通卡目录文件名及无原名的 `capt0001.jpg` 回退名不改，
  目标路径继续采用既有重名后缀策略，不覆盖照片。RAW/JPG 同编号保持配对。
- 移除联拍选相机对话框左上角的相机数量数字，扫描中的文字提示仍保留。
- 新增下载名恢复和 RAW/JPG 收据路径回归；相机选择前端 5 项及 TypeScript
  检查通过。本轮未拍摄新照片，未批量重命名已有照片。

## 相册可选与管理页联拍入口（2026-10-12）

- 普通照片可没有相册；删除 `ensure_default_album` 与名称特权、默认常量、
  导入自动预选、回收站还原自动入默认相册、编辑成片默认入册。已有未分组
  相册当普通相册处理，不自动删除其记录或照片。
- 删除相册仅删除引用，保留照片。相册照片菜单新增从此相册移除，与显式
  移入回收站分开。清除默认角标、旧提示及加入相册弹窗中已不可达的物理
  归入分支；子组建议 helper 改名为 subgroupSuggestions。
- 相册管理页新增联机拍摄按钮；TetherAlbumDialog 要求先选择已有相册或
  创建相册，再进入原相机选择流程，实际联拍仍强制绑定指定相册。新增照片
  事件更新管理页相册数量。
- 前端相册/导入/联拍相册选择测试通过，含无相册默认导入、创建新联拍相册
  和仅移除引用。后端新增删除/还原不创建默认相册和无相册导出登记测试。

### 删除相册的两种选择（2026-10-12）

用户纠正：删除相册提供仅删除相册/同时将照片移入回收站两种选择，默认
保留照片；删除源文件只在回收站操作，相册删除不提供文件删除或输入名字。
后端在同一事务中软删成员并删除相册引用，照片在其他相册中也会隐藏，
其他相册引用保留以便还原。相册计数和指定封面排除回收站照片。
新增前端两种选择参数传递测试、后端源文件保留/无关照片不受影响/还原后
仍属于其他相册的回归测试。
验证：相册页面前端 34 项、相册后端 14 项通过，TypeScript 与变更文件检查通过。

## 导入发现与按需读取（2026-10-12）

- 用户澄清：首次允许读取名称、大小、修改时间等低开销文件属性，禁止
  读取照片内容/EXIF/缩略图来完成点数。发现通道采用 list_names，WPD 只
  请求目录类型、名称、大小、修改时间，不为每个对象请求持久 ID；传输
  时仍可使用对象 ID，真正导入会重取完整清单与属性。
- 初始发现不打开数据库逐个判定新文件；new_files 尚未预判时不显示
  “0 个新文件”。监视文件夹自动导入的预判仍单独保留，避免自动导入停摆。
- DeviceEntry 保存发现清单，按连接代次隔离，向导读取缓存；刷新重新
  枚举，重连换源丢弃旧缓存。按批次事件逐步展示，空卡与失败状态仍保留。
- 缩略图维持现有可见区域与少量 overscan、并发限流、未开始请求取消、
  缓存复用；MTP 预览只读缩略资源，不下载整张原图。复制、校验与精确
  查重留给导入阶段，索引留给后续任务。
- 回归检查新增：发现时详细枚举/读头调用直接 panic，低开销属性正常
  保留；删除测试源后向导仍读取发现缓存，证明不二次枚举。
验证：导入前端 68 项、扫描 4 项、设备调和/缓存 8 项通过，TypeScript
及变更文件检查通过；未对真机 MTP 大目录进行本轮耗时测量。
