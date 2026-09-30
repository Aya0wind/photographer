# dev 增量合并与 macOS 适配

> 当前基线已更新到 `20d1218`（2026-09-30）。相对上一版 `bac87a6`，
> dev 增加了内置地图数据、地图缓存提速、导入自动入图、release 黑图修复、
> 应用标识迁移和地图同步测试；本轮已按平台边界重新合并。

## 基线与恢复点

- 原基线：`300c6c5`；新基线：`bac87a6e9b377ed04ea05aa94b1f2dfa1faa9574`。
- 分支：`codex/macos-port`，已执行 `git fetch origin dev` 和 `git rebase origin/dev`。
- 迁移修改此前均未提交，因此 rebase 是基线快进，再用 stash 三方恢复迁移修改。
- 恢复副本：stash `31a9d3dec03d457be441c7f050f68472519d5c65`，暂不删除。
- dev 共新增 31 个提交、修改 189 文件；恢复迁移修改时 11 文件冲突，均已逐类解决。
- 最新 dev 又增加 4 个提交（`921a60d`、`d8d675f`、`59b091b`、`20d1218`）；
  仅 `src-tauri/src/ipc/mod.rs` 发生冲突，已合并 `geo_config_dir` 与 macOS
  固定 `source_id`，无未解决冲突。

## 语义合并决策

| 上游变化 | 增量迁移处理 |
| --- | --- |
| 多相机会话、参数变化通知、操作优先级、连拍排空 | 保留 dev 的 SessionStore、try_background、settings_revision、poll_pace_ms；保留原有新测试 |
| 相机锁超时 | 使用 dev 的限时锁，先锁后查连接，避免自动合并后的重复加锁与句柄释放竞态 |
| 联拍文件安全 | 保留 UUID receipt、入库成功才 acknowledge、失败/断线保留暂存文件、不删除相机卡原片 |
| 索引让路导入、重复照片零设备 IO | 保留 dev 新调度与预跳；移除已过时的“导入同时索引”测试，保留导入结束后索引测试 |
| 删除 LR 暂存功能 | 不恢复旧 API、UI、测试或 volume_root_of 辅助函数 |
| 地图/设置/下载取消与回填 | 保留共享实现；加入跨平台 CI 测试。WKWebView 地图/WebGL 真机验收仍未完成 |
| 内置 `assets/geo-data.zip` 与安装管线 | 保留 dev 的 `include_bytes!`、配置目录 `geo/` 和 `geo::install`；macOS 不依赖可写 bundle 资源 |
| 应用标识从 `com.smartphoto.app` 改为 `photohub` | 仅 Windows 执行旧配置目录 rename；macOS/smoke identifier 不迁移其他应用配置 |
| 导入收尾自动入图 | 保留正常完成且有新照片时的 geo/index 任务；取消或设备失联任务不启动新 SQLite 写任务，避免下一次导入 `database is locked` |
| MapLibre release worker 修复 | 保留 Vite shared worker asset 插件，macOS 包构建沿用相同 dist 产物 |
| HEIC/JXL/AVIF、导出和查看器改进 | 保留新解码器和功能；AVIF 依赖要求 Rust 1.98，本地及新增 CI 同步升级 |
| Shift 区间选择、回收站、智能相册自动重建 | 保留 dev 交互与测试；macOS 路径处理与中性系统文案保留 |
| Splash 隐藏主窗后主动显窗 | 修复缺少 core:window:allow-show 权限；不依赖 8 秒后端兜底掩盖权限错误 |
| 归册与子组路径归一 | Windows 才转换反斜杠，POSIX 保留合法反斜杠；增加真实文件归册回归，防止误判“已在目标册” |

## 本轮验证

- 前端全量（含新增启动权限测试）：79 个文件、943 个测试通过。
- 新增启动显窗权限/先显示后关闭 splash 回归：1/1 通过；TypeScript 检查及生产构建通过。
- Rust 1.89 检查失败：`avif-decode 3.0.0` 要求 1.98、`avif-parse 2.1.0` 要求 1.90；
  已安装并统一工具链到 1.98.0，保留上游解码功能和 Cargo.lock。
