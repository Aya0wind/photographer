# 固定 RawLab core 的浮点接入原型（M0）

直接复用 RawLab core，**不自行实现 RAW 或胶片算法**。固定版本、Git submodule和更新方式见 [UPSTREAM.md](UPSTREAM.md)。Rust 薄封装在 `crates/media/src/rawlab.rs`，PhotoCraft 交接为 `photographer_lib::linear_rgb_document`。主编辑器现已提供显式选择的胶片显影链，历史配方和默认后端保持原语义。

## 构建与使用

需要 C++17、CMake 3.24+、zlib 开发包。Windows CI 使用 `zlib:x64-windows-static-md`，传入 vcpkg toolchain/triplet。先执行 `git submodule update --init --recursive`，再从仓库根目录执行：

```sh
cmake -S native/rawlab -B native/rawlab/build -DCMAKE_BUILD_TYPE=Release
cmake --build native/rawlab/build --config Release --parallel 2
ctest --test-dir native/rawlab/build -C Release --output-on-failure
```

Linux 产物为 build/libphotographer_raw.so，macOS 为 build/libphotographer_raw.dylib，Windows 多配置生成器为 build/Release/photographer_raw.dll。JPEG XL DNG 默认按最新 core 构建 Adobe DNG SDK/libjxl；可用 `-DPHOTOGRAPHER_RAWLAB_JXL=OFF` 明确裁剪此能力。libtiff 使用上游固定源码构建，Lensfun/OpenCV/wavelib 暂不启用。构建 GPU 默认开启，`-DPHOTOGRAPHER_RAWLAB_GPU=OFF` 可验 CPU-only 回退。LibRaw 0.22.2 及官方 CMake 适配的下载地址/SHA256 由 upstream.lock.json 固定；桌面 build.rs 不下载或编译这些源码。

```sh
cargo run --manifest-path src-tauri/Cargo.toml --example rawlab_probe -- \
  /absolute/path/libphotographer_raw.so /photos/input.DNG /tmp/new-neutral.png
# 追加 RawLab 能识别色彩合同的胶片 CUBE：
cargo run --manifest-path src-tauri/Cargo.toml --example rawlab_probe -- \
  /absolute/path/libphotographer_raw.so /photos/input.DNG /tmp/new-film.png /luts/film.cube
```

必须指定从本目录构建的可信动态库；不允许在应用中开放上传/执行第三方动态库。输出 create_new，存在则报错。原片只读。诊断打印实际 GPU 成功标记和处理耗时，不表示交互响应/实际呈现已测量。

## 直接复用的调用链

- `RAWProcessor::loadFile/process`：LibRaw 相机处理、去马赛克前 WB、黑电平、高光、相机矩阵、活动裁切/方向和线性工作输出。桥只映射参数/生命周期，不再复制这些算法。
- `LUTParser`、`encodePhotoLUTInput`、`applyLUTWithConfig`、`renderPhotoLUTOutput`、`neutralDisplay`：core 识别 Gamma/Gamut 元数据，按原生合同做色域/Log/三线性采样和显示转换；桥组合中性/胶片并按强度混合，与上游 C ABI 的 CPU 顺序一致。
- `renderPhotoMetal`、`D3D11PhotoRenderer`、`GlesPhotoRenderer`：float 输入直接交给 core 的平台 photo GPU。Auto 失败从原始线性数据重算 CPU，Force 失败明确报错。Linux 当前无平台 GPU，会走 CPU。

未引入客户端、联拍、RAW 样片、第三方 LUT、Python 应用运行时。GPU/空间算法源码随 core 固定，但本批关闭 OpenCV/wavelib 降噪能力，只复用显影和基础胶片路径。现有 PhotoCraft 后续调色/蒙版/输出能力继续保留。

## 浮点合同和当前边界

上游现有公共 C ABI 只暴露 RGB8/RGBA8，不适合保存浮点线性代理。因此另加窄 C ABI，直接调用已有 C++ 实现，返回原生拥有的 float RGB。桥 ABI 为 4，解码 options 版本为 3，色彩 options 为 2；旧桥在加载时明确拒绝，避免结构体布局错读。静态检查 RGB 布局；Rust 后端生命周期覆盖图像，Drop 用同库释放。线性 sRGB/D65、display-sRGB 使用不同 encoding 标记；方向已应用，不重复转正。

桥使用 as-shot 或绝对 R/G/B/G WB。关闭 RawLab 默认工作流 baseline 和 JPEG 测光，将已经校验的 DNG BaselineExposure 作为一次曝光输入；无隐式 +0.7 EV，会话支持显影后线性曝光与相对 RGB WB；后者不是传感器 Kelvin 白平衡。负值/超白直到 Log/显示阶段仍保留；LibRaw 内部仍有 16 位运算，不宣称全浮点去马赛克。

原型支持 core 能识别的 CUBE（F-Log/F-Log2/F-Log2C/sRGB；对应显示/Log 输出），拒绝未知色彩合同；DCP 接入留后续。无第三方 LUT 随包分发，合成 identity LUT 用于自动化回归。

当前限制：

