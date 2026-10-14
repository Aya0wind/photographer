# 固定 RawLab core 的浮点接入原型（M0）

直接复用 RawLab core，**不自行实现 RAW 或胶片算法**。固定版本、未修改源码快照和更新方式见 [UPSTREAM.md](UPSTREAM.md)。Rust 薄封装在 `crates/media/src/rawlab.rs`，PhotoCraft 交接为 `photographer_lib::linear_rgb_document`。尚未替换主编辑器默认后端。

## 构建与使用

需要 C++17、CMake 3.24+、zlib 开发包。Windows CI 使用 `zlib:x64-windows-static-md`，传入 vcpkg toolchain/triplet。从仓库根目录执行：

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

上游现有公共 C ABI 只暴露 RGB8/RGBA8，不适合保存浮点线性代理。因此另加窄 C ABI，直接调用已有 C++ 实现，返回原生拥有的 float RGB。桥 ABI 为 2，解码 options 版本仍为 1，色彩 options 为 2；旧桥在加载时明确拒绝，避免结构体布局错读。静态检查 RGB 布局；Rust 后端生命周期覆盖图像，Drop 用同库释放。线性 sRGB/D65、display-sRGB 使用不同 encoding 标记；方向已应用，不重复转正。

桥使用 as-shot 或绝对 R/G/B/G WB。关闭 RawLab 默认工作流 baseline 和 JPEG 测光，将已经校验的 DNG BaselineExposure 作为一次曝光输入；无隐式 +0.7 EV，会话支持显影后线性曝光与相对 RGB WB；后者不是传感器 Kelvin 白平衡。负值/超白直到 Log/显示阶段仍保留；LibRaw 内部仍有 16 位运算，不宣称全浮点去马赛克。

原型支持 core 能识别的 CUBE（F-Log/F-Log2/F-Log2C/sRGB；对应显示/Log 输出），拒绝未知色彩合同；DCP 接入留后续。无第三方 LUT 随包分发，合成 identity LUT 用于自动化回归。

当前限制：

- RAWProcessor 接收 UTF-8 文件路径，不支持公共缓冲输入；非路径来源后续由来源适配准备受控临时文件，不能伪称无路径流程已完成。
- 上游 loadFile 已在返回前 unpack，128 MP 检查因此是活动区/输出限制，**不是解码前内存保证**。诊断入口额外限制源文件 1 GiB。必须在应用接入时纳入宿主字节预算；X-Trans 源快照还需约 8 字节/活动像素。
- 上游未公开中途取消入口。桥在阶段前后检查取消并释放/丢弃结果；库调用期间算力与许可仍被占用，不宣称立即停止。
- full 输出的 source_dimensions 为最终方向/几何尺寸；half-size 的精确源坐标映射未验证，返回 None，不能用于现有蒙版坐标。
- OpenMP 暂关闭，防止未接统一预算就超额并行。应用后续在预算内选择 C++ 并行，避免与 Rust 池嵌套满载。
- 一次性 film 入口仍有输入复制和 GPU 读回。新增持久色彩会话只复制源图一次，Windows/Android 复用 renderer 和源上传 revision；Metal 由上游管理内部 renderer。会话必须在同一线程创建、渲染和释放。应用级缓存、原生呈现和后台代理切换尚未接通，因此还不能视为最终实时帧循环。

## 来源与分发

RawLab 原创代码为 GPL-3.0-only，全文和来源声明保留在 upstream/LICENSE、upstream/docs/licensing.md。LibRaw 使用 LGPL 选项，LGPL/CDDL/COPYRIGHT 原文随固定下载源码保留，发布桥二进制时还需提供固定源码和本地补丁；当前原型产物不进入安装包。

RawLab 相机补丁/校准数据原样保留；RawSpeed contributors 的四款矩阵为 CC BY-SA 3.0（来源/改动在 upstream/lutools/third_party/rawspeed-camera-calibration.inc），Canon 的 Adobe DNG SDK 许可原文也保留。stb 等第三方代码的原始许可在源码头部/third_party 内，不被 GPL 原创声明覆盖。任何素材分发依据在实际打包前独立核实。
