# RawLab / LibRaw 移植实施记录

关联：[专项色彩合同](2026-10-12-libraw-smart-preview-film.md)、[阶段实施顺序](platform-evolution/implementation.md)、[roadmap](platform-evolution/roadmap.md)。

## 2026-10-14：第一批 M0 原型

移植前先将 dev 合入 main 并推送，基线合并提交 `fcf3257`。main 上最新编辑器改动保留，dev 同步该合并后开展移植。RawLab 参考源码固定为 `dcecbf82189360288085ee2d8017ed44f1eef919`。

### 本批代码

路线由用户确认：**直接复用固定版本 RawLab core，通过薄 Rust 封装接入**。不自行移植数学，不把 RAW 显影扩展为重写算法。

| 位置 | 已落地内容 |
| --- | --- |
| `native/rawlab/upstream` | 未修改的 RawLab core 源码快照，包含显影/胶片/平台 GPU 和必要依赖声明/补丁，不含客户端、LUT、RAW 样片 |
| `native/rawlab/CMakeLists.txt` | 固定 LibRaw 0.22.2 与官方 CMake 适配；配置上游 core 能力，原样应用其补丁 |
| `native/rawlab/src/photographer_raw.cpp` | float C ABI；直接调用 RAWProcessor、core 色彩/LUT 和 Metal/D3D11/GLES photo renderer；无 GPU 的 Linux 使用 CPU |
| `crates/media/src/rawlab.rs` | 可信库加载、版本/参数映射、编码标记、原生拥有的 RGB 借用与同库释放；请求 GPU 与实际成功分开 |
| `src-tauri/src/edit/photocraft.rs::linear_rgb_document` | float linear sRGB/D65 到 F32 PhotoCraft tiles，匹配 ICC，不提前裁掉负值/超白 |
| `src-tauri/examples/rawlab_probe.rs` | RAW → 线性文档交接 → core 中性/胶片 CPU/GPU → PhotoCraft PNG；原片只读，禁止覆盖输出 |
| `.github/workflows/media-core.yml` | Linux/macOS/Windows 原生构建、C ABI 与 Rust 跨边界用例 |
| Windows/macOS 安装包 CI | 原型示例和线性文档交接测试的编译检查 |

选择先交付 M0 验证入口；不跳过 M0–M4 把 Log LUT 直接套到旧显示 RGB 上。crate 暂用仓内 path 依赖，M1 再统一根 workspace/锁文件。主应用加载源、配方、预览与导出仍沿用既有后端；尚未开放新后端开关。上游已有 C ABI 只有 RGB8/RGBA8，所以浮点桥是必要的表示/生命周期适配，不是显影或胶片算法重写。

不捆绑第三方 LUT。使用 core 识别 Gamma/Gamut 合同的用户 CUBE，未知输入明确失败，DCP 待后续。归属/具体边界见 [原生说明](../../native/rawlab/README.md)。

### 实际验证证据

- 两份上游源码包下载和 SHA256 已核对，原样 RawLab 补丁对临时 LibRaw 0.22.2 源码完成静态应用检查。源码快照来源/更新方式记录在 [UPSTREAM.md](../../native/rawlab/UPSTREAM.md)。
- Rust 格式/语法解析、锁定 Cargo metadata、自有代码的 diff 空白检查执行；上游 `gles_lut.cpp:244` 原有行尾空白随未修改快照保留，不将清理空白混入上游算法快照；遵循编译交 CI 的约定，未本地构建/运行应用、样片或测试。
- 已写 C ABI 失败/取消/中性范围、Rust 合成 Log LUT 强度/CPU-GPU Auto 对照、GPU fallback/Force 和 PhotoCraft 浮点/ICC 交接测试。真实 RAW 生命周期用例需私有样片，显式 ignored。CI 结果待发布，不能计作已通过。
- 未验证实机 GPU 成功、全部目标原生库、代表相机画质、首帧延迟、峰值 RAM/VRAM 或 30 fps。M0 尚未达到完整出口，M5 未完成。

### 接下来实施顺序

1. **收口 M0**：以新 CI 修复跨平台编译问题；用 Bayer、X-Trans、DNG/压缩 RAW 样片记录浮点颜色、高光、方向、特殊几何/半尺寸、耗时与内存。旧 PhotoCraft、LibRaw/RawLab 的对比要使用同原图、明确曝光基准。不能以单张 DNG 测试代表全部机型。
2. **组织与共享边界 M1–M4**：按现有总计划迁根 workspace、稳定模型、来源与预算，提取现有处理链。现有工作代码优先搬迁/适配，不在这次原型里重写它们。
3. **LibRaw 新来源**：显式配方/后端版本；线性基础代理保留浮点范围，固定白平衡/曝光基准。所有后台 RAW 任务纳入宿主预算；源变化/切图/关闭丢弃过期结果。不可中断段仍持有许可。
4. **Smart Preview**：先 JPEG 可编辑，异步代理缓存、损坏重建、最新配方候选帧及原子切换。专项列出的视口、工具、历史和蒙版坐标必须保留。
5. **胶片编辑**：在正确色彩阶段增加资源身份/内容版本及面板；直接复用 core 的色域/Log/LUT 与平台 GPU，接通持久纹理/会话及 CPU 回退；是否再迁 wgpu 以数据交接和实测瓶颈决定，不重复实现算法。普通创意 LUT 位于后续，胶片/强度不能重新显影。
6. **导出/兼容**：完整 RAW 与代理共享色彩/配方合同，冻结资源版本，基准/WB/exposure 不重复应用。旧配方明确保持旧像素语义。再验收 CPU/GPU、导出/代理和真实交互性能。

### 本批边界与性能债务

OpenMP 暂关闭，宿主预算接入后再选择 C++ 并行。RawLab loadFile 已经 unpack，当前像素检查不代表解码前内存保证；完整字节预算和源租约仍待应用层接入。上游未公开中途取消，桥只在前后丢弃过期结果，库调用结束前不得释放许可。

LibRaw 内部仍有16位处理，不宣称全浮点去马赛克。float film 入口有输入复制/输出读回，Windows/Android renderer 目前一次性创建；尚未接持久缓存、原生呈现和编辑配方，所以不是最终实时帧循环。full 源尺寸为最终方向化尺寸，half-size 精确坐标未验证、返回 None；不可用于既有蒙版坐标。详细限制与复现入口见 [native/rawlab](../../native/rawlab/README.md) 和 [crates/media](../../crates/media/README.md)。