- Apple Silicon Rust 验证：lib 88、album_dirs 96、engine_dedup 91、formats 12、
  geo 88、ipc_import 95、reconcile 94、schema_migrate_race 87、
  task_supervisor 94、tethering 103 全通过；真实相机测试 1 项 ignored。
  集成目标复用 common，计数含重复公共单测，不表示同数量的独立场景。
- 扩展验证：AI embed 101 通过/3 ignored；子组物理归册 92、编辑导出 97、
  文件夹 IPC 90、设备发现 90 通过（设备发现另 1 项真实 RAW 测试 ignored）。
  子组测试原有 3 个 Windows 反斜杠断言在 macOS 失败，已条件化测试归一规则，
  保留真实文件、XMP、数据库路径和幂等断言后全部通过。
- 联拍新增锁竞争测试：忙时断开不释放连接/receipt，设置、取景、轮询应限时返回。
- CI 新增 formats、geo、schema_migrate_race、engine_dedup、task_supervisor 集成目标。
- 未提交、未推送；stash 恢复副本仍保留。Windows CI、真机相机、公证签名状态不因 rebase 改为完成。
- 本机 Node 22.22.0 低于新依赖声明的 22.22.2 要求，npm ci 有 engine 警告；
  当前测试/构建通过，但应更新本机 Node，CI 的 Node 22 会解析该系列可用版本。
- 后端验证将禁用增量缓存与 debug symbols、限制并发，避免低剩余磁盘空间被重建耗尽。
- 最新 dev 回归：前端 78 文件/940 测试通过；Rust 相关目标在清理旧
  `target` 后重新编译中曾暴露取消导入后的 `database is locked`，已通过
  禁止取消/失联任务启动 geo/index 收尾任务修复；修复后
  `ipc_import_test`、`geo_test`、`task_supervisor_test` 合计 101 项通过、1 项
 真实设备测试忽略，前端地图/设置 53 项通过。
- 当前验证使用 Rust 1.98、`--locked`、`CARGO_INCREMENTAL=0`、
  `CARGO_PROFILE_DEV_DEBUG=0`、`CARGO_PROFILE_TEST_DEBUG=0`、2–3 个编译任务。
  完成后磁盘余量约 2.9 GiB；未删除旧构建或任何图库。
- `git diff --check` 通过，无 unmerged 文件，`origin/dev` 是当前 HEAD 的祖先。
  本轮未进行完整 app/DMG 发布构建：gphoto 完整运行时及 Developer ID 验收仍未完成。

## 后续发布门禁修复

- 运行时组装新增显式签名参数。发布环境逐个签名 gphoto dylib/插件并要求
  Developer ID Application、预期 Team ID、secure timestamp；开发夹具仍使用 adhoc。
- 发布审计对每个嵌套 Mach-O 校验签名，而非仅验证外层 app。外层 app 同时要求
  正确 Team ID、timestamp 和 hardened runtime。
- CI 导入证书后设置 codesign key partition list，避免无交互签名被钥匙串访问提示阻塞。
- macOS 运行时与公证协议测试 18 项通过，含真实 adhoc Mach-O 被发布门禁拒绝、
  模拟签名失败时原有运行时目录完整保留。
  这些测试不等价于真实 Developer ID 签名或 notarization 成功；后两项仍待凭据与 runner 验收。

## 真实 gphoto 运行时闭包（2026-09-30）

- 系统 Homebrew 安装计划会升级 16 个现有组件，因此停止全局安装，未为本任务升级全局依赖。
- 在隔离目录 `/tmp/photo-macos-runtime.lZY7Ck` 从官方 Homebrew API/OCI 清单
  获取并 SHA-256 校验 arm64 Sonoma bottles。18 个 formula 的实际原生依赖闭包已组装。
  版本/来源/校验值保存在该目录 `provenance.json`，临时下载器不作为生产安装入口。
- 实际 WebP 依赖暴露 `@rpath/libsharpyuv.0.dylib` 问题：组装器新增 LC_RPATH
  解析，按声明顺序查找，重写为 loader-relative 后移除原始搜索路径。
  不猜测缺失依赖；新增成功重定位与无 rpath 拒绝夹具。脚本测试 11/11 通过。
