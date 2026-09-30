# macOS（Apple Silicon）迁移实施计划

> 2026-09-30 增量状态：已 rebase 到 `bac87a6`；最新差异与验证记录见
> `2026-09-30-dev-incremental-port.md`。下面旧基线测试计数为历史证据，
> 不能替代新基线验证。dev 已删除 LR 暂存，且索引改为导入结束后启动；
> 不再要求恢复旧 LR 功能或“边导入边索引”的旧测试契约。
> 运行时更新：完整 gphoto arm64 闭包已在隔离目录组装并加载通过，项目资源已补齐。
> 下文“本机缺运行时”为历史阻塞，当前剩余的是正式签名、真机及干净环境验收。

## 目标与约束

- 目标平台：macOS 14.4+，首版仅 `aarch64-apple-darwin`。
- 不迁移已有 Windows 图库；macOS 首次运行新建图库。
- 不实现 Sony SDK 与 WPD/MTP；相机能力统一走 libgphoto2。
- 首版支持文件夹选择导入；外置卷自动发现与热插拔后置。
- macOS 与 Windows 最终保持同一套业务功能，平台差异集中在条件编译适配层。
- 发布要求：Developer ID 签名、notarization、DMG。

## 当前已完成

- 初始从 `origin/dev` 同步到 `300c6c5`；2026-09-30 已 rebase 到最新 `bac87a6`。
- 已创建并使用分支 `codex/macos-port`。
- `src-tauri/src/platform` 平台边界已建立，Windows 原生实现已隔离。
- macOS/Linux/Android/unsupported 已有安全占位入口。
- `PlatformCapabilities`、`ResourceRef`、`InferencePlan`、`FilesystemIdentity` 已进入共享平台契约。
- 文件移动已改为直接尝试 `rename`，失败后校验复制并删除源文件。
- 文件夹导入已具备 `folder_scan → device_files → import_start` 完整业务链路。
- gphoto2 已有跨平台抽象的动态加载后端；Windows DLL 与 macOS dylib 路径已分别接入。
- 已修复 `claim.rs` 中缺失的 `volume_root_of` 编译回归。
- 已修复 unsupported WPD 后端在集成测试模块树中的绝对路径引用。
- macOS `cargo check`、关键 Rust 集成测试和 LR 暂存测试已通过。
- 初始迁移阶段前端 73 个测试文件、886 个测试已通过；rebase 后最新验证为
  79 个文件、943 个测试，详见增量报告。
- Apple Silicon `aarch64-apple-darwin` 的 `.app` 与 `.dmg` 已在本机成功构建。
- macOS 原生窗口 chrome 已实测：AppKit 圆角窗口、左上红黄绿 traffic lights，
  无右上 Windows 三按钮；Windows 仍保留原来自绘标题栏。
- 本机产物为 adhoc 签名，不能替代 Developer ID 签名和 notarization 验证。
- macOS 构建现在会在 `beforeBuildCommand` 先审计 gphoto Mach-O 闭包；缺少完整
  运行时会 fail-fast，不再生成缺主库的误导性 `.app`/DMG。Apple Silicon
  闭包已在隔离依赖目录组装并通过原生加载烟测；正式包仍必须在签名 runner
  执行 arm64 gphoto bundle 组装脚本。
- gphoto 后端已按 `camera_id` 路由，macOS 不再让 gphoto 相机误走 WPD 主后端。
- macOS CoreML provider 已启用；Auto 推理计划选择 CoreML，失败仍按模型回退 CPU。
- macOS Tauri 配置、Rust 工具链固定、gphoto bundle 脚本和签名/notarization workflow 已建立。

## 已知问题

### 验证状态校正（本轮审计）

下面各阶段的勾选表示已实现或指定测试已通过，不代表端到端迁移已验收。
尚无 GUI 完整工作流、CoreML 真实模型/CPU 一致性、Windows 构建回归、
相机拍摄和正式公证通过的证据；不能称“只剩外部凭据”。
导入跟随索引测试在并发负载下出现过缩略图超时，单次重跑通过不代表稳定性已修复。
窗口样式已验收；系统打开、Finder/剪贴板、图库导入、地图/WKWebView 等完整 GUI
流程仍需继续验收。当前 14.4 最低部署版本是本机构建基线，并非对“当前 macOS
最新发行版本”的核实结论。

