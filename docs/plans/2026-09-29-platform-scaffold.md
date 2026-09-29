# 跨平台代码组织准备

本轮整理平台边界并修正审计发现的契约与性能隐患。Windows 保留原生功能；macOS 留白，Linux、Android 同样预留入口，不代表已能在这些平台编译或运行。

## 目录和接口

```text
src-tauri/src/
  platform/
    mod.rs              # 系统操作接口和平台选择
    windows/
      filesystem.rs     # 根目录、属性、卷标、文件系统识别、顺序读取
      system.rs         # 默认打开、文件剪贴板、Explorer 定位
      runtime.rs        # 字体、子进程标志、Sony helper 路径、AI EP
      libraries.rs      # 动态库命名、加载、预加载、开发机搜索、CRT 环境
      present.rs        # 卷探测、已插卡设备和空读卡器枚举
      hotplug.rs        # WM_DEVICECHANGE 消息线程
      wpd.rs            # WPD DeviceSource、COM worker 和文件传输
      wpd_backend.rs    # WPD CameraBackend、探测缓存、拍摄和事件
    macos/              # 平台入口与设备后端占位
    linux/              # 同上
    android/            # 同上
    unsupported/        # 共用的不支持错误和 CPU 回退
  devices/
    mod.rs              # DeviceSource 契约、领域规则、WPD 实现选择
    present.rs          # 共用卷过滤策略和枚举接口
    hotplug.rs          # 纯转换函数和监视接口
    wpd_helpers.rs      # 纯数据解析和错误策略
    volume.rs           # 共用文件系统 DeviceSource
  tethering/
    backend.rs          # CameraBackend 契约
    mod.rs              # 相机实现选择
```

一般系统功能由业务代码调用 `crate::platform`；设备和相机继续复用既有 `DeviceSource`、`CameraBackend`，不再增加一个包揽所有能力的 trait。

WPD 文件通过 `#[cfg] + #[path]` 挂到原来的 `devices::wpd` / `tethering::wpd_backend` 命名空间，保持现有调用和 COM worker 的模块关系。macOS/Linux/Android 的同名入口现在只转发共用占位实现。WPD 名称是 Windows 通道的兼容入口；未来相机后端仍通过 `CameraBackend` 注册，不要求其他平台实现 Windows COM。

平台选择集中在 `platform/mod.rs` 以及设备、联拍的模块入口，业务流程不再直接调用 Win32 API。卷设备 ID 转为卷根的规则也交给平台接口，设备同步不再自行追加 Windows 分隔符。

## 当前占位行为

- 原生设备、空读卡器、文件系统根枚举及热插拔启动返回明确不支持错误；能力快照为 false，设备管理不创建枚举/轮询线程。
- 文件剪贴板、系统打开、文件定位和 WPD 操作返回明确不支持错误。
- WPD 相机占位不声明可用能力；注册表聚合时贡献空集合，直接连接和拍摄返回错误。
- 动态库加载未适配，开发机搜索和字体候选为空；保留业务层已有失败/字体回退逻辑。
- 非 Windows 的 AI EP 使用通用 CPU；不实现 CoreML、Metal 或其他 GPU 后端。`ort/directml` 只在 Windows 目标启用，Windows 的模型毒化与重试逻辑保留。

## 后续填充方式

macOS 的通用系统操作可在 `platform/macos/` 添加 `filesystem.rs`、`system.rs`、`runtime.rs`、`libraries.rs`，再在该目录的 `mod.rs` 显式导出已实现接口，其余接口继续从 `unsupported` 导出。不要同时 glob 导出本地实现和同名占位函数。

设备枚举、监视、传输分别填充该目录现有 `present.rs`、`hotplug.rs`、`wpd.rs` 入口；保持对外签名，或以兼容别名适配已有源类型。新增拍摄后端实现 `CameraBackend` 并注册。Linux、Android 沿用同一组织方式，已预置目标条件，不需要再移动业务文件。