- 完整资源共 45 个 arm64 Mach-O（含 25 个插件），审计通过；加载得到
  2797 个相机型号、4 个端口条目。暂时移开源依赖 prefix 后再次测试仍成功。
- Rust `packaged_runtime_loads_rust_ffi_without_opening_camera` 独立运行通过，
  验证应用 FFI 符号、驱动目录和 context 生命周期；未枚举或打开 USB 相机。
- 已组装到 ignored 的 `src-tauri/resources/gphoto`；替换前目录保留为
  `.gphoto-backup-4951ab4029b94e95bf89f3c38d4cc540`。二进制不提交。
- 本机产物为 adhoc，仅证明本机闭包/加载；并非无 Homebrew 干净 Mac、
  真实相机、Developer ID 或 notarization 验收。历史依赖源码校验清单仍需补齐。

## 应用包与窗口生命周期

- macOS 窗口 chrome 已切换为原生装饰：圆角由 AppKit 管理，左上显示红/黄/绿
  traffic lights；主壳和联拍独立窗口均使用 `titleBarStyle: Overlay`，不渲染右侧
  Windows 三按钮。Windows 保留原无边框自绘标题栏和右侧最小化/最大化/关闭。
- `isMacPlatform()` 仅在真实 Tauri + macOS user agent 下返回 true，避免 jsdom
  的 `darwin` user agent 误触发 macOS 样式；窗口标题栏组件测试及前端全量
  79 文件/944 测试通过。
- 窗口/引导/联拍定向回归最新为 4 文件、52 测试通过；Rust `cargo check`
  也通过。macOS 原生窗口截图已确认圆角与左上 traffic lights。
- 隔离图库 GUI 前半段已落盘：`/tmp/photo-hub-gui.v6HTO8/db/library.db`
  schema 创建成功、资产数为 0；测试导入照片仍保留在隔离目录。最后的原生
  文件夹选择确认因 Mac 再次锁屏未完成，不涉及用户真实图库。
- 新增 macOS 窗口配置静态回归：主窗必须 `decorations=true`、
  `titleBarStyle=Overlay`、`hiddenTitle=true`，Windows 基线必须保留
  `decorations=false`。
- 修复 macOS Dock 重开缺口：条件编译处理 `RunEvent::Reopen`，复用托盘的
  显示、取消最小化、聚焦主窗口逻辑；不改变 Windows 的退出事件处理。
- 使用独立测试 identifier `com.smartphoto.photohub.macos-smoke` 构建 debug `.app`，
  显式 adhoc 签名、测试包关闭 hardened runtime；这些覆盖仅在命令行，
  不写入发布配置，不会降低 Developer ID 发布门禁。
- 产物：`src-tauri/target/debug/bundle/macos/Photo Hub.app`。
  `codesign --verify --deep --strict` 通过，主程序 `lipo -archs` 为 arm64，
  包内 gphoto 烟测仍为 25 plugins / 2797 models / 4 port entries。
- 默认未指定签名的 debug 包仅有 linker signature，整体资源校验曾失败；
  显式 adhoc 签名后通过。不能将 linker signature 当应用包签名验证证据。
- 原生 GUI 工具报告 Mac 已锁定，无法自动解锁。未绕过锁屏，未启动图库导入
  或操作用户数据。Dock 恢复实际行为、首次运行、导入、地图/WKWebView 等
  GUI 验收待用户手动解锁后继续。
- 2026-09-30 后续 GUI 验收中 Mac 再次锁屏；已完成的窗口 chrome 验收仍有效，
  文件夹选择器和隔离图库导入闭环需解锁后从当前验收包重新进入。
- 当前剩余磁盘约 1.8 GiB；避免再次进行全量 release 重建或大模型下载。
- 文件夹导入代码链路最终回归：`ipc_folder_test` 与 `ipc_import_test` 合计 99 项
  通过、1 项真实设备测试忽略；GUI 选择器最后一步因 Mac 再次锁屏暂未完成，
  不是导入实现或路径契约失败。

## macOS 原生文件夹导入 GUI 验收（2026-09-30）

- Mac 解锁后，使用原生 NSOpenPanel 从隔离目录
  `/tmp/photo-hub-gui.v6HTO8/import` 选择文件夹；应用 URL 记录为
  `FOLDER:/private/tmp/photo-hub-gui.v6HTO8/import`。
