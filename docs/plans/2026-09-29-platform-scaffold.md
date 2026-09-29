# 跨平台代码组织准备

本轮只整理平台边界。Windows 保留现有实现；macOS 留白，Linux、Android 同样预留入口，不代表已能在这些平台编译或运行。

## 目录和接口

```text
src-tauri/src/
  platform/
    mod.rs              # 系统操作接口和平台选择
    windows/
      filesystem.rs     # 根目录、属性、卷标、卷序列号、顺序读取
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
    unsupported/        # 共用的空枚举、不支持错误和安全回退
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

- 原生设备、空读卡器、文件系统根枚举返回空集合；热插拔监视不产生事件。
- 文件剪贴板、系统打开、文件定位和 WPD 操作返回明确不支持错误。
- WPD 相机占位不声明可用能力，连接和拍摄返回错误。
- 动态库加载未适配，开发机搜索和字体候选为空；保留业务层已有失败/字体回退逻辑。
- 非 Windows 的 AI EP 使用通用 CPU；不实现 CoreML、Metal 或其他 GPU 后端。`ort/directml` 只在 Windows 目标启用，Windows 的模型毒化与重试逻辑保留。

## 后续填充方式

macOS 的通用系统操作可在 `platform/macos/` 添加 `filesystem.rs`、`system.rs`、`runtime.rs`、`libraries.rs`，再在该目录的 `mod.rs` 显式导出已实现接口，其余接口继续从 `unsupported` 导出。不要同时 glob 导出本地实现和同名占位函数。

设备枚举、监视、传输分别填充该目录现有 `present.rs`、`hotplug.rs`、`wpd.rs` 入口；保持对外签名，或以兼容别名适配已有源类型。新增拍摄后端实现 `CameraBackend` 并注册。Linux、Android 沿用同一组织方式，已预置目标条件，不需要再移动业务文件。

实际移植仍需处理前端盘符/路径展示、路径大小写与去重规则、资源和相机库打包、系统字体、权限、托盘/自启动，以及 macOS 构建和 CI。本轮不实现这些能力，也不增加 macOS CI。

## 验证范围

本轮不在 Linux 构建或执行 Windows 项目测试。

- `git diff --check` 检查变更空白。
- `rustfmt --emit stdout --config skip_children=true` 对变更源码和占位文件做语法解析，不修改未涉及代码的格式。
- 源码审计检查所有 `#[path]` 目标存在、测试模块树导出平台入口、22 个系统接口的 Windows/占位签名一致，以及 Win32 生产调用全部位于 `platform/windows`。
- 与 HEAD 做忽略注释和格式的 token 对比：WPD worker、WPD COM、WPD 联拍 COM、热插拔消息循环保持一致。
- ripwire `--edit-check` 检查现有 IPC、AI、卷标、资源命名和设备同步契约。卷标接口的定义从两个条件实现合并为一个委派，因此工具报告定义数变化；迁移后的 COM 调度报告新路径。
- ripwire `--quality-delta` 并未得到全绿：仍报告搬移后重新识别的缓存/能力位/名称读取重复、占位代码与测试桩的相似性，以及近期修改频率告警。保留报告，不重置基线或屏蔽告警；已去掉本轮不相关的格式改动和占位错误分支重复。
- ripwire `--test-gate` 给出 Windows CI 的待测范围，包括设备生命周期、导入、IPC、AI 和联拍；其非零结果表示测试及未覆盖范围仍待验证。本轮没有运行这些测试，编译和运行验证交由后续 Windows CI 与真机完成。

Windows 编译、集成测试、实际剪贴板/Explorer、相机和热插拔行为仍需后续 CI 与真机验证。