实际移植仍需处理前端盘符/路径展示、资源和相机库打包、系统字体、权限、托盘/自启动，以及 macOS 构建和 CI。本轮不实现这些能力，也不增加 macOS CI。

## 抽象 API 修正

- `platform_capabilities` 返回序列化的能力快照，前端 `platformCapabilities()` 保留查询错误；false 表示尚未适配，与支持但无设备分开。`fs_list_dirs` 的根枚举传递平台错误，前端目录浏览保留原有静默降级，并提供 strict 参数。
- `filesystem_identity` 通过文件/目录句柄查询 64 位卷序列号，跟随 junction/symlink，未来目标查询最近存在的父目录；失败保持 Result，不再缓存挂载关系。该接口只作冷路径参考，实际移动直接尝试 rename，再回退复制、实际字节数/指纹校验与删源；硬链接同样直接尝试。
- AI 平台接口接收 `AccelerationPreference`，返回 `InferencePlan`（后端、图优化、批量偏好），EP 在创建会话时构造。公共推理故障按模型/后端隔离并回退 CPU；Windows 仍用 DirectML，占位平台只用 CPU。批量决策尊重 use_gpu，且每批重查，故障回退后不继续大批推理。
- 系统入口接收借用的 `ResourceRef`，区分本地路径和 URI；当前文档 URI 未实现，明确报错，不当作不存在的本地文件。普通文件去重保持大小写，不损失大小写敏感目录中的不同文件。Android 后续仍需实现 URI 授权、读取、流及导入源，当前只留类型边界。
- Windows 剪贴板复用 COM apartment guard，错误返回也成对清理。热插拔句柄拥有取消状态及线程，在窗口创建前退出也能传递取消；应用管理其生命周期并在退出时停止。

## 验证范围

本轮不在 Linux 构建或执行 Windows 项目测试。

- `git diff --check` 检查变更空白。
- `rustfmt --emit stdout --config skip_children=true` 对变更源码和占位文件做语法解析，不修改未涉及代码的格式。
- 源码审计检查所有 `#[path]` 目标存在、测试模块树导出平台入口、系统接口的 Windows/占位签名一致，以及 Win32 生产调用全部位于 `platform/windows`。
- 本次修正涉及移动、AI 策略与热插拔生命周期行为，需 Windows 回归测试；不能仅用迁移前后的 token 一致性作为验证。
- ripwire `--edit-check` 检查移动、设备枚举、AI 策略和系统入口的调用方；工具只识别部分 Rust 合约，`fs_list_dirs` 的 Result 返回变化需手动追踪，已更新异步 IPC 测试。
- ripwire `--quality-delta` 未全绿：仍有简单占位错误/测试桩、前端数组降级的相似代码，以及近期修改频率告警。应用入口新增的复杂度和失去调用的旧包装已修正；保留其余报告，不重置基线或屏蔽告警。
- ripwire `--test-gate` 给出 Windows CI 的待测范围，包括设备生命周期、导入、IPC、AI 和联拍；其非零结果表示测试及未覆盖范围仍待验证。本轮没有运行这些测试，编译和运行验证交由后续 Windows CI 与真机完成。

Windows 编译、集成测试、实际剪贴板/Explorer、相机和热插拔行为仍需后续 CI 与真机验证。

新增回归覆盖：卷句柄/未来目标路径、复制成功/字节数或指纹不符/rename 共享冲突、路径大小写和 URI、CPU 不重试/不批量、模型故障隔离、窗口创建前取消、能力 IPC 错误传播。旧的 verbatim 路径「模拟跨卷」测试改为真实 Windows 共享锁阻止 rename，同卷路径别名应直接移动/硬链接。

当前 Windows workflow 只构建安装包，不自动执行上述测试。Windows 环境至少执行 `cargo test --lib`，并运行 `ai_embed_test`、`album_dirs_test`、`engine_move_test`、`ipc_async_test`、`ipc_folder_test`、`device_present_test` 和 `device_lifecycle_test` 集成目标。前端新增用例在 `src/ipc/api.test.ts`；本地 node_modules 缺少 Vitest 可执行入口，未安装依赖、未运行。