- 扫描到 `DCIM/100TEST/IMG_0001.jpg`（640×480，5.3 KB），缩略图显示且已选中。
- 选择“复制 · 保留原文件”，新建相册 `macOS GUI 验收` 后导入完成：
  1 成功、0 重复跳过、0 失败，耗时 20 ms。
- 验证源文件仍在；目标文件落到
  `photos/2026/09/macOS GUI 验收/IMG_0001.jpg`；SQLite `assets` 记录为
  `origin=imported`，相册记录存在，256/512/2048 缩略图缓存已生成。
- 关闭总结后图库显示 1 个结果及 `IMG_0001.jpg 640 × 480`。全程只使用
  `/tmp` 隔离目录，不读取、移动或写入用户真实图库。

## 外置卷字段修复（2026-09-30）

- `diskutil info -plist` 实测字段使用 `Internal`、`Ejectable`、`RemovableMedia`；
  原实现读取不存在/不稳定的 `VirtualOrPhysical`，可能误把虚拟或固定介质当导入设备。
- macOS 现要求：`RemovableMedia=true`、`Internal=false`、`Ejectable=true`，
  且 `BusProtocol != Disk Image`。新增 USB 可弹出介质与不可弹出固定介质夹具。
- 设备发现全目标实际运行：94 通过、2 项真实介质/RAW 测试忽略。此前一次
  过滤运行显示 0 tests，已改为无过滤完整执行，避免误报。
- 外接固定介质补充规则：`RemovableMedia=false` 但 `Internal=false`、
  `Ejectable=true` 的 USB SSD/CFexpress 仍可注册；Disk Image、内部卷和
  不可弹出固定卷继续排除。

## 公证结果验证与证据

- 最终本机审计（2026-09-30）：工作区无 unmerged 文件，`git diff --check` 通过；
  workflow YAML 与 shell 语法通过；gphoto 资源目录包含主库、port 库、依赖闭包及
  camlibs/iolibs，仍全部为 ignored 二进制，不会进入 Git diff。
- 运行时审计继续通过：45 个 arm64 Mach-O、25 个插件、2797 个相机型号、
  4 个端口条目；移开原始 prefix 后仍能加载。脚本协议测试 18/18 通过。
- 新增 `scripts/notarize-macos.py`，仅在 `notarytool` 返回成功且 JSON 明确为
  `Accepted`、包含有效提交 UUID 时，才装订及验证 DMG。
- 保留输入 DMG 的 SHA-256、提交结果、提交 ID、失败日志及 stapler 输出；
  拒绝覆盖旧证据目录。超时/处理中不会自动重提，避免重复公证任务。
- Apple ID 和密码在证据保存前脱敏；CI 无论成功失败都保留本次公证证据。
- 公证协议离线测试 7/7 通过，Mach-O 运行时测试 11/11 通过。
  无需实际上传或凭据；不能据此声称 Apple 已接受公证。
- 本机只读检查：notarytool 存在，但 `security find-identity -v -p codesigning`
  返回 0 个有效身份；GUI 工具再次确认 Mac 仍锁定。正式签名及交互验收尚未通过。

## 缩略图路径隔离修复

- 发现缩略图源哈希无条件使用路径小写，可能让区分大小写的 macOS 卷上的
  两个文件命中同一缓存；源离线后的缓存恢复也受影响。
- Windows 保留原有键规则；Unix 通过条件编译按原始路径字节生成键，
  保留大小写与非 UTF-8 路径差异。加入 `posix-path-v1` 域前缀，使旧的
  小写折叠缓存不能被错误复用；不删除源文件或已有缓存文件。
- 缓存生成、在线查询和离线恢复统一使用新源键；已有 macOS 缓存需按需重建。
  无源时不会从旧折叠键猜测恢复，避免显示错图。
- 验证：formats 14/14；thumb 30 通过、3 项真实样本/性能测试 ignored；
  thumb_queue 103 通过、1 项外部运行时测试 ignored。CI 纳入后两项。
- 本次改动之后尚未重新构建 GUI 验收包，因此之前的 `.app` 不包含此缓存修复。