- RAWProcessor 接收 UTF-8 文件路径，不支持公共缓冲输入；非路径来源后续由来源适配准备受控临时文件，不能伪称无路径流程已完成。
- 上游 loadFile 已在返回前 unpack，128 MP 检查因此是活动区/输出限制，**不是解码前内存保证**。诊断入口额外限制源文件 1 GiB。必须在应用接入时纳入宿主字节预算；X-Trans 源快照还需约 8 字节/活动像素。
- 上游未公开中途取消入口。桥在阶段前后检查取消并释放/丢弃结果；库调用期间算力与许可仍被占用，不宣称立即停止。
- full 输出的 source_dimensions 为最终方向/几何尺寸；half-size 的精确源坐标映射未验证，返回 None，不能用于现有蒙版坐标。
- OpenMP 暂关闭，防止未接统一预算就超额并行。应用后续在预算内选择 C++ 并行，避免与 Rust 池嵌套满载。
- 一次性 film 入口仍有输入复制和 GPU 读回。新增持久色彩会话只复制源图一次，Windows/Android 复用 renderer 和源上传 revision；Metal 由上游管理内部 renderer。会话必须在同一线程创建、渲染和释放。应用层已接持久线程、版本化线性缓存、候选帧/激活、原生呈现与 JPEG 回退；其画质与交互帧率仍须用实际设备和样片验收。

## 来源与分发

RawLab 原创代码为 GPL-3.0-only，全文和来源声明保留在 upstream/LICENSE、upstream/docs/licensing.md。LibRaw 使用 LGPL 选项，LGPL/CDDL/COPYRIGHT 原文随固定下载源码保留，发布桥二进制时还需提供固定源码和本地补丁；Windows/macOS 安装包现在由 `scripts/build-rawlab.py` 构建并装配固定桥与许可；完整对应源码压缩包作为同一 CI artifact / release 资产交付。

RawLab 相机补丁/校准数据原样保留；RawSpeed contributors 的四款矩阵为 CC BY-SA 3.0（来源/改动在 upstream/lutools/third_party/rawspeed-camera-calibration.inc），Canon 的 Adobe DNG SDK 许可原文也保留。stb 等第三方代码的原始许可在源码头部/third_party 内，不被 GPL 原创声明覆盖。任何素材分发依据在实际打包前独立核实。


## 主编辑器接入

高级编辑器中的“胶片模拟”可显式启用胶片显影链，并导入 core 支持 Gamma/Gamut 合同的 CUBE。配方记录 colorPipeline 版本与 coreRevision，未知/不同 core 版本明确报错；普通创意 LUT 和局部调整仍由 PhotoCraft 执行。JPEG 缓存先编辑，RAW 在后台生成 1600 长边 float linear 代理；准备与激活分开，激活前用当前配方生成候选帧，前端解码并检查更新后发布。视口/工具/撤销栈不由源准备命令重置。

线性代理位于应用配置目录 smart-previews，缓存键包括来源路径/大小/mtime、core commit、处理合同和目标尺寸，带像素 SHA256 与尺寸/有限值校验；损坏会重新生成，磁盘上限 512 MiB。native 色彩会话最多两个、按源/上传估算上限 128 MiB、空闲 30 秒释放。这个估算不包含驱动、呈现器或上游内部的额外内存。后台显影和导出申请既有 CPU/512 MiB 内存许可；不可中断段结束前不提前释放。完整导出的色彩会话独立于预览队列，但共享 GPU 硬件仍可能产生竞争。

预览和导出共用同一线性色彩阶段；曝光与相对 RGB WB 在下游配方归零。完整 RAW 仅在后台准备或导出显影，胶片强度/后续调整不触发重新读 RAW。JPEG 快速代理只能近似 RAW 色彩，切换时画面可变化；本批不提供绝对传感器 Kelvin 控件、HDR 呈现或 DCP 资源入口。


### Lensfun

复用固定 RawLab 中的 `LensCorrectionDatabase` 和 `RAWProcessOptions::lensCorrection`，在传感器 RGB 阶段应用畸变、横向色差和自动裁边。配置匹配仅调用 `readLensMetadata`，不解包 RAW。镜头变更和传感器白平衡共同决定线性代理缓存；预览、100% 检查和全尺寸导出使用同一设置。当前不对 JPEG 再套用 RAW 镜头配置。

- 库源码版本和 SHA-256：`upstream/lutools/cmake/lens-dependencies.json`（Lensfun 0.3.4）。
- 离线数据库：`upstream/lutools/third_party/lensfun/version_1/*.xml`，原快照日期 2026-10-12；说明和许可证在其父目录。
- 打包后数据库：应用资源的 `rawlab/lensfun/version_1/`。`scripts/build-rawlab.py` 复制原文件，不在运行时联网更新。
- Windows 复用上游静态 Lensfun/GLib 构建并独立缓存；macOS 构建固定 Lensfun 静态库，随桥打包并签名 GLib 的完整动态依赖闭包。Unix 开发环境需要 `lensfun >= 0.3.4` 的 pkg-config 开发文件。
- 桥 ABI 4 的 `pc_raw_lens_catalog` 输出调用方持有的 UTF-8 JSON；解码选项 3 增加受信任数据库路径、稳定配置 ID 和操作标志。无匹配、无校准和原生不支持时明确报告失败，不伪装成已校正。

未进行编译或实图验证；需在后续获准验证后检查不同机型、焦距、方向及打包安装环境。