新增打包验证：`python3 -m unittest discover -s scripts -p 'test_macos_*.py' -v`
在 Apple Silicon 上 18 项通过，使用真正编译的 Mach-O 夹具验证传递依赖、
重定位后 dlopen、失败保留旧目录，以及无执行权限 .so 的占位引用拦截。
这不代表 libgphoto2 的真实完整依赖已验证。组装脚本不再删除原资源目录，
不再吞掉 install_name_tool/codesign 错误；失败保留临时目录供诊断，
成功替换后保留旧目录备份。

### 本轮可复现验证

- AI 新增初始化回退：CoreML 注册或模型编译失败时，以新 CPU builder 重试一次，
  保留两次错误，按模型/后端禁用失败加速路径；CPU 自身失败不重试。
  6 项定向测试通过（含真实 ORT builder 加载损坏 ONNX），`ai_embed_test`
  75 项通过、3 项真实模型/规模测试忽略；不据此宣称有效模型 CoreML 推理已验收。
  Windows 上显式请求 CoreML 时回到 CPU，不再误选 DirectML。
- 前端路径帮助函数按路径类型比较，POSIX 保留大小写/合法反斜杠，
  `/` 和 `C:\` 根不退化为空或驱动器相对路径；每个导入目的地独立选择
  预览分隔符。路径/引导/导入向导共 74 项通过；本轮 `npm run build` 通过
  （原有大 chunk 与无效动态 import 警告仍在）。Rust lib 65 项通过；迁移关键集成目标本轮合计 429 项通过，1 项真实 RAW 测试忽略。
- `ipc_import_test` 的跟随索引测试改用 channel 门控第二个文件，恢复严格要求：
  第一张缩略图在其余文件被放行前完成，放行后四张缩略图全部完成。
  定向连续 3 次通过；本轮完整 `ipc_import_test` 71 项通过。未放宽生产解码超时；
  仍需负载测试判断历史解码超时的原因，不能据此宣称性能问题全部解决。
- 修复前端 `CullingOverlay` 连续 `ArrowLeft/ArrowRight` 使用旧闭包索引导致的丢步：
  单图导航改为函数式索引更新；定向测试 22/22、前端全量 73 文件/886 测试通过。
- 断盘处理使用活跃任务固定的源 ID，不再查询可能被切换的当前图库。
  收尾中的失联任务拒绝伪恢复；选中文件夹位于断开的卷内时同样暂停，
  路径边界匹配避免误伤同名前缀目录。完整 `reconcile_test` 71 项通过。
- macOS 系统操作改为固定 `/usr/bin` 工具路径；默认打开遵循不支持 URI
  的平台契约；剪贴板 AppleScript 使用固定源码，文件路径仅作为 argv 数据。
  两项测试通过：URI 拒绝及含中文/引号/反斜杠/换行的真实文件路径往返。
  测试未更改剪贴板，Finder/实际粘贴体验仍待 GUI 验收。
- gphoto 联拍不再在下载后立即删除相机卡原件；本地 receipt 仅在库事务
  成功后清理，入库失败、断线和窗口关闭都保留未确认文件。相同文件名使用
  独立 receipt 目录，防止覆盖；2 项 staging 测试和 1 项 session 入库失败
  测试通过。

### 阻塞问题

- 尚未在真实相机和真实 gphoto arm64 bundle 上完成枚举、取景、拍摄、收片真机验收。
- 尚未在安装了完整 Xcode 的 macOS runner 上完成签名、DMG、notarization 和 stapling。
- 完整 gphoto arm64 闭包已在隔离目录完成加载验收；仍需在正式签名 runner
  复验许可证、源码来源、Developer ID 嵌套签名及 notarization。

### 非阻塞问题

- macOS 集成测试的 Windows 路径样例和 Windows 专属断言已按平台条件化。
- unsupported/WPD 兼容模块在 macOS 编译时仍有 unused/dead-code 警告。
- 前端部分文案仍直接写“资源管理器”、盘符和 Windows 路径格式。
- workflow 已加入签名凭据缺失时的显式失败检查，并覆盖 `dev/main` 推送验证。
- macOS release/test workflow 使用 Apple Silicon `macos-15` runner，并在 job 中断言 `arm64`；
  Windows/macOS 共享回归 workflow 已加入前端与 Rust 关键集成测试。
- gphoto 组装脚本会保留可发现的 libgphoto2 许可证文件；完整依赖许可证清单仍需在发布机审计。

## 实施阶段

### M0：基线与回归

- [x] 修复最新 `dev` 编译回归。
- [x] macOS `cargo check`。
- [x] LR 暂存定向测试。
- [x] 前端依赖安装后运行 `npm test` 与 `npm run build`。
- [x] Rust 关键集成测试完整运行并修复非平台回归。

### M1：macOS 平台基础能力

- [x] 文件系统根目录（主目录、`/Volumes`）。
- [x] 隐藏文件和系统目录判断。
- [x] 卷标与文件系统身份。
- [x] `open` 默认程序打开。
- [x] Finder `open -R` 定位。
- [x] macOS 文件剪贴板（osascript）。
- [x] macOS 系统字体候选。
- [x] macOS 子进程和动态库加载基础设施。

### M2：文件夹导入首版

- [x] 验证目录树和文件夹选择导入链路。
- [x] 验证本地图库创建、文件夹扫描、copy/move、去重、缩略图、EXIF/XMP 的现有测试链路。
- [x] 增加 macOS 路径和平台能力契约测试。
- [x] 前端通过 `PlatformCapabilities` 保留文件夹导入并区分未适配设备。

### M3：AI

- [x] 为 macOS 启用 CoreML provider。
- [x] 增加 `InferenceBackend::CoreMl` 和条件编译 EP。
- [x] 失败后按模型隔离回退 CPU。
- [x] 更新设置文案为平台无关的硬件加速。

### M4：gphoto 联机拍摄

- [x] macOS arm64 libgphoto2/libgphoto2-port/camlibs/iolibs 构建入口。
- [x] macOS `.dylib` 搜索与加载基础设施。
- [x] 资源打包到 `Contents/Resources/gphoto` 的目录约定。
- [x] 后端按 camera id 路由。
- [x] 解析传递 dylib 依赖并重写 `@rpath`/`@loader_path`（真实 45 个 Mach-O 与原生加载通过）。
- [ ] 枚举、参数、取景、对焦、拍摄、自动收片和入册真机验收。

### M5：外置卷与热插拔

- [x] `/Volumes` 外置卷发现。
- [x] 初版周期扫描。
- [x] 断开/重连和导入任务暂停恢复的模拟事件集成测试。
- [ ] 真实外置卷拔出/重新挂载测试（含文件夹导入、换卡、挂载点复用）。
- [ ] 后续再评估 Disk Arbitration 原生事件。

### M6：发布

- [x] Apple Silicon `.app` / `.dmg` Tauri 配置入口。
- [x] Developer ID/notarization 环境变量和凭据检查骨架。
- [ ] notarization + stapling 真机/runner 验证。
- [x] macOS GitHub Actions workflow。
- [ ] 发布前资源、许可证、架构和签名检查。

### M7：完整验收（不可用编译通过替代）

- [ ] 新建空图库的 GUI 导入、图库/相册、搜索、选片、回收站、元数据、编辑导出闭环。
- [ ] WKWebView 图片协议、RAW/JPEG 显示、缩放、Retina/中文字体和快捷键。
- [ ] Finder 多文件定位、文件复制后粘贴、关闭/重新打开窗口、托盘和登录自启。
- [ ] 真实 ONNX 模型的 CPU/CoreML 输出一致性、模型级初始化/运行失败回退和性能。
- [ ] Windows 构建与跨平台共享逻辑回归（不能只在 macOS 上测试 Windows 字符串样例）。
- [ ] 干净 Mac 无 Homebrew 的 gphoto 全部插件加载、真机联拍/断线/收片失败原片保留。
- [ ] Developer ID、notarytool Accepted、staple validate、Gatekeeper 验收和版本化发布证据。
- [ ] 发布依赖的许可证、版本、来源/校验值与必要的源码分发材料审查。

## 验收原则

- 每完成一个阶段，必须同时通过 `cargo check`、相关 Rust 测试、前端类型/单测（若涉及前端）。
- 平台不支持的能力必须通过 `PlatformCapabilities` 明确呈现，不能伪装成“没有设备”。
- 所有平台差异通过 `#[cfg]` 和 `platform/<os>` 实现，业务层不直接调用 macOS/Windows API。
- 发现新的编译、测试、路径、权限或发布问题时，先修复再推进下一阶段。
