# 阶段 E「相机联机拍摄」立项前评估（2026-09-28）

> 纯调研文档，不含代码改动。依据：[docs/implementation-plan.md](../implementation-plan.md) §7（联机拍摄需求）与 §12（许可风险表）、[2026-09-27-followup-roadmap.md](2026-09-27-followup-roadmap.md) §2.E。外部信息检索时点 **2026-09-28**，所有结论注来源；查不到的一律标「未核实」。老资料（如 2016 年博客）标注年份。
>
> 评估基准：用户实际器材为 **Sony ILCE-7RM5（A7R V）** 与 **Nikon D750 世代** DSLR（依据库内 EXIF）；应用现状为 Windows-only Tauri 2 + Rust，已有成熟 WPD 文件导入栈（`DeviceSource` trait、`src-tauri/src/devices/wpd.rs` 直接 COM 调用 `IPortableDevice` 系列，无新增原生依赖）。

---

## 0. 结论先行

**推荐路线：WPD 优先的两步走。**

1. **第一步（立即立项，零驱动风险）**：在现有 WPD 栈上补两个能力——① `WPD_EVENT_OBJECT_ADDED` 事件订阅（拍摄后自动收片）；② **WPD MTP 扩展命令透传**（微软官方机制，可向相机发送任意厂商 PTP 操作码）。这一层可覆盖 Nikon（PTP 模式下一个 USB 模式同时提供文件访问 + 厂商码 0x90C0 触发拍摄）以及部分支持标准 `InitiateCapture` 的机型，形成「触发拍摄 + 事件收片」最小闭环，**不装任何驱动、不换任何驱动、不引入任何新原生依赖**。商业软件 Smart Shooter（Canon/Nikon 联机）在 Windows 上正是依赖 WPD 栈，先例成立。
2. **第二步（Sony 专线，申请制）**：A7R V 走 **Sony Camera Remote SDK** 插件——免费、需区域申请表单；A7R V 自 SDK 1.07.00（2022-10）即支持，能力完整（遥控拍摄 / LiveView 流 / 参数设置 / 文件传输）。**SDK 二进制不随 Apache-2.0 仓库分发**，采用「用户自行向 Sony 申请下载、本地放置」的可选插件模式（开源项目 TetherMoon 同款做法）。
3. **libgphoto2 降级为「不默认启用的备选通道」**：路线图 §7 要求的候选评估已完成，结论是它**不适合作为本应用的默认通用通道**——官方明确不支持 Windows；Windows 上必须用 Zadig 把相机的 WPD/MTP 驱动换成 WinUSB，**换完之后本应用现有的 WPD 导入栈、资源管理器、LR 的设备导入全部失效**（与既有 WPD 决策直接冲突）；Rust 绑定 gphoto2-rs 已约 1.5 年无提交（最后提交 2025-03）。仅在将来需要 Canon LiveView 等 WPD 透传拿不到的能力时，作为高级用户 opt-in 的实验后端。

**与 V3 既定决策的关系**：维持「相机直连走 WPD API」的方向，且本次调研发现 WPD 的能力上限比决策时认知的更高（MTP 透传可发厂商扩展码），进一步强化该决策；同时补齐了路线图要求的 libgphoto2 候选评估义务。

---

## 1. 逐项调研结果

### 1.1 libgphoto2 Windows 实况

| 问题 | 结论 | 来源 |
|---|---|---|
| 官方 Windows 支持 | **无**。README 原文："Support for operating systems from Microsoft is not currently available"，设计目标为 Unix-like 平台 | [github.com/gphoto/libgphoto2](https://github.com/gphoto/libgphoto2) README |
| Rust 绑定成熟度 | `gphoto2` crate（gphoto2-rs，作者 maxicarlos08）：安全封装 capture / preview / 配置 / 事件 / abilities / 内存下载；LGPL-2.1；crate 3.x。**最后提交 2025-03-04，repo pushed_at 2025-05-09**（GitHub API，2026-09-28 查询）→ 截至 2026-09 约 1.5 年无实质维护，43 stars，API 未到 1.0 稳定 | [github.com/maxicarlos08/gphoto2-rs](https://github.com/maxicarlos08/gphoto2-rs)、[crates.io/crates/gphoto2](https://crates.io/crates/gphoto2) |
| Windows 安装路径 | 绑定 README 原文："There is no official way to install libgphoto2 on windows, but you can install it with MSYS2"（mingw-w64-libgphoto2 包）。MSYS2 是 MinGW 工具链，产物 DLL 与本应用 MSVC 工具链混用需整组分发 MinGW 运行时；**vcpkg 无 libgphoto2 port**（2026-09-28 检索 vcpkg.io） | [gphoto2-rs README](https://github.com/maxicarlos08/gphoto2-rs)、[packages.msys2.org/package/mingw-w64-x86_64-libgphoto2](https://packages.msys2.org/package/mingw-w64-x86_64-libgphoto2) |
| USB 后端 / 驱动 | libgphoto2 USB 通信走 libusb；Windows 上每个非 HID USB 设备都要绑定 WinUSB/libusbK/libusb-win32 驱动后 libusb 才能访问，相机出厂绑定的是微软 MTP/WPD 类驱动，**必须用 Zadig 手动替换** | [libusb wiki（Windows 驱动）](https://github.com/libusb/libusb/wiki)、社区多案例（[Subsurface 邮件列表](https://groups.google.com/g/subsurface-divelog/c/wgyd9BkzXvs)、[Espressif 文档](https://documentation.espressif.com/)） |
| 换驱动的后果 | 相机停止出现在「便携设备/资源管理器」——即 **WPD 栈（含本应用现有导入、Explorer、LR 的 WPD 设备导入）对该相机全部失效**；恢复需设备管理器卸载 WinUSB 驱动并重插让 Windows 重装 MTP 驱动 | 用户案例与修复流程：[Tom's Hardware 论坛](https://forums.tomshardware.com)、[Hovatek MTP 驱动修复](https://www.hovatek.com)（年代 2023-2024） |
| 许可 | libgphoto2 与 gphoto2-rs 均 **LGPL-2.1**。动态链接（DLL 随应用分发 + 声明）可满足常规义务；静态链接须提供目标文件供最终用户重链接——对 Rust 单二进制分发不友好 | [libgphoto2 COPYING](https://github.com/gphoto/libgphoto2)、[GNU 许可 FAQ](https://www.gnu.org/licenses/gpl-faq.html) |
| 机型遥控支持面 | gphoto.org 遥控列表：**Canon EOS** 全线良好（含 LiveView）；**Nikon** D600/D610/D800/D810 等标注 capture+config+viewfinder 可用（**D750 未单独列出**——该列表 wiki 式维护、长期滞后，未核实）；**Sony** 列表只到 A7R II/A7S II 世代（2015 前后），但源码 NEWS 证实支持已扩展到新机身 | [gphoto.org/doc/remote/](https://www.gphoto.org/doc/remote/)、libgphoto2 NEWS（下条） |
| **A7R V 关键事实** | libgphoto2 **2.5.31** 新增 ID："Sony A7S III, ILCE-1, ILME-FX3, **7RM5 aka A7-RV**"；**2.5.32**："Sony: Now officially documented by Sony. Changes imported from the documentation"（Sony 2024 公开 PTP 扩展文档反哺）+ "support newer sony property format"；2.5.34（当前最新 release，含 2026 年 CVE 修复）："Sony generation 2 vs 3 handling enhanced"。即 A7R V 在 PC Remote 模式下 libgphoto2 **有一定支持**（拍摄/部分配置），但机型级参数受限的报告存在（A7M4 的 0x5013 Still Capture Mode 切换受限，gphoto2 GitHub issue 2026-03） | [libgphoto2 NEWS（master）](https://raw.githubusercontent.com/gphoto/libgphoto2/master/NEWS)、[gphoto2 issues](https://github.com/gphoto/gphoto2/issues) |

**小结**：libgphoto2 的协议知识库（ptp2 camlib 对 Sony gen2/gen3、Nikon 0x90C0 系、Canon EOS 扩展的实现与文档）是它对本项目**最有价值的部分**——可作为 WPD 透传通道的协议参考（操作码等事实性协议知识不受版权保护；**注意不要直接复制其 LGPL 代码**，否则把 LGPL 义务引进 Apache-2.0 仓库）。

### 1.2 WPD 原生遥控可行性

| 问题 | 结论 | 来源 |
|---|---|---|
| 微软是否提供「发任意 PTP 命令」的官方机制 | **有**。WPD MTP 扩展命令：`WPD_COMMAND_MTP_EXT_EXECUTE_COMMAND_WITHOUT_DATA_PHASE` / `..._WITH_DATA_TO_READ` / `..._WITH_DATA_TO_WRITE` / `..._END_DATA_TRANSFER`，经 `IPortableDevice::SendCommand` 发送；必填参数 `WPD_PROPERTY_MTP_EXT_OPERATION_CODE`（操作码，**厂商扩展码放这里**）与 `WPD_PROPERTY_MTP_EXT_OPERATION_PARAMS`；响应含 `WPD_PROPERTY_MTP_EXT_RESPONSE_CODE/PARAMS`。原文注明这些命令 "are therefore, only implemented by the WPD MTP class driver" | [Supporting MTP Extensions - Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/wpd_sdk/supporting-mtp-extensions)、[MTP Extension Commands](https://learn.microsoft.com/en-us/windows/win32/wpd_sdk/mtp-extension-commands) |
| 标准 0x100E InitiateCapture 哪些相机响应 | 不是所有 PTP 相机都实现——调用前应查 DeviceInfo 的 OperationsSupported 列表是否含 0x100E。实证：RICOH THETA S 用 `WPD_COMMAND_STILL_IMAGE_CAPTURE_INITIATE` 直接触发拍摄**失败**，改用 WPD MTP 透传发厂商码才成功；Panasonic GH4 在其 MTP DeviceInfo 中明列 0x100E | [Emsi 博客（2015-06）](https://emsi.wordpress.com/)、[THETA over USB（codetricity，2016-07/08）](http://codetricity.github.io/)、[THETA 博客（2016-04）](https://theta360blog.wordpress.com/) |
| Canon / Nikon 是否走标准命令 | **不走**。Nikon 遥控拍摄用厂商扩展码 `PTP_OC_NIKON_Capture 0x90C0`（1 参数、无数据相位），完整 Nikon 扩展操作码族在 libmtp `ptp.h` 中公开；Canon EOS 用自有扩展命令（EOS Utility / 第三方走 SDK 或 PTP 扩展）。这正是 Smart Shooter / qDslrDashboard 等软件的工作方式 | [libmtp ptp.h（Nikon 扩展码）](https://android.googlesource.com/platform/external/libmtp/+/master/src/ptp.h)、[gphoto2 issue #635（Nikon 对标准码报 Parameter Not Supported）](https://github.com/gphoto/gphoto2/issues/635) |
| 对象添加事件 | `WPD_EVENT_OBJECT_ADDED`：`IPortableDevice::Advise` 注册 `IPortableDeviceEventCallback::OnEvent`，回调参数中查 `WPD_EVENT_PARAMETER_EVENT_ID` 并取新对象 `WPD_OBJECT_ID`，随后用现有 `IPortableDeviceContent` 读属性/取流。微软官方示例代码在 Windows-classic-samples；注意微软文档说明 MTP 透传场景下设备事件本身不携带数据，需主动查询（推断：MTP 类驱动会把 PTP ObjectAdded 映射为该 WPD 事件——**机制推断，待真机验证**） | [microsoft/Windows-classic-samples（WPD API 示例）](https://github.com/microsoft/Windows-classic-samples)、[WPD 团队博客（2007，存档）](https://blogs.msdn.microsoft.com/wpdblog/) |
| 商业先例 | **Smart Shooter**（Canon/Nikon 商业联机软件）Windows 版明确依赖 Microsoft WPD 包（Windows N/KN 版需装 Media Feature Pack 才能联机）→ 证明 Nikon/Canon 遥控**可以**在纯 WPD 栈上实现，无需换驱动。对照：qDslrDashboard 走 WinUSB（Zadig）路线即互斥路线 | [Smart Shooter FAQ（Tether Tools）](https://tethertools.com/smart-shooter-faq/)、[qDslrDashboard](https://dslrdashboard.info/introduction/) |
| 占用互斥 | WPD MTP 驱动占用相机时其他软件不能再占用（同一设备单会话）；本应用现有 `DeviceError`/设备生命周期体系已覆盖独占提示，直接复用 | [Tether Tools Case Air FAQ](https://tethertools.com/blog/case-air-wireless-tethering-faq/) |

**小结**：**「触发拍摄 + OBJECT_ADDED 自动收片」可以完全复用现有 WPD 栈实现**，零驱动、零新原生依赖（`SendCommand`/`Advise` 都是现有 `IPortableDevice` COM 接口的方法，`windows` crate 已在用）。关键不确定点集中在「具体机型在 WPD 透传下对厂商码的实际响应」，必须真机验证（见 §5 清单）。

### 1.3 Sony A7R V 专线

Sony 现有两条并行产品线（Camera Remote Toolkit 之下）：

| 维度 | Camera Remote SDK | Camera Remote Command |
|---|---|---|
| 形态 | C++ SDK（闭源二进制） | **PTP 扩展协议规格书**（Sony 私有扩展 ISO PTP，含命令参考与示例代码） |
| 获取 | 免费 + 区域申请表单（美加/欧盟/中国大陆/日韩等分区链接） | 免费，但 FAQ 明确**仅限企业客户申请，个人开发者不可** |
| A7R V 支持 | **ILCE-7RM5 自 1.07.00（2022-10-27）支持**；当前版本 2.02.00（2026-06-10） | ILCE-7RM5 在支持列表（覆盖至 ILCE-7RM6） |
| 能力 | 改拍摄设置、快门释放、**LiveView 监视**、视频录制、文件传输、后台传输、remote emulation（OSD 远程仿真）、远程固件升级 | 300+ 命令（2.00.00 起）：设置变更、快门释放、live view 监视、对焦控制、PTP-IP 等 |
| 接口 | USB / 有线 LAN / Wi-Fi | USB / 有线 LAN / Wi-Fi（PTP-IP 自 2024.1.0） |
| 平台 | Windows 11（**仅 Intel/AMD，ARM 不支持**）、macOS、Linux | 协议规格，平台无关 |
| 商用 | 官方 FAQ：免费，可用 SDK 开发并**销售**应用 | 同左（限企业） |
| 再分发 | 页面未明确授权再分发 SDK 本体——**未核实**（EULA 随 SDK 提供，申请下载后核对）；开源项目通行做法是用户自装（TetherMoon 模式） | 规格书本身有 License Agreement 页，条款细节未核实 |

来源：[Camera Remote SDK 官方页](https://support.d-imaging.sony.co.jp/app/sdk/en/index.html)、[Camera Remote Command 官方页](https://support.d-imaging.sony.co.jp/app/cameraremotecommand/en/index.html)、[pro.sony 新闻稿](https://pro.sony/ue_US/press/sdk-update-camera-remote-command)、[cineD 报道（申请表单流程）](https://www.cined.com/sony-camera-remote-sdk-version-1-06-adds-compatibility-for-fx6-fx3-and-fx30)。

**与 libgphoto2 的联动事实**：Camera Remote Command（2024-04 公开，首批 29 机型）发布后，libgphoto2 2.5.32 的 NEWS 写明 "Sony: Now officially documented by Sony. Changes imported from the documentation"——即 Sony 官方 PTP 文档已反哺进 libgphoto2。对本项目的含义：**Sony 机身在「PC Remote」模式下的 PTP 扩展协议是有官方文档背书的**，理论上 WPD MTP 透传也可能驱动 Sony 拍摄（个人开发者拿不到规格书，但 libgphoto2 源码内已有同源实现可作参考）；**此路径未做任何验证，仅作为记录，不作为主路线**。

**USB 模式互斥（关键）**：Sony Alpha 机身的 USB 连接模式为四选一：Auto / Mass Storage / MTP / **PC Remote**（[ILCE-7RM3 帮助指南](https://helpguide.sony.net/ilc/1710/v1/en/contents/TP0001629775.html)、[ILCE-7M4 帮助指南](https://helpguide.sony.net/ilc/2110/v1/en/contents/TP1000616544.html)）。MSC/MTP 模式 = WPD 文件通道（现有导入栈所见形态）；PC Remote 模式 = 遥控通道（SDK / libgphoto2 / Imaging Edge 所需）。**两种模式互斥**，且 PC Remote 模式下另有关联设置「PC Remote Settings: Still Img. Save Dest.」（决定照片存相机/PC/两者）。PC Remote 模式下 WPD 还能看到什么（是否仍枚举为 MTP 设备、能否列文件）——**未核实，列真机测试项**。SDK 在 Windows 上的 USB 访问机制细节（是否自带驱动安装、与 WPD 的冲突面）——**未核实，申请到 SDK 后核对文档**。

### 1.4 Canon EDSDK / Nikon SDK 概览（风险定性）

| SDK | 获取与许可 | 平台 | 随开源应用分发 | 定性 |
|---|---|---|---|---|
| Canon EDSDK（+ 网络版 CCAPI） | 免费专有；开发者注册并接受 SDK 许可后下载 | Windows/macOS（C/C++）；CCAPI 为 HTTP/JSON | 运行库 DLL 原则上仅可随「使用它的应用」分发，SDK 本体/示例不可再分发；开源项目通行做法 = 用户自行注册下载（如 ofxCanonEOS 不含 SDK 二进制） | 中风险：可做用户自装插件；用户无 Canon 机身，优先级最低 |
| Nikon SDK（Camera Control 类） | **不公开下载**，需直接联系 Nikon 申请；专有 EULA；历史上限制严格（2005 年 NEF 加密争议时期第三方仅可得 TIFF/JPEG 输出） | Windows 为主 | 基本不可随开源应用分发 | 高风险：**放弃**；Nikon 机器改走 WPD 透传 0x90C0（协议知识公开于 libmtp/libgphoto2） |

来源：[Canon USA SDK 页](https://www.usa.canon.com/support/sdk)、[opensource.stackexchange: How to read the license for EDSDK](https://opensource.stackexchange.com/questions/4444/how-to-read-the-license-for-edsdk)、[ofxCanonEOS（不含 SDK 的开源先例）](https://github.com/wouterverweirder/ofxCanonEOS)、[DPReview 2005 Nikon/Adobe SDK 争议](https://www.dpreview.com/)（年代 2005，仅作历史背景）。参考：Nikon 官方免费联机软件 [NX Tether](https://imaging.nikon.com/imaging/lineup/software/nx_tether)（v2.5 起含 LiveView）的存在证明 Nikon 机身可被遥控，可作为能力对照基准。

---

## 2. 能力矩阵建议（CameraBackend 分层）

按路线图 §7 的 `CameraBackend` 抽象（发现设备、列存储、读文件、订阅新文件事件、触发拍摄、取预览、取消、断线恢复；每能力报告 supported/unsupported），建议四层：

| 层 | 通道 | 驱动变更 | 触发拍摄 | LiveView | 参数设置 | 文件/收片 | Sony A7R V | Nikon D750 世代 | 新增依赖 | 许可 |
|---|---|---|---|---|---|---|---|---|---|---|
| **L0** | WPD 文件通道（**已有**） | 无 | ✗（机身按键拍摄 + 事件收片 = 「半联机」） | ✗ | ✗ | ✓ | ✓（MSC/MTP 模式）；PC Remote 模式下**未核实** | ✓（PTP 模式） | 无 | 无新增 |
| **L1** | WPD MTP 透传（本次新识别） | 无 | 按机型：Nikon 0x90C0 **预期可行待真机**；标准 0x100E 视机型 DeviceInfo | 厂商码存在（Nikon LiveView 扩展码公开），**未验证** | 部分（PTP 标准属性 D0xxx 系） | ✓（复用 L0 流） | PC Remote 模式下透传 Sony 扩展码：**未核实**（非主路线） | **首个验证目标** | 无（复用 windows crate COM） | 无新增 |
| **L2** | libgphoto2（备选，opt-in） | **Zadig 换 WinUSB——与 L0/L1 互斥，换了 WPD 全死** | ✓（按机型） | ✓（Canon 系最佳） | ✓（最全） | ✓ | 部分（2.5.31+ 已加 ID；参数受限报告存在） | 遥控列表未列 D750；同代 D600/D800 可用，**未核实** | libgphoto2 DLL（MSYS2 构建）+ MinGW 运行时 | LGPL-2.1（动态链接 + 声明） |
| **L3** | Sony Camera Remote SDK 插件 | 无（SDK 自行管理 USB，机制细节未核实） | ✓ | ✓ | ✓ | ✓ | ✓（1.07.00+） | n/a | 用户自装的 SDK DLL + FFI 封装 crate | 专有；**不随仓库分发**，用户自行申请下载 |

**每层的 Windows 驱动冲突面**：

- **L0/L1（WPD 系）**：无驱动变更。冲突面只有一个——WPD 会话独占（本应用打开相机时，资源管理器/其他软件不能同时占用，反之亦然；现有 `DeviceError` 体系已能表达）。
- **L2（libgphoto2）**：冲突面最大且**不可逆于普通用户**——必须 Zadig 替换驱动后才能工作，替换后 WPD 通道（含本应用自身的文件导入）对该相机失效；恢复要用户手动卸载驱动。**这就是 L2 不能做默认通道的决定性原因**：它会摧毁 L0。
- **L3（Sony SDK）**：不换系统驱动，但相机必须切到 PC Remote 模式（与 MSC/MTP 互斥）→ 使用 L3 期间 WPD 文件导入对同一相机自然让位；SDK 会话独占。对应用架构的要求是「模式感知 + 后端切换提示」，而不是驱动级冲突。

---

## 3. 风险与许可红线（延续 §12 纪律）

1. **SDK 不干净就停**：Nikon SDK（申请制+专有+历史限制）**不立项**；Canon EDSDK 待用户有 Canon 需求时再按「用户自装插件」评估；Sony Camera Remote SDK 走「FFI 封装层开源、SDK 二进制用户自装」模式，仓库内**绝不提交** SDK 二进制（TetherMoon/ofxCanonEOS 先例）。申请到的 EULA 全文需在接入前逐条核对再分发与开源兼容条款（**未核实项**）。
2. **Camera Remote Command 规格书企业限定**：个人开发者身份拿不到规格书；不要以「逆向 Sony 协议」为主路线（个人开发者合规性不明）。libgphoto2 内的同源实现可作为协议参考，但**不复制其代码**（LGPL 传染），只参考操作码等事实性知识。
3. **libgphoto2 LGPL-2.1**：若将来启用 L2，必须动态链接 + THIRD_PARTY_NOTICES 记档 + 分发 MinGW 运行时义务评估；静态链接对单二进制 Tauri 分发不友好（GNU LGPL FAQ 的重链接要求）。
4. **WPD 透传的机型不确定性**：厂商码响应是「事实问题」不是「许可问题」——0x90C0 对 Nikon、扩展码对 Sony 均需逐机型实测后写进能力矩阵；UI 永远按后端报告的能力渲染（路线图 §7 原则），不做品牌名承诺。
5. **2027 时效警告**：Sony 官方页明确 "Camera Control PTP 2 commands may become unusable on some models from 2027"——Sony 通道的协议层存在已知时效风险，L3 依赖 SDK 版本跟进，需在文档中持续跟踪。
6. **占用与断线**：所有层的会话独占、拔线、模式切换（Sony 四模式菜单）都必须落到现有 `DeviceError::Disconnected/AccessDenied` 与设备生命周期体系，不新增异常路径。

---

## 4. 分阶段实施建议

### 4.1 可立即实施（不接相机，全部可离线落码 + 单测桩）

1. **`CameraBackend` trait 骨架 + `CapabilitySet` 能力位**（discover / list / read / delete / **notify_object_added** / **capture** / **liveview** / **config** / 取消 / 断线恢复语义），现有 `DeviceSource` 作为其文件子集实现，不推翻现有抽象。
2. **WPD 层扩展**（`devices/wpd.rs` 内新增，均可在 COM 桩上单测）：
   - `SendCommand` 封装的 MTP 透传函数（WITHOUT_DATA_PHASE / WITH_DATA_TO_READ 两类）；
   - `GetDeviceInfo` 解析 OperationsSupported → 生成能力探测报告（是否含 0x100E、Nikon/Sony 扩展码段）；
   - `Advise`/`OnEvent` 的 OBJECT_ADDED 订阅管线 + 去抖（连拍时事件风暴）。
3. **IPC 与 UI**：联机拍摄页骨架——设备连接状态、能力矩阵显示（unsupported 的操作置灰）、「拍摄」按钮、自动收片开关（复用现有导入管线与相册化流程）。
4. **后端注册表**：L0/L1 合并为 `WpdBackend`（能力位区分），为 L3 预留 `SonySdkBackend` 插件槽位（运行时探测 SDK DLL 存在性，不存在则整层隐藏）。
5. **THIRD_PARTY_NOTICES**：本轮无新增第三方依赖（L0/L1 零新增）。

### 4.2 需要用户接相机配合的验证（立项后第一迭代收尾）

见 §5 清单。顺序建议：先 Nikon（D750 世代，验证 L1 主路线是否成立），后 Sony（先验证模式行为，再决定是否申请 SDK）。

### 4.3 后续阶段（按验证结果触发）

- L1 验证通过 → 「触发拍摄+自动收片」进入正式功能，公布机型能力矩阵；
- Sony SDK 申请获批 → L3 插件实现（FFI crate + 用户自装引导 UI + LiveView 渲染进 Tauri WebView）；
- L2 仅在出现 WPD 透传无法覆盖的需求（典型：Canon LiveView）时，作为实验特性重新评估，并在设置中强制「了解驱动替换后果」确认流程。

---

## 5. 真机测试清单（需用户接相机）

### 5.1 Sony ILCE-7RM5（A7R V）

| # | 测试项 | 方法 | 通过标准 |
|---|---|---|---|
| S1 | MSC/MTP 模式回归 | 现有导入流程 | 现有能力不回退 |
| S2 | **PC Remote 模式下 WPD 可见性** | 切 PC Remote，看应用/资源管理器是否枚举、能否列 DCIM | 记录形态（决定 Sony 上 L0/L1 的边界；预期受限，未核实） |
| S3 | OBJECT_ADDED 事件 | MSC/MTP 模式机身拍摄，观察事件 | 拍后 ≤2s 收到事件并可自动导入 |
| S4 | （可选）PC Remote 模式下 WPD 透传 Sony 扩展码 | 发 Sony SDIO/扩展操作码 | 仅记录是否响应（非主路线，探索性） |
| S5 | （申请 SDK 后）L3 全链路 | SDK 采样代码：连接、改 ISO/光圈、拍摄、LiveView 帧、收片 | 全部通过后 L3 立项；同时核对 SDK Windows USB 机制与 WPD 的冲突面 |
| S6 | 模式切换行为 | 四种 USB 模式来回切 | 断线/重连被设备生命周期正确感知，无僵尸任务 |

### 5.2 Nikon D750 世代

| # | 测试项 | 方法 | 通过标准 |
|---|---|---|---|
| N1 | PTP/MTP 模式枚举与文件导入回归 | 现有流程 | 现有能力不回退 |
| N2 | **DeviceInfo 能力探测** | WPD 透传 GetDeviceInfo，记录 OperationsSupported 全表 | 拿到 0x100E / 0x90C0 是否在列的实证 |
| N3 | **0x90C0 触发拍摄 + 事件收片** | 透传发 0x90C0，等 OBJECT_ADDED，自动导入 | 「按软件快门 → 照片落库」闭环（L1 成立的决定性证据） |
| N4 | RAW+JPEG 双写收片 | 机内设 RAW+JPEG 后重复 N3 | 两种文件成对入库且配对正确（复用 pair_id） |
| N5 | LiveView 厂商码（可选） | 按 libgphoto2/libmtp 公开操作码尝试启动与取帧 | 记录可行性；不通过则 L1 的 LiveView 标 unsupported |
| N6 | 长时间占用与断线 | 保持会话 30 分钟、中途拔线重插 | 断线不崩、任务可恢复（现有体系回归） |
| N7 | 并发占用 | 应用占用时用资源管理器访问相机 | 双方得到明确的占用/失败提示 |

---

## 6. 来源汇总（检索时点 2026-09-28）

**libgphoto2 / Rust 绑定**：[libgphoto2 仓库](https://github.com/gphoto/libgphoto2)（Windows 不支持声明、LGPL）· [libgphoto2 NEWS](https://raw.githubusercontent.com/gphoto/libgphoto2/master/NEWS)（2.5.31 加 7RM5、2.5.32 Sony 官方文档反哺、2.5.34 gen2/3）· [gphoto.org 遥控列表](https://www.gphoto.org/doc/remote/) · [gphoto2-rs](https://github.com/maxicarlos08/gphoto2-rs)（LGPL-2.1、MSYS2 说明）· [crates.io/gphoto2](https://crates.io/crates/gphoto2) · GitHub API（最后提交 2025-03-04）· [MSYS2 libgphoto2 包](https://packages.msys2.org/package/mingw-w64-x86_64-libgphoto2) · [libusb wiki](https://github.com/libusb/libusb/wiki) · [GNU 许可 FAQ](https://www.gnu.org/licenses/gpl-faq.html)

**WPD / MTP**：[Supporting MTP Extensions（微软）](https://learn.microsoft.com/en-us/windows/win32/wpd_sdk/supporting-mtp-extensions) · [MTP Extension Commands（微软）](https://learn.microsoft.com/en-us/windows/win32/wpd_sdk/mtp-extension-commands) · [Windows-classic-samples](https://github.com/microsoft/Windows-classic-samples)（事件示例）· [Smart Shooter FAQ](https://tethertools.com/smart-shooter-faq/)（WPD 依赖实证）· [libmtp ptp.h](https://android.googlesource.com/platform/external/libmtp/+/master/src/ptp.h)（Nikon 0x90C0）· [gphoto2 issue #635](https://github.com/gphoto/gphoto2/issues/635) · [Emsi 2015](https://emsi.wordpress.com/) / [codetricity 2016](http://codetricity.github.io/) / [THETA 博客 2016](https://theta360blog.wordpress.com/)（0x100E 机型差异与 WPD 透传实例）

**Sony**：[Camera Remote SDK](https://support.d-imaging.sony.co.jp/app/sdk/en/index.html) · [Camera Remote Command](https://support.d-imaging.sony.co.jp/app/cameraremotecommand/en/index.html)（企业限定、2027 警告）· [pro.sony 新闻稿](https://pro.sony/ue_US/press/sdk-update-camera-remote-command) · [cineD](https://www.cined.com/sony-camera-remote-sdk-version-1-06-adds-compatibility-for-fx6-fx3-and-fx30) · [ILCE-7RM3 USB Connection 帮助指南](https://helpguide.sony.net/ilc/1710/v1/en/contents/TP0001629775.html) · [ILCE-7M4 帮助指南](https://helpguide.sony.net/ilc/2110/v1/en/contents/TP1000616544.html)

**Canon / Nikon**：[Canon USA SDK](https://www.usa.canon.com/support/sdk) · [EDSDK 许可讨论](https://opensource.stackexchange.com/questions/4444/how-to-read-the-license-for-edsdk) · [ofxCanonEOS](https://github.com/wouterverweirder/ofxCanonEOS) · [DPReview 2005 Nikon SDK 争议](https://www.dpreview.com/) · [NX Tether](https://imaging.nikon.com/imaging/lineup/software/nx_tether)

**明确未核实项汇总**：① D750 在 libgphoto2/gphoto 遥控支持（列表未列）；② Nikon/Sony 厂商码经 WPD 透传的实际响应（本评估的核心待验证假设）；③ PC Remote 模式下 WPD 的可见形态；④ Sony SDK EULA 再分发条款全文与 Windows USB 访问机制；⑤ WPD 将 PTP ObjectAdded 映射为 OBJECT_ADDED 的机制细节（推断）；⑥ A7R V 在 libgphoto2 下的拍摄/预览实际可用性（有 ID 与属性支持记录，无整机验证报告）。
